#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Installs the normal ELF in a headless disposable
# Ubuntu overlay, then boots a child overlay in a bounded local QEMU window.

set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 7 ]] || vm_die \
    'usage: run-ubuntu-gui.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE BOOTART_BIN QEMU QEMU_IMG'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4
bootart_bin=$5
configured_qemu=$6
configured_qemu_img=$7

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
[[ -f "$matrix_file" && ! -L "$matrix_file" ]] || vm_die 'adapter matrix is missing or unsafe'
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'Ubuntu GUI requires the repository artifact lock'

bootart_physical="$(readlink -f -- "$bootart_bin")" || vm_die 'cannot resolve Bootart ELF'
case "$bootart_physical" in
    "$repo_root/target/artifacts/generations/"*/release/bootart) ;;
    *) vm_die 'Ubuntu GUI accepts only the ordinary immutable release ELF' ;;
esac
[[ -f "$bootart_physical" && ! -L "$bootart_physical" ]] || vm_die 'release ELF is unsafe'
vm_assert_owned "$bootart_physical"
READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
    x86_64 "$bootart_physical"

lock_record="$(vm_lock_record "$lock_file" ubuntu-26.04-dracut-systemd-amd64-derived)"
IFS='|' read -r _ lock_status _ _ _ _ _ _ _ _ _ max_run_bytes max_file_bytes \
    _ _ <<< "$lock_record"
[[ "$lock_status" == derived ]] || vm_die 'Ubuntu GUI requires the authenticated derived base'
vm_is_positive_byte_count "$max_run_bytes" || vm_die 'Ubuntu GUI run-byte limit is invalid'
vm_is_positive_byte_count "$max_file_bytes" || vm_die 'Ubuntu GUI file-byte limit is invalid'
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

install_log=
gui_run=
cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    if [[ -n "$install_log" && "$install_log" == "$vm_root"/.ubuntu-gui-install.* &&
          -f "$install_log" && ! -L "$install_log" &&
          "$(vm_stat_uid "$install_log")" == "$(id -u)" ]]; then
        rm -f -- "$install_log" || true
    fi
    if [[ -n "$gui_run" && "$gui_run" == "$vm_root/runs/"* ]] &&
       vm_validate_run "$vm_root" "$gui_run" >/dev/null 2>&1 &&
       vm_assert_no_mount_below "$gui_run" >/dev/null 2>&1; then
        find "$gui_run" -xdev -depth -mindepth 1 -delete 2>/dev/null || true
        rmdir -- "$gui_run" 2>/dev/null || true
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

expected_result='BOOTART_VM_LANE_STATUS_V3|fixture=ubuntu-26.04-dracut-systemd|pair=dracut-systemd|lane=install|status=PASS|image=ubuntu-26.04-dracut-systemd-amd64-derived|oracle=BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1|reason=exact-serial-oracle'
legacy_expected_result='BOOTART_VM_LANE_STATUS_V2|pair=dracut-systemd|lane=install|status=PASS|image=ubuntu-26.04-dracut-systemd-amd64-derived|oracle=BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1|reason=exact-serial-oracle'
recoverable_failure='BOOTART_VM_LANE_STATUS_V3|fixture=ubuntu-26.04-dracut-systemd|pair=dracut-systemd|lane=install|status=FAIL|image=ubuntu-26.04-dracut-systemd-amd64-derived|oracle=BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1|reason=infrastructure-error'
legacy_recoverable_failure='BOOTART_VM_LANE_STATUS_V2|pair=dracut-systemd|lane=install|status=FAIL|image=ubuntu-26.04-dracut-systemd-amd64-derived|oracle=BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1|reason=infrastructure-error'
bootart_digest="$(sha256sum -- "$bootart_physical" | awk '{ print $1 }')"
[[ "$bootart_digest" =~ ^[0-9a-f]{64}$ ]] || vm_die 'cannot hash release ELF'

# A full install lane is intentionally thorough: it installs, checks an
# idempotent second apply, reboots, unlocks through Bootart, verifies the
# initramfs copy, and powers off. Repeating all of that before every visual
# boot made this target appear broken for many minutes under TCG. Reuse a
# retained PASS run only when it proves this exact ELF and is no longer live.
install_run=
install_run_recovered=0
for candidate in "$vm_root"/runs/run.*; do
    [[ -d "$candidate" && ! -L "$candidate" ]] || continue
    vm_validate_run "$vm_root" "$candidate" >/dev/null 2>&1 || continue
    [[ -f "$candidate/lane.result" && ! -L "$candidate/lane.result" ]] || continue
    candidate_result="$(cat -- "$candidate/lane.result")"
    [[ "$candidate_result" == "$expected_result" ||
       "$candidate_result" == "$legacy_expected_result" ]] || continue
    [[ -f "$candidate/serial.log" && ! -L "$candidate/serial.log" ]] || continue
    grep -a -F -q -- "bootart-sha256=$bootart_digest" "$candidate/serial.log" || continue
    grep -a -F -q -- 'BOOTART_VM_INSTALL_REBOOT_HASH_V1' "$candidate/serial.log" || continue
    grep -a -F -q -- 'BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1' "$candidate/serial.log" || continue
    vm_pid_matches_run "$candidate" && continue
    install_run=$candidate
    break
