#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Prefer a persistent, already-patched visual cache.
# Only when none exists, prove an install and publish a standalone cache.

set -Eeuo pipefail
umask 077
ulimit -c 0

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 8 ]] || vm_die \
    'usage: run-postmarketos-gui.sh REPO_ROOT VM_ROOT LOCK_FILE MATRIX_FILE FIXTURE SART_BIN QEMU QEMU_IMG'
repo_root=$1
vm_root=$2
lock_file=$3
matrix_file=$4
fixture=$5
sart_bin=$6
configured_host_qemu=$7
configured_qemu_img=$8

case "$fixture" in
    postmarketos-qemu-aarch64)
        pair=mkinitfs-boot-deploy-openrc
        image_id=postmarketos-qemu-aarch64-derived
        oracle=SART_VM_MKINITFS_BOOT_DEPLOY_OPENRC_INSTALL_PASS_V1
        install_target=vm-test-install-mkinitfs-boot-deploy-openrc
        verify_target=vm-verify-postmarketos-qemu-aarch64
        ;;
    postmarketos-qemu-aarch64-systemd)
        pair=mkinitfs-boot-deploy-systemd
        image_id=postmarketos-qemu-aarch64-systemd-derived
        oracle=SART_VM_MKINITFS_BOOT_DEPLOY_SYSTEMD_INSTALL_PASS_V1
        install_target=vm-test-install-mkinitfs-boot-deploy-systemd
        verify_target=vm-verify-postmarketos-qemu-aarch64-systemd
        ;;
    *) vm_die "unsupported postmarketOS GUI fixture: $fixture" ;;
esac

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
[[ -f "$matrix_file" && ! -L "$matrix_file" ]] || vm_die 'adapter matrix is missing or unsafe'
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'postmarketOS GUI requires the repository artifact lock'

lock_record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r _ lock_status _ _ _ _ _ _ _ _ _ max_run_bytes max_file_bytes \
    _ _ <<< "$lock_record"
[[ "$lock_status" == derived ]] || vm_die 'postmarketOS GUI requires the authenticated derived base'
vm_is_positive_byte_count "$max_run_bytes" || vm_die 'postmarketOS GUI run-byte limit is invalid'
vm_is_positive_byte_count "$max_file_bytes" || vm_die 'postmarketOS GUI file-byte limit is invalid'
vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"

qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
gui_cache_root="$vm_root/cache/gui"
gui_cache="$gui_cache_root/$fixture"
cache_disk="$gui_cache/disk.qcow2"
cache_vars="$gui_cache/edk2-arm-vars.fd"
cache_manifest="$gui_cache/manifest"
cache_ready=0
if [[ -d "$gui_cache" && ! -L "$gui_cache" &&
      "$(vm_stat_uid "$gui_cache")" == "$(id -u)" &&
      "$(vm_stat_mode "$gui_cache")" == 700 &&
      -f "$cache_disk" && ! -L "$cache_disk" && "$(vm_stat_mode "$cache_disk")" == 400 &&
      -f "$cache_vars" && ! -L "$cache_vars" && "$(vm_stat_mode "$cache_vars")" == 400 &&
      -f "$cache_manifest" && ! -L "$cache_manifest" && "$(vm_stat_mode "$cache_manifest")" == 400 &&
      "$(sed -n 's/^schema=//p' "$cache_manifest")" == SART_VM_GUI_CACHE_V1 &&
      "$(sed -n 's/^fixture=//p' "$cache_manifest")" == "$fixture" &&
      "$(sed -n 's/^pair=//p' "$cache_manifest")" == "$pair" &&
      "$(sed -n 's/^image=//p' "$cache_manifest")" == "$image_id" &&
      "$(sed -n 's/^disk_bytes=//p' "$cache_manifest")" == "$(vm_stat_size "$cache_disk")" &&
      "$(sed -n 's/^vars_bytes=//p' "$cache_manifest")" == "$(vm_stat_size "$cache_vars")" ]]; then
    cache_info="$("$qemu_img" info --output=json -- "$cache_disk")" ||
        vm_die 'cannot inspect cached postmarketOS GUI disk'
    jq -e '.format == "qcow2" and (has("backing-filename") | not)' \
        <<< "$cache_info" >/dev/null || vm_die 'cached postmarketOS GUI disk is not standalone qcow2'
    QEMU_IMG="$qemu_img" vm_assert_qcow2_virtual_size "$cache_disk" 8589934592 8589934592 >/dev/null
    cache_ready=1
    printf '%s\n' 'sart-vm: found persistent patched postmarketOS GUI cache; skipping build, matrix scan, and install'
