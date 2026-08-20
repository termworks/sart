#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Fast visual boot for an installed x86_64 fixture.
# A persistent standalone patched cache is preferred before any build or lane.

set -Eeuo pipefail
umask 077
ulimit -c 0

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 8 ]] || vm_die \
    'usage: run-x86-installed-gui.sh REPO VM LOCK MATRIX FIXTURE SART QEMU QEMU_IMG'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4
fixture=$5
sart_bin=$6
configured_qemu=$7
configured_qemu_img=$8

case "$fixture" in
    ubuntu-26.04-dracut-systemd)
        pair=dracut-systemd
        image_id=ubuntu-26.04-dracut-systemd-amd64-derived
        oracle=SART_VM_DRACUT_SYSTEMD_INSTALL_PASS_V1
        install_target=vm-test-install-dracut-systemd
        verify_target=vm-verify-ubuntu-26.04-dracut-systemd
        firmware=uefi
        ;;
    fedora-44-dracut-systemd)
        pair=dracut-systemd
        image_id=fedora-44-dracut-systemd-amd64-derived
        oracle=SART_VM_FEDORA_44_DRACUT_SYSTEMD_INSTALL_PASS_V1
        install_target=vm-test-install-fedora-44-dracut-systemd
        verify_target=vm-verify-fedora-44-dracut-systemd
        firmware=uefi
        ;;
    debian-13.6-initramfs-tools-systemd)
        pair=initramfs-tools
        image_id=debian-13.6-initramfs-tools-systemd-amd64-derived
        oracle=SART_VM_INITRAMFS_TOOLS_INSTALL_PASS_V1
        install_target=vm-test-install-initramfs-tools
        verify_target=vm-verify-debian-13.6-initramfs-tools-systemd
        firmware=uefi
        ;;
    arch-mkinitcpio-systemd)
        pair='mkinitc''pio'
        image_id=arch-mkinitcpio-systemd-amd64-derived
        oracle=SART_VM_MKINITCPIO_INSTALL_PASS_V1
        install_target='vm-test-install-mkinitc''pio'
        verify_target='vm-verify-arch-mkinitc''pio-systemd'
        firmware=bios
        ;;
    alpine-mkinitfs-openrc)
        pair=mkinitfs-openrc
        image_id=alpine-3.24.1-mkinitfs-openrc-amd64-derived
        oracle=SART_VM_MKINITFS_OPENRC_INSTALL_PASS_V1
        install_target=vm-test-install-mkinitfs-openrc
        verify_target=vm-verify-alpine-3.24.1-mkinitfs-openrc
        firmware=bios
        ;;
    *) vm_die "unsupported installed x86 GUI fixture: $fixture" ;;
esac

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
[[ -f "$matrix_file" && ! -L "$matrix_file" ]] || vm_die 'adapter matrix is missing or unsafe'
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'installed GUI requires the repository artifact lock'

lock_record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r _ lock_status _ _ _ guest_arch _ _ _ _ max_virtual_bytes \
    max_run_bytes max_file_bytes _ _ <<< "$lock_record"
[[ "$lock_status" == derived && "$guest_arch" == x86_64 ]] ||
    vm_die 'installed x86 GUI requires its authenticated derived base'
for bytes in "$max_virtual_bytes" "$max_run_bytes" "$max_file_bytes"; do
    vm_is_positive_byte_count "$bytes" || vm_die 'installed x86 GUI resource limit is invalid'