done

# The proof wrapper publishes lane.result as its last durable operation.  If a
# developer interrupts the outer Make process after the guest has powered off
# but before that final rename, the installed overlay can have no result or an
# infrastructure-only FAIL. Do not repeat a many-minute TCG install merely
# because of that interruption. A GUI-only recovery is
# allowed when every retained completion artifact independently agrees: the
# stopped VM used the current ELF, the validated QEMU argv is unchanged, the
# secret scan is empty, and the ordered serial oracle still passes.  This does
# not manufacture or promote lane.result; release gates continue to require a
# normally completed proof transaction.
if [[ -z "$install_run" ]]; then
    for candidate in "$vm_root"/runs/run.*; do
        [[ -d "$candidate" && ! -L "$candidate" ]] || continue
        vm_validate_run "$vm_root" "$candidate" >/dev/null 2>&1 || continue
        candidate_result=
        if [[ -e "$candidate/lane.result" || -L "$candidate/lane.result" ]]; then
            [[ -f "$candidate/lane.result" && ! -L "$candidate/lane.result" ]] || continue
            candidate_result="$(cat -- "$candidate/lane.result")"
            [[ "$candidate_result" == "$recoverable_failure" ||
               "$candidate_result" == "$legacy_recoverable_failure" ]] || continue
        fi
        vm_pid_matches_run "$candidate" && continue
        for required in lane.meta overlay.qcow2 OVMF_VARS.fd serial.log \
            qemu.args qemu.policy.sha256 qemu.stderr; do
            [[ -f "$candidate/$required" && ! -L "$candidate/$required" ]] || continue 2
            vm_assert_owned "$candidate/$required" || continue 2
        done
        qmp_evidence=
        if [[ -f "$candidate/qmp.log" && ! -L "$candidate/qmp.log" ]]; then
            qmp_evidence="$candidate/qmp.log"
        else
            mapfile -t temporary_qmp_evidence < <(
                find "$candidate" -xdev -mindepth 1 -maxdepth 1 -type f \
                    -name '.qmp.log.*' -print
            )
            [[ ${#temporary_qmp_evidence[@]} -eq 1 ]] || continue
            qmp_evidence=${temporary_qmp_evidence[0]}
        fi
        vm_assert_owned "$qmp_evidence" || continue
        [[ "$(sed -n 's/^pair=//p' "$candidate/lane.meta")" == dracut-systemd &&
           "$(sed -n 's/^lane=//p' "$candidate/lane.meta")" == install &&
           "$(sed -n 's/^image=//p' "$candidate/lane.meta")" == \
               ubuntu-26.04-dracut-systemd-amd64-derived &&
           "$(sed -n 's/^oracle=//p' "$candidate/lane.meta")" == \
               BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1 ]] || continue
        candidate_policy_hash="$(cat -- "$candidate/qemu.policy.sha256")"
        [[ "$candidate_policy_hash" =~ ^[0-9a-f]{64}$ &&
           "$(sha256sum -- "$candidate/qemu.args" | awk '{ print $1 }')" == \
               "$candidate_policy_hash" ]] || continue
        [[ ! -s "$candidate/qemu.stderr" ]] || continue
        grep -a -F -q -- "bootart-sha256=$bootart_digest" "$candidate/serial.log" || continue
        [[ "$(awk '{ line = $0; sub(/\r$/, "", line); if (line == \
            "BOOTART_VM_INSTALL_REBOOT_HASH_V1") count++ } END { print count + 0 }' \
            "$candidate/serial.log")" == 1 ]] || continue
        # An infrastructure-error can be the QMP socket disappearing during
        # the guest's final poweroff. In that case the temporary driver log is
        # intentionally incomplete. Do not require optional progress lines
        # from it: the ordered serial oracle below already proves transport
        # removal, the Bootart reboot/unlock, the disk-only status/hash, and
        # final shutdown. The QMP artifact is still required to be a private,
        # owned regular file so recovery cannot ignore forged filesystem state.
        bash "$SCRIPT_DIR/check-adapter-oracle.sh" "$candidate/serial.log" \
            BOOTART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1 >/dev/null 2>&1 || continue

        if [[ -e "$candidate/secret-scan.matches" ||
              -L "$candidate/secret-scan.matches" ]]; then
            [[ -f "$candidate/secret-scan.matches" &&
               ! -L "$candidate/secret-scan.matches" &&
               ! -s "$candidate/secret-scan.matches" ]] || continue
            vm_assert_owned "$candidate/secret-scan.matches" || continue
        else
            recovery_scan="$(mktemp "$vm_root/.ubuntu-gui-recovery-scan.XXXXXXXXXX")" ||
                vm_die 'cannot allocate GUI-only recovery secret scan'
            chmod 0600 -- "$recovery_scan"
            printf -v recovery_secret '%s%s' 112 358
            exec 8< <(printf '%s\n' "$recovery_secret")
            set +e
            timeout --signal=TERM --kill-after=5s 30s \
                bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
                bash "$SCRIPT_DIR/scan-secret-artifacts.sh" \
                "$candidate" "$candidate/overlay.qcow2" 8 > "$recovery_scan"
            recovery_scan_status=$?
            set -e
            exec 8<&-
            unset recovery_secret
            rm -f -- "$recovery_scan"
            [[ $recovery_scan_status -eq 1 ]] || continue
        fi
        install_run=$candidate
        install_run_recovered=1
        printf 'bootart-vm: recovering completed stopped install for GUI only: %s\n' \
            "$install_run"
        break
    done
fi

if [[ -n "$install_run" ]]; then
    if [[ $install_run_recovered -eq 0 ]]; then
        printf 'bootart-vm: reusing authenticated install evidence for this exact ELF: %s\n' \
            "$install_run"
    fi
else
    printf '%s\n' 'bootart-vm: no matching installed evidence exists; running the one-time headless install proof'
    printf '%s\n' 'bootart-vm: this can take several minutes under TCG; the GUI opens immediately on later runs of the same ELF'
    install_log="$(mktemp "$vm_root/.ubuntu-gui-install.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private install transcript'
    set +e
    BOOTART_BIN="$bootart_physical" QEMU="$configured_qemu" QEMU_IMG="$configured_qemu_img" \
        make --no-print-directory -C "$repo_root/scripts/vm" \
        vm-test-install-dracut-systemd 2>&1 | tee "$install_log"
    install_status=${PIPESTATUS[0]}
    set -e
    [[ $install_status -eq 0 ]] || vm_die 'normal release ELF install lane failed before GUI boot'

    mapfile -t install_runs < <(
        sed -n 's/^bootart-vm: unpromoted adapter evidence retained: //p' "$install_log"
    )
    [[ ${#install_runs[@]} -eq 1 ]] || vm_die 'install lane did not identify exactly one evidence run'
    install_run=${install_runs[0]}
    vm_validate_run "$vm_root" "$install_run"
fi

if [[ $install_run_recovered -eq 0 &&
      ( -e "$install_run/lane.result" || -L "$install_run/lane.result" ) ]]; then
    [[ -f "$install_run/lane.result" && ! -L "$install_run/lane.result" &&
       ( "$(cat -- "$install_run/lane.result")" == "$expected_result" ||
         "$(cat -- "$install_run/lane.result")" == "$legacy_expected_result" ) ]] ||
        vm_die 'GUI source overlay lacks the exact authenticated install result'
fi
source_overlay="$install_run/overlay.qcow2"
source_vars="$install_run/OVMF_VARS.fd"
for source in "$source_overlay" "$source_vars"; do
    [[ -f "$source" && ! -L "$source" && "$(vm_stat_mode "$source")" == 600 ]] ||
        vm_die "GUI source artifact is missing or unsafe: $source"
    vm_assert_owned "$source"
done
source_overlay_digest="$(sha256sum -- "$source_overlay" | awk '{ print $1 }')"
source_vars_digest="$(sha256sum -- "$source_vars" | awk '{ print $1 }')"

qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'GUI QEMU_IMG executable'

gui_run="$(vm_create_run "$vm_root")"
gui_overlay="$gui_run/overlay.qcow2"
gui_vars="$gui_run/OVMF_VARS.fd"
timeout --signal=TERM --kill-after=5s 30s "$qemu_img" create \
    -f qcow2 -F qcow2 -b "$source_overlay" "$gui_overlay" >/dev/null
chmod 0600 -- "$gui_overlay"
cp -- "$source_vars" "$gui_vars"
chmod 0600 -- "$gui_vars"
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'GUI QEMU_IMG executable'
vm_assert_qcow2_backing_file "$gui_overlay" "$source_overlay"
vm_assert_file_size_at_most "$gui_overlay" "$max_file_bytes" 'Ubuntu GUI overlay'
vm_assert_file_size_at_most "$gui_vars" "$max_file_bytes" 'Ubuntu GUI firmware state'
vm_assert_run_bytes_at_most "$vm_root" "$gui_run" "$max_run_bytes"

ovmf_code=
for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
    if [[ -f "$candidate" && ! -L "$candidate" ]]; then
        ovmf_code=$candidate
        break
    fi
done
[[ -n "$ovmf_code" ]] || vm_die 'cannot resolve read-only OVMF code'

qemu_supports_display() {
    local executable=$1 backend=$2
    "$executable" -display help 2>&1 | grep -Fx -- "$backend" >/dev/null
}

declare -a qemu_candidates=("$(vm_resolve_qemu "$configured_qemu")")
if command -v -- bootart-qemu-gui >/dev/null 2>&1; then
    qemu_candidates+=(bootart-qemu-gui)
fi
if [[ -x /usr/bin/qemu-system-x86_64 ]]; then
    qemu_candidates+=(/usr/bin/qemu-system-x86_64)
fi
qemu=
display_backend=
for candidate in "${qemu_candidates[@]}"; do
    candidate="$(vm_resolve_qemu "$candidate")"
    if qemu_supports_display "$candidate" gtk; then
        qemu=$candidate
        display_backend=gtk,gl=off
        break
    elif qemu_supports_display "$candidate" sdl; then
        qemu=$candidate
        display_backend=sdl
        break
    fi
done
[[ -n "$qemu" ]] || vm_die 'no window-capable QEMU with GTK or SDL is available'
qemu_identity="$(vm_executable_identity "$qemu")"
vm_assert_executable_identity "$qemu" "$qemu_identity" 'Ubuntu GUI QEMU executable'

# Accept only a live same-user compositor socket, with the same conservative
# fallback used by the small visual smoke target.
wayland_socket=
if [[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" && -d "$XDG_RUNTIME_DIR" ]]; then
    case "$WAYLAND_DISPLAY" in
        /*) wayland_socket=$WAYLAND_DISPLAY ;;
        *) wayland_socket="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ;;
    esac
fi
if [[ -n "$wayland_socket" && -S "$wayland_socket" ]]; then
    :
elif [[ -n "${XDG_RUNTIME_DIR:-}" && -d "$XDG_RUNTIME_DIR" &&
        ! -L "$XDG_RUNTIME_DIR/wayland-0" && -S "$XDG_RUNTIME_DIR/wayland-0" &&
        "$(vm_stat_uid "$XDG_RUNTIME_DIR/wayland-0")" == "$(id -u)" ]]; then
    export WAYLAND_DISPLAY=wayland-0
elif [[ -n "${DISPLAY:-}" ]]; then
    unset WAYLAND_DISPLAY
else
    vm_die 'no live graphical session found; set WAYLAND_DISPLAY or DISPLAY'
fi

printf 'bootart-vm: launching installed Ubuntu GUI from private run %s\n' "$gui_run"
printf '%s\n' 'bootart-vm: click the window and type 112358 in the centered Bootart prompt'
printf '%s\n' 'bootart-vm: close the window after login is visible; the hard deadline is 15 minutes'

set +e
timeout --signal=TERM --kill-after=5s 900s \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" "$qemu" \
    -name bootart-ubuntu-26.04-gui \
    -nodefaults \
    -no-user-config \
    -machine q35,accel=tcg \
    -cpu max \
    -smp 2 \
    -m 4096M \
    -display "$display_backend" \
    -device VGA,id=video \
    -device qemu-xhci,id=xhci \
    -device usb-kbd,bus=xhci.0 \
    -serial null \
    -monitor none \
    -nic none \
    -no-reboot \
    -boot c,strict=on \
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code" \
    -drive "if=pflash,format=raw,unit=1,file=$gui_vars" \
    -drive "file=$gui_overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=allow,resourcecontrol=deny
qemu_status=$?
set -e

vm_assert_executable_identity "$qemu" "$qemu_identity" 'Ubuntu GUI QEMU executable'
vm_assert_file_size_at_most "$gui_overlay" "$max_file_bytes" 'Ubuntu GUI overlay after boot'
vm_assert_file_size_at_most "$gui_vars" "$max_file_bytes" 'Ubuntu GUI firmware after boot'
vm_assert_run_bytes_at_most "$vm_root" "$gui_run" "$max_run_bytes"
[[ "$(sha256sum -- "$source_overlay" | awk '{ print $1 }')" == "$source_overlay_digest" ]] ||
    vm_die 'GUI changed the authenticated installed source overlay'
[[ "$(sha256sum -- "$source_vars" | awk '{ print $1 }')" == "$source_vars_digest" ]] ||
    vm_die 'GUI changed the authenticated installed firmware state'

case "$qemu_status" in
    0) ;;
    124|137) printf '%s\n' 'bootart-vm: Ubuntu GUI deadline reached; QEMU was terminated' ;;
    *) exit "$qemu_status" ;;
esac