fi

expected_result="SART_VM_LANE_STATUS_V3|fixture=$fixture|pair=$pair|lane=install|status=PASS|image=$image_id|oracle=$oracle|reason=exact-serial-oracle"
install_run=
if [[ $cache_ready -eq 0 ]]; then
    for candidate in "$vm_root"/runs/run.*; do
        [[ -d "$candidate" && ! -L "$candidate" ]] || continue
        vm_validate_run "$vm_root" "$candidate" >/dev/null 2>&1 || continue
        [[ -f "$candidate/lane.result" && ! -L "$candidate/lane.result" &&
           "$(cat -- "$candidate/lane.result")" == "$expected_result" ]] || continue
        [[ -f "$candidate/lane.meta" && ! -L "$candidate/lane.meta" ]] || continue
        [[ "$(sed -n 's/^fixture=//p' "$candidate/lane.meta")" == "$fixture" &&
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
    if [[ -n "$install_log" && "$install_log" == "$vm_root"/.postmarketos-gui-install.* &&
          -f "$install_log" && ! -L "$install_log" &&
          "$(vm_stat_uid "$install_log")" == "$(id -u)" ]]; then
        rm -f -- "$install_log" || true
    fi
    if [[ -n "$cache_stage" &&
          "$cache_stage" == "$gui_cache_root"/."$fixture".* &&
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
    source_overlay=$cache_disk
    source_vars=$cache_vars
elif [[ -n "$install_run" ]]; then
    printf 'sart-vm: caching existing authenticated postmarketOS install: %s\n' \
        "$install_run"
else
    if ! sart_physical="$(readlink -f -- "$sart_bin" 2>/dev/null)" ||
       [[ ! -f "$sart_physical" ]]; then
        printf '%s\n' 'sart-vm: no patched cache or ARM64 ELF exists; building once'
        make --no-print-directory -C "$repo_root" vm-artifact-aarch64
        sart_physical="$(readlink -f -- "$sart_bin")" ||
            vm_die 'cannot resolve newly built ARM64 Sart ELF'
    fi
    case "$sart_physical" in
        "$vm_root/cache/artifacts/aarch64/generations/"*/sart) ;;
        *) vm_die 'postmarketOS GUI accepts only the immutable ARM64 artifact' ;;
    esac
    [[ -f "$sart_physical" && ! -L "$sart_physical" ]] ||
        vm_die 'ARM64 Sart ELF is unsafe'
    vm_assert_owned "$sart_physical"
    READELF="$(command -v readelf)" bash "$repo_root/scripts/artifact-inspect.sh" \
        aarch64 "$sart_physical"
    printf '%s\n' 'sart-vm: no matching postmarketOS install exists; running the one-time headless install proof'
    printf '%s\n' 'sart-vm: ARM64 TCG can take many minutes; after this, the persistent GUI cache opens directly'
    make --no-print-directory -C "$repo_root" "$verify_target"
    install_log="$(mktemp "$vm_root/.postmarketos-gui-install.XXXXXXXXXX")" ||
        vm_die 'cannot allocate private postmarketOS install transcript'
    chmod 0600 -- "$install_log"
    set +e
    SART_BIN="$sart_physical" QEMU="$configured_host_qemu" QEMU_IMG="$configured_qemu_img" \
        make --no-print-directory -C "$repo_root/scripts/vm" \
        "$install_target" 2>&1 | tee "$install_log"
    install_status=${PIPESTATUS[0]}
    set -e
    [[ $install_status -eq 0 ]] || vm_die 'postmarketOS install lane failed before GUI boot'
    mapfile -t install_runs < <(
        sed -n 's/^sart-vm: unpromoted adapter evidence retained: //p' "$install_log"
    )
    [[ ${#install_runs[@]} -eq 1 ]] ||
        vm_die 'postmarketOS install lane did not identify exactly one evidence run'
    install_run=${install_runs[0]}
    vm_validate_run "$vm_root" "$install_run"
fi

if [[ $cache_ready -eq 0 ]]; then
    [[ -f "$install_run/lane.result" && ! -L "$install_run/lane.result" &&
       "$(cat -- "$install_run/lane.result")" == "$expected_result" ]] ||
        vm_die 'postmarketOS GUI source lacks the exact authenticated install result'
    source_overlay="$install_run/overlay.qcow2"
    source_vars="$install_run/edk2-arm-vars.fd"
    for source in "$source_overlay" "$source_vars"; do
        [[ -f "$source" && ! -L "$source" && "$(vm_stat_mode "$source")" == 600 ]] ||
            vm_die "postmarketOS GUI source artifact is missing or unsafe: $source"
        vm_assert_owned "$source"
    done

    if [[ -e "$gui_cache" || -L "$gui_cache" ]]; then
        [[ -d "$gui_cache" && ! -L "$gui_cache" &&
           "$(vm_stat_uid "$gui_cache")" == "$(id -u)" ]] ||
            vm_die 'refusing to replace unsafe postmarketOS GUI cache'
        vm_assert_no_mount_below "$gui_cache"
        find "$gui_cache" -xdev -depth -mindepth 1 -delete
        rmdir -- "$gui_cache"
    fi
    if [[ ! -e "$gui_cache_root" ]]; then
        mkdir -- "$gui_cache_root"
        chmod 0700 -- "$gui_cache_root"
    fi
    vm_assert_private_dir "$gui_cache_root"
    cache_stage="$(mktemp -d "$gui_cache_root/.$fixture.XXXXXXXXXX")" ||
        vm_die 'cannot allocate postmarketOS GUI cache stage'
    chmod 0700 -- "$cache_stage"
    printf '%s\n' 'sart-vm: publishing standalone patched postmarketOS GUI cache (one time)'
    vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'postmarketOS cache QEMU_IMG'
    "$qemu_img" convert -O qcow2 "$source_overlay" "$cache_stage/disk.qcow2"
    cp -- "$source_vars" "$cache_stage/edk2-arm-vars.fd"
    sart_digest="$(sed -n 's/^sart_sha256=//p' "$install_run/lane.meta")"
    [[ "$sart_digest" =~ ^[0-9a-f]{64}$ ]] || sart_digest=legacy-authenticated-install
    printf 'schema=SART_VM_GUI_CACHE_V1\nfixture=%s\npair=%s\nimage=%s\nsart_sha256=%s\ndisk_bytes=%s\nvars_bytes=%s\n' \
        "$fixture" "$pair" "$image_id" "$sart_digest" \
        "$(vm_stat_size "$cache_stage/disk.qcow2")" \
        "$(vm_stat_size "$cache_stage/edk2-arm-vars.fd")" > "$cache_stage/manifest"
    chmod 0400 -- "$cache_stage/disk.qcow2" "$cache_stage/edk2-arm-vars.fd" \
        "$cache_stage/manifest"
    mv -T -- "$cache_stage" "$gui_cache"
    cache_stage=
    source_overlay=$cache_disk
    source_vars=$cache_vars
fi

for source in "$source_overlay" "$source_vars"; do
    [[ -f "$source" && ! -L "$source" ]] ||
        vm_die "postmarketOS GUI cache artifact is missing or unsafe: $source"
    vm_assert_owned "$source"
done
source_overlay_identity="$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_overlay")"
source_vars_identity="$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_vars")"

gui_run="$(vm_create_run "$vm_root")"
gui_overlay="$gui_run/postmarketos-gui.qcow2"
gui_vars="$gui_run/edk2-arm-vars.fd"
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" 'postmarketOS GUI QEMU_IMG'
"$qemu_img" create -f qcow2 -F qcow2 -b "$source_overlay" "$gui_overlay" >/dev/null
chmod 0600 -- "$gui_overlay"
cp -- "$source_vars" "$gui_vars"
chmod 0600 -- "$gui_vars"
QEMU_IMG="$qemu_img" vm_assert_qcow2_backing_file "$gui_overlay" "$source_overlay"
vm_assert_file_size_at_most "$gui_overlay" "$max_file_bytes" 'postmarketOS GUI overlay'
vm_assert_file_size_at_most "$gui_vars" "$max_file_bytes" 'postmarketOS GUI firmware state'
vm_assert_run_bytes_at_most "$vm_root" "$gui_run" "$max_run_bytes"

host_qemu="$(vm_resolve_qemu "$configured_host_qemu")"
firmware_prefix=${host_qemu%/bin/qemu-system-x86_64}
[[ "$firmware_prefix" != "$host_qemu" ]] || vm_die 'configured QEMU package layout is unexpected'
uefi_code="$firmware_prefix/share/qemu/edk2-aarch64-code.fd"
[[ -f "$uefi_code" && ! -L "$uefi_code" && "$(vm_stat_size "$uefi_code")" == 67108864 ]] ||
    vm_die 'reviewed ARM64 UEFI code is unavailable'
uefi_code="$(readlink -f -- "$uefi_code")"

qemu_supports_display() {
    local executable=$1 backend=$2
    "$executable" -display help 2>&1 | grep -Fx -- "$backend" >/dev/null
}

declare -a qemu_candidates=()
package_qemu_aarch64="$firmware_prefix/bin/qemu-system-aarch64"
[[ -x "$package_qemu_aarch64" ]] && qemu_candidates+=("$package_qemu_aarch64")
[[ -x /usr/bin/qemu-system-aarch64 ]] && qemu_candidates+=(/usr/bin/qemu-system-aarch64)
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
[[ -n "$qemu" ]] || vm_die 'no window-capable qemu-system-aarch64 with GTK or SDL is available'
qemu_identity="$(vm_executable_identity "$qemu")"
vm_assert_executable_identity "$qemu" "$qemu_identity" 'postmarketOS GUI QEMU'

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

printf 'sart-vm: launching installed postmarketOS ARM64 GUI: %s\n' "$gui_run"
printf '%s\n' 'sart-vm: click the window and type 112358 in the centered Sart prompt'
printf '%s\n' 'sart-vm: close the window after login is visible; hard deadline is 15 minutes'

set +e
timeout --signal=TERM --kill-after=5s 900s \
    bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" "$qemu" \
    -name "sart-$fixture-gui" \
    -nodefaults -no-user-config \
    -machine virt,accel=tcg -cpu max -smp 2 -m 4096M \
    -display "$display_backend" \
    -device virtio-gpu-pci,id=video \
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 \
    -serial null -monitor none -nic none -no-reboot -boot c,strict=on \
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$uefi_code" \
    -drive "if=pflash,format=raw,unit=1,file=$gui_vars" \
    -drive "file=$gui_overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=allow,resourcecontrol=deny
qemu_status=$?
set -e

vm_assert_executable_identity "$qemu" "$qemu_identity" 'postmarketOS GUI QEMU'
vm_assert_file_size_at_most "$gui_overlay" "$max_file_bytes" 'postmarketOS GUI overlay after boot'
vm_assert_file_size_at_most "$gui_vars" "$max_file_bytes" 'postmarketOS GUI firmware after boot'
vm_assert_run_bytes_at_most "$vm_root" "$gui_run" "$max_run_bytes"
[[ "$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_overlay")" == "$source_overlay_identity" ]] ||
    vm_die 'postmarketOS GUI changed the authenticated installed source overlay'
[[ "$(stat -Lc '%d:%i:%s:%Y:%a' -- "$source_vars")" == "$source_vars_identity" ]] ||
    vm_die 'postmarketOS GUI changed the authenticated installed firmware state'

case "$qemu_status" in
    0) ;;
    124|137) printf '%s\n' 'sart-vm: postmarketOS GUI deadline reached; QEMU was terminated' ;;
    *) exit "$qemu_status" ;;
esac
