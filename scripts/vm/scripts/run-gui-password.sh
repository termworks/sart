#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Interactive Bootart prompt against a disposable
# LUKS volume stored inside one private qcow2 regular file.

set -Eeuo pipefail
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 7 ]] || vm_die \
    'usage: run-gui-password.sh REPO_ROOT VM_ROOT LOCK_FILE IMAGE_ID BOOTART_BIN QEMU QEMU_IMG'
repo_root=$1
vm_root=$2
lock_file=$3
image_id=$4
bootart_bin=$5
configured_qemu=$6
configured_qemu_img=$7

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'password GUI lifecycle requires the repository artifact lock'

record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r id status _url sha format arch filename kernel initrd \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    max_log_bytes max_evidence_bytes <<<"$record"
[[ "$status" == verified && "$format" == iso && "$arch" == x86_64 ]] ||
    vm_die 'password GUI requires the verified x86_64 lifecycle ISO'
[[ "$kernel" == /* && "$initrd" == /* ]] ||
    vm_die 'password GUI lifecycle ISO has invalid kernel/initramfs members'
[[ "$bootart_bin" == "$repo_root/target/"* ]] ||
    vm_die 'password GUI Bootart input must remain below repository target/'

image="$vm_root/cache/images/$filename"
vm_assert_not_symlink "$image"
[[ -f "$image" ]] || vm_die "verified password GUI image is not cached: $image"
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] ||
    vm_die 'cached password GUI image must have mode 0400'
vm_assert_file_size_exact "$image" "$download_bytes" 'cached password GUI image'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'cached password GUI image checksum mismatch'

cryptsetup_input="$(command -v -- cryptsetup)" ||
    vm_die 'cryptsetup is required to create the disposable encrypted QEMU file'
cryptsetup="$(readlink -f -- "$cryptsetup_input")" ||
    vm_die 'cannot resolve cryptsetup'
[[ -f "$cryptsetup" && -x "$cryptsetup" && ! -L "$cryptsetup" && ! -w "$cryptsetup" ]] ||
    vm_die 'cryptsetup must resolve to a canonical read-only executable'
case "$cryptsetup" in
    /nix/store/*|/usr/sbin/cryptsetup|/usr/bin/cryptsetup) ;;
    *) vm_die "cryptsetup resolved outside a trusted system prefix: $cryptsetup" ;;
esac

qemu_img="$(vm_resolve_qemu_img "$configured_qemu_img")"
qemu_img_identity="$(vm_executable_identity "$qemu_img")"
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" \
    'password GUI QEMU_IMG executable'

# This is a public, fixed fixture credential for a disposable test volume, not
# a user secret. It is kept out of QEMU argv/environment and is never retained
# in the generated image or logs. The user still types it into Bootart so the
# interactive prompt path remains under visual test.
test_passphrase=112358

vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"
run_dir="$(vm_create_run "$vm_root")"
raw="$run_dir/.encrypted-drive.raw"
drive="$run_dir/encrypted-drive.qcow2"
serial="$run_dir/password-serial.log"
truncate -s 67108864 -- "$raw"
chmod 0600 -- "$raw"
[[ -f "$raw" && ! -L "$raw" && "$(vm_stat_uid "$raw")" == "$(id -u)" ]] ||
    vm_die 'disposable raw LUKS staging file is unsafe'

# This formats only the validated regular file above. There is deliberately no
# loop device, NBD device, host mapping, filesystem mount, or /dev path here.
printf '%s' "$test_passphrase" | "$cryptsetup" luksFormat --batch-mode --type luks2 \
    --pbkdf pbkdf2 --iter-time 100 --key-file - "$raw" >/dev/null 2>&1 ||
    vm_die 'cannot format the disposable regular-file LUKS volume'
unset test_passphrase
"$cryptsetup" isLuks "$raw" >/dev/null 2>&1 ||
    vm_die 'disposable regular-file LUKS header verification failed'

vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" \
    'password GUI QEMU_IMG executable before conversion'
"$qemu_img" convert -f raw -O qcow2 "$raw" "$drive" ||
    vm_die 'cannot wrap disposable LUKS bytes in qcow2'
vm_assert_executable_identity "$qemu_img" "$qemu_img_identity" \
    'password GUI QEMU_IMG executable after conversion'
rm -f -- "$raw"
[[ -f "$drive" && ! -L "$drive" ]] || vm_die 'encrypted qcow2 output is unsafe'
vm_assert_owned "$drive"
chmod 0600 -- "$drive"
vm_assert_qcow2_virtual_size "$drive" 67108864

bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    bash "$SCRIPT_DIR/prepare-smoke.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$image" \
    "$kernel" "$initrd" "$bootart_bin" >/dev/null
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    bash "$SCRIPT_DIR/prepare-password-smoke.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$image" "$cryptsetup" >/dev/null
for prepared in kernel base-initramfs initramfs.cpio.gz encrypted-drive.qcow2; do
    vm_assert_file_size_at_most "$run_dir/$prepared" "$max_file_bytes" \
        "prepared password GUI artifact $prepared"
done
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - ||
    vm_die 'immutable lifecycle ISO changed during password guest preparation'

if [[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" &&
      -d "$XDG_RUNTIME_DIR" ]]; then
    case "$WAYLAND_DISPLAY" in
        /*) wayland_socket=$WAYLAND_DISPLAY ;;
        *) wayland_socket="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ;;
    esac
else
    wayland_socket=
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
    vm_die 'no live graphical session found for password GUI'
fi

qemu_supports_display() {
    local executable=$1 backend=$2
    "$executable" -display help 2>&1 | grep -Fx -- "$backend" >/dev/null
}
configured_qemu_physical="$(vm_resolve_qemu "$configured_qemu")"
declare -a qemu_candidates=("$configured_qemu_physical")
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
    fi
    if qemu_supports_display "$candidate" sdl; then
        qemu=$candidate
        display_backend=sdl
        break
    fi
done
[[ -n "$qemu" ]] || vm_die 'no window-capable QEMU found for password GUI'
qemu_identity="$(vm_executable_identity "$qemu")"
vm_assert_executable_identity "$qemu" "$qemu_identity" 'password GUI QEMU executable'

: >"$serial"
chmod 0600 -- "$serial"
printf 'bootart-vm: encrypted qcow2 guest: %s\n' "$run_dir"
printf '%s\n' 'bootart-vm: click the QEMU window and type 112358 when Bootart asks'

set +e
timeout --signal=TERM --kill-after=2s 180s "$qemu" \
    -name bootart-gui-password \
    -nodefaults \
    -no-user-config \
    -machine q35,accel=tcg \
    -display "$display_backend" \
    -vga std \
    -m 512M \
    -smp 1 \
    -cpu max \
    -serial "file:$serial" \
    -monitor none \
    -nic none \
    -no-reboot \
    -drive "file=$drive,format=qcow2,if=none,id=encrypted,cache=none,aio=threads" \
    -device virtio-blk-pci,drive=encrypted \
    -kernel "$run_dir/kernel" \
    -initrd "$run_dir/initramfs.cpio.gz" \
    -append 'console=tty0 rdinit=/init panic=-1 quiet bootart.vm.gui-password=1' \
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=allow,resourcecontrol=deny
qemu_status=$?
set -e
[[ "$qemu_status" -eq 0 ]] ||
    vm_die "password GUI QEMU failed or timed out with status $qemu_status"
password_prompt_count="$(grep -Fxc 'BOOTART_VM_GUI_PASSWORD_PROMPT_V1' "$serial" || true)"
if [[ "$(grep -Fxc 'BOOTART_VM_GUI_PASSWORD_PASS_V1' "$serial" || true)" -ne 1 ]]; then
    if grep -Fxq 'BOOTART_VM_LIFECYCLE_FAIL_V1:encrypted-qemu-drive-unlock' "$serial"; then
        vm_die "password GUI guest rejected $password_prompt_count submitted passphrase attempt(s); type exactly 112358 using the QEMU window; inspect $serial"
    fi
    vm_die "password GUI did not complete the disposable qcow2 unlock; inspect $serial"
fi
! grep -Fq 'BOOTART_VM_LIFECYCLE_FAIL_V1' "$serial" ||
    vm_die "password GUI guest reported failure; inspect $serial"
vm_assert_file_size_at_most "$serial" "$max_log_bytes" 'password GUI serial log'
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
printf 'bootart-vm: encrypted qcow2 password preview PASS: %s\n' "$run_dir"