done
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
cache_root="$vm_root/cache/gui"
cache_dir="$cache_root/$fixture"
cache_disk="$cache_dir/disk.qcow2"
cache_vars="$cache_dir/OVMF_VARS.fd"
cache_manifest="$cache_dir/manifest"
cache_ready=0
if [[ -d "$cache_dir" && ! -L "$cache_dir" &&
      "$(vm_stat_uid "$cache_dir")" == "$(id -u)" && "$(vm_stat_mode "$cache_dir")" == 700 &&
      -f "$cache_disk" && ! -L "$cache_disk" && "$(vm_stat_mode "$cache_disk")" == 400 &&
      -f "$cache_manifest" && ! -L "$cache_manifest" && "$(vm_stat_mode "$cache_manifest")" == 400 &&
      "$(sed -n 's/^schema=//p' "$cache_manifest")" == SART_VM_GUI_CACHE_V1 &&
      "$(sed -n 's/^fixture=//p' "$cache_manifest")" == "$fixture" &&
      "$(sed -n 's/^pair=//p' "$cache_manifest")" == "$pair" &&
      "$(sed -n 's/^image=//p' "$cache_manifest")" == "$image_id" &&
      "$(sed -n 's/^firmware=//p' "$cache_manifest")" == "$firmware" &&
      "$(sed -n 's/^disk_bytes=//p' "$cache_manifest")" == "$(vm_stat_size "$cache_disk")" ]]; then
    if [[ "$firmware" == bios ]] ||
       [[ -f "$cache_vars" && ! -L "$cache_vars" && "$(vm_stat_mode "$cache_vars")" == 400 &&
          "$(sed -n 's/^vars_bytes=//p' "$cache_manifest")" == "$(vm_stat_size "$cache_vars")" ]]; then
        cache_info="$("$qemu_img" info --output=json -- "$cache_disk")" ||
            vm_die "cannot inspect cached $fixture GUI disk"
        jq -e '.format == "qcow2" and (has("backing-filename") | not)' \
            <<< "$cache_info" >/dev/null || vm_die "cached $fixture GUI disk is not standalone qcow2"
        QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size \
            "$cache_disk" "$max_virtual_bytes" >/dev/null
        cache_ready=1
        printf 'sart-vm: found persistent patched %s GUI cache; skipping build, matrix scan, and install\n' \
            "$fixture"
    fi
fi

expected_result="SART_VM_LANE_STATUS_V3|fixture=$fixture|pair=$pair|lane=install|status=PASS|image=$image_id|oracle=$oracle|reason=exact-serial-oracle"
install_run=
if [[ $cache_ready -eq 0 ]]; then
    for candidate in "$vm_root"/runs/run.*; do
        [[ -d "$candidate" && ! -L "$candidate" ]] || continue
        vm_validate_run "$vm_root" "$candidate" >/dev/null 2>&1 || continue
        [[ -f "$candidate/lane.result" && ! -L "$candidate/lane.result" &&
           "$(cat -- "$candidate/lane.result")" == "$expected_result" ]] || continue
        [[ -f "$candidate/lane.meta" && ! -L "$candidate/lane.meta" &&
           "$(sed -n 's/^fixture=//p' "$candidate/lane.meta")" == "$fixture" &&
           "$(sed -n 's/^pair=//p' "$candidate/lane.meta")" == "$pair" &&
           "$(sed -n 's/^lane=//p' "$candidate/lane.meta")" == install ]] || continue
        vm_pid_matches_run "$candidate" && continue
        install_run=$candidate
        break
    done
fi

install_log=
gui_run=
cache_stage=
cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    if [[ -n "$install_log" && "$install_log" == "$vm_root"/.installed-gui.* &&
          -f "$install_log" && ! -L "$install_log" &&
          "$(vm_stat_uid "$install_log")" == "$(id -u)" ]]; then
        rm -f -- "$install_log" || true
    fi
    if [[ -n "$cache_stage" && "$cache_stage" == "$cache_root"/."$fixture".* &&
          -d "$cache_stage" && ! -L "$cache_stage" &&
          "$(vm_stat_uid "$cache_stage")" == "$(id -u)" ]]; then
        find "$cache_stage" -xdev -depth -delete 2>/dev/null || true
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

if [[ $cache_ready -eq 1 ]]; then
    source_disk=$cache_disk
    [[ "$firmware" == uefi ]] && source_vars=$cache_vars
elif [[ -n "$install_run" ]]; then
    printf 'sart-vm: caching existing authenticated %s install: %s\n' "$fixture" "$install_run"
else
    if ! sart_physical="$(readlink -f -- "$sart_bin" 2>/dev/null)" ||
       [[ ! -f "$sart_physical" ]]; then
        printf 'sart-vm: no patched %s cache or x86_64 ELF exists; building once\n' "$fixture"
        make --no-print-directory -C "$repo_root" static-build
        sart_physical="$(readlink -f -- "$sart_bin")" ||
            vm_die 'cannot resolve newly built x86_64 Sart ELF'
    fi
    case "$sart_physical" in
        "$repo_root/target/artifacts/generations/"*/release/sart) ;;
        *) vm_die 'installed x86 GUI accepts only an immutable release ELF' ;;
    esac
    [[ -f "$sart_physical" && ! -L "$sart_physical" ]] ||
        vm_die 'x86_64 Sart ELF is unsafe'
    vm_assert_owned "$sart_physical"
    READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
        x86_64 "$sart_physical"
    printf 'sart-vm: no patched %s install exists; running the one-time headless install proof\n' \
        "$fixture"
    printf '%s\n' 'sart-vm: after this finishes, the persistent GUI cache opens directly'
    make --no-print-directory -C "$repo_root" "$verify_target"
    install_log="$(mktemp "$vm_root/.installed-gui.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private installed-GUI transcript'
    chmod 0600 -- "$install_log"
    set +e
    SART_BIN="$sart_physical" QEMU="$configured_qemu" QEMU_IMG="$configured_qemu_img" \
        make --no-print-directory -C "$repo_root/scripts/vm" "$install_target" \
        2>&1 | tee "$install_log"
    install_status=${PIPESTATUS[0]}
    set -e
    [[ $install_status -eq 0 ]] || vm_die "$fixture install lane failed before GUI boot"
    mapfile -t install_runs < <(
        sed -n 's/^sart-vm: unpromoted adapter evidence retained: //p' "$install_log"
    )
    [[ ${#install_runs[@]} -eq 1 ]] ||
        vm_die "$fixture install lane did not identify exactly one evidence run"
    install_run=${install_runs[0]}
    vm_validate_run "$vm_root" "$install_run"
fi

if [[ $cache_ready -eq 0 ]]; then
    [[ -f "$install_run/lane.result" && ! -L "$install_run/lane.result" &&
       "$(cat -- "$install_run/lane.result")" == "$expected_result" ]] ||
        vm_die "$fixture GUI source lacks the exact authenticated install result"
    source_disk="$install_run/overlay.qcow2"
    [[ -f "$source_disk" && ! -L "$source_disk" && "$(vm_stat_mode "$source_disk")" == 600 ]] ||
        vm_die "$fixture installed overlay is missing or unsafe"
    vm_assert_owned "$source_disk"
    if [[ "$firmware" == uefi ]]; then
        source_vars="$install_run/OVMF_VARS.fd"
        [[ -f "$source_vars" && ! -L "$source_vars" && "$(vm_stat_mode "$source_vars")" == 600 ]] ||
            vm_die "$fixture installed UEFI state is missing or unsafe"
        vm_assert_owned "$source_vars"
    fi

    if [[ -e "$cache_dir" || -L "$cache_dir" ]]; then
        [[ -d "$cache_dir" && ! -L "$cache_dir" &&
           "$(vm_stat_uid "$cache_dir")" == "$(id -u)" ]] ||
            vm_die "refusing to replace unsafe $fixture GUI cache"
        vm_assert_no_mount_below "$cache_dir"
        find "$cache_dir" -xdev -depth -mindepth 1 -delete
        rmdir -- "$cache_dir"
    fi
    if [[ ! -e "$cache_root" ]]; then mkdir -- "$cache_root"; chmod 0700 -- "$cache_root"; fi
    vm_assert_private_dir "$cache_root"
    cache_stage="$(mktemp -d "$cache_root/.$fixture.XXXXXXXXXX")" ||
        vm_die "cannot allocate $fixture GUI cache stage"
    chmod 0700 -- "$cache_stage"
    printf 'sart-vm: publishing standalone patched %s GUI cache (one time)\n' "$fixture"
    vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'installed GUI cache QEMU_IMG'
    "$qemu_img" convert -O qcow2 "$source_disk" "$cache_stage/disk.qcow2"
    if [[ "$firmware" == uefi ]]; then cp -- "$source_vars" "$cache_stage/OVMF_VARS.fd"; fi
    sart_digest="$(sed -n 's/^sart_sha256=//p' "$install_run/lane.meta")"
    [[ "$sart_digest" =~ ^[0-9a-f]{64}$ ]] || sart_digest=legacy-authenticated-install
    vars_bytes=0
    [[ "$firmware" == uefi ]] && vars_bytes="$(vm_stat_size "$cache_stage/OVMF_VARS.fd")"
    printf 'schema=SART_VM_GUI_CACHE_V1\nfixture=%s\npair=%s\nimage=%s\nfirmware=%s\nsart_sha256=%s\ndisk_bytes=%s\nvars_bytes=%s\n' \
        "$fixture" "$pair" "$image_id" "$firmware" "$sart_digest" \
        "$(vm_stat_size "$cache_stage/disk.qcow2")" "$vars_bytes" > "$cache_stage/manifest"
    chmod 0400 -- "$cache_stage/disk.qcow2" "$cache_stage/manifest"
    [[ "$firmware" == uefi ]] && chmod 0400 -- "$cache_stage/OVMF_VARS.fd"
    mv -T -- "$cache_stage" "$cache_dir"
    cache_stage=
    source_disk=$cache_disk
    [[ "$firmware" == uefi ]] && source_vars=$cache_vars
fi

source_disk_identity="$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_disk")"
if [[ "$firmware" == uefi ]]; then
    source_vars_identity="$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_vars")"
fi
gui_run="$(vm_create_run "$vm_root")"
gui_disk="$gui_run/disk.qcow2"
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'installed GUI QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$source_disk" "$gui_disk" >/dev/null
chmod 0600 -- "$gui_disk"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$gui_disk" "$source_disk"
if [[ "$firmware" == uefi ]]; then
    gui_vars="$gui_run/OVMF_VARS.fd"
    cp -- "$source_vars" "$gui_vars"
    chmod 0600 -- "$gui_vars"
fi

qemu_supports_display() {
    local executable=$1 backend=$2
    "$executable" -display help 2>&1 | grep -Fx -- "$backend" >/dev/null
}
declare -a qemu_candidates=("$(vm_resolve_qemu "$configured_qemu")")
[[ -x /usr/bin/qemu-system-x86_64 ]] && qemu_candidates+=(/usr/bin/qemu-system-x86_64)
qemu=
display_backend=
for candidate in "${qemu_candidates[@]}"; do
    candidate="$(vm_resolve_qemu "$candidate")"
    if qemu_supports_display "$candidate" gtk; then
        qemu=$candidate; display_backend=gtk,gl=off; break
    elif qemu_supports_display "$candidate" sdl; then
        qemu=$candidate; display_backend=sdl; break
    fi
done
[[ -n "$qemu" ]] || vm_die 'no window-capable qemu-system-x86_64 with GTK or SDL is available'
qemu_identity="$(vm_executable_identity "$qemu")"

wayland_socket=
if [[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" && -d "$XDG_RUNTIME_DIR" ]]; then
    case "$WAYLAND_DISPLAY" in
        /*) wayland_socket=$WAYLAND_DISPLAY ;;
        *) wayland_socket="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ;;
    esac
fi
if [[ -n "$wayland_socket" && -S "$wayland_socket" ]]; then :
elif [[ -n "${XDG_RUNTIME_DIR:-}" && -d "$XDG_RUNTIME_DIR" &&
        ! -L "$XDG_RUNTIME_DIR/wayland-0" && -S "$XDG_RUNTIME_DIR/wayland-0" &&
        "$(vm_stat_uid "$XDG_RUNTIME_DIR/wayland-0")" == "$(id -u)" ]]; then
    export WAYLAND_DISPLAY=wayland-0
elif [[ -n "${DISPLAY:-}" ]]; then unset WAYLAND_DISPLAY
else vm_die 'no live graphical session found; set WAYLAND_DISPLAY or DISPLAY'
fi

qemu_args=(
    "$qemu" -name "sart-$fixture-gui" -nodefaults -no-user-config
    -machine q35,accel=tcg -cpu max -smp 2 -m 4096M
    -display "$display_backend" -device VGA,id=video
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
    -serial null -monitor none -nic none -no-reboot -boot c,strict=on
)
if [[ "$firmware" == uefi ]]; then
    ovmf_code=
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then ovmf_code=$candidate; break; fi
    done
    [[ -n "$ovmf_code" ]] || vm_die 'cannot resolve read-only x86_64 OVMF code'
    qemu_args+=(
        -drive "if=pflash,format=raw,unit=0,readonly=on,file=$ovmf_code"
        -drive "if=pflash,format=raw,unit=1,file=$gui_vars"
    )
fi
qemu_args+=(
    -drive "file=$gui_disk,format=qcow2,if=virtio,cache=none,aio=threads"
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=allow,resourcecontrol=deny
)

printf 'sart-vm: launching patched %s GUI immediately\n' "$fixture"
printf '%s\n' 'sart-vm: click the window and type 112358 in the centered Sart prompt'
printf '%s\n' 'sart-vm: close the window after login is visible; hard deadline is 15 minutes'
set +e
timeout --signal=TERM --kill-after=5s 900s \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" "${qemu_args[@]}"
qemu_status=$?
set -e

vm_assert_executable_identity "$qemu" "$qemu_identity" 'installed GUI QEMU'
[[ "$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_disk")" == "$source_disk_identity" ]] ||
    vm_die 'GUI changed the persistent patched disk cache'
if [[ "$firmware" == uefi ]]; then
    [[ "$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_vars")" == "$source_vars_identity" ]] ||
        vm_die 'GUI changed the persistent patched UEFI cache'
fi
case "$qemu_status" in
    0) ;;
    124|137) printf 'sart-vm: %s GUI deadline reached; QEMU was terminated\n' "$fixture" ;;
    *) exit "$qemu_status" ;;
esac
