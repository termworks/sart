#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Launches the test-only Sart initramfs visually.

set -Eeuo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 6 ]] || vm_die \
    'usage: run-gui.sh REPO_ROOT VM_ROOT LOCK_FILE IMAGE_ID SART_BIN QEMU'
repo_root=$1
vm_root=$2
lock_file=$3
image_id=$4
sart_bin=$5
configured_qemu=$6

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_lock "$lock_file"

record="$(vm_lock_record "$lock_file" "$image_id")"
IFS='|' read -r id status _url sha format arch filename kernel initrd \
    download_bytes max_virtual_bytes max_run_bytes max_file_bytes \
    _max_log_bytes _max_evidence_bytes <<< "$record"

[[ "$status" == verified ]] || vm_die "GUI image is not verified: $id"
[[ "$format" == iso && "$arch" == x86_64 ]] || \
    vm_die "GUI requires the verified x86_64 lifecycle ISO: $id"
[[ "$kernel" == /* && "$initrd" == /* ]] || \
    vm_die "GUI lifecycle ISO has invalid kernel/initramfs members: $id"
[[ "$sart_bin" == "$repo_root/target/"* ]] || \
    vm_die 'GUI Sart input must remain below repository target/'
bash "$repo_root/scripts/artifact-lock-assert.sh" "$repo_root" >/dev/null ||
    vm_die 'GUI lifecycle requires the repository artifact lock'

image="$vm_root/cache/images/$filename"
vm_assert_not_symlink "$image"
[[ -f "$image" ]] || vm_die "verified GUI image is not cached: $image"
vm_assert_owned "$image"
[[ "$(vm_stat_mode "$image")" == 400 ]] || \
    vm_die "cached GUI image must have mode 0400: $image"
vm_assert_file_size_exact "$image" "$download_bytes" 'cached GUI image'
vm_assert_file_size_at_most "$image" "$max_virtual_bytes" 'cached GUI ISO'
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - || \
    vm_die "cached GUI image checksum mismatch: $image"

vm_require_free_bytes "$vm_root/runs" "$max_run_bytes"
run_dir="$(vm_create_run "$vm_root")"
bash "$SCRIPT_DIR/run-with-file-limit.sh" "$max_file_bytes" \
    bash "$SCRIPT_DIR/prepare-smoke.sh" \
    "$repo_root" "$vm_root" "$run_dir" "$image" \
    "$kernel" "$initrd" "$sart_bin" >/dev/null
vm_validate_run "$vm_root" "$run_dir"
for prepared in kernel base-initramfs initramfs.cpio.gz; do
    vm_assert_file_size_at_most "$run_dir/$prepared" "$max_file_bytes" \
        "prepared GUI lifecycle $prepared"
done
vm_assert_run_bytes_at_most "$vm_root" "$run_dir" "$max_run_bytes"
printf '%s  %s\n' "$sha" "$image" | sha256sum --check --status - || \
    vm_die 'immutable GUI ISO changed during guest preparation'

# GTK works natively in both Wayland-only and X11 sessions. Tool/approval
# wrappers can preserve a compositor socket name after that particular socket
# has disappeared, even though the session's conventional wayland-0 socket is
# live. Recover only to that unambiguous, same-user socket; never guess among
# arbitrary wayland-* sockets. If X11 is available, discard a stale Wayland
# hint so GTK can use DISPLAY instead.
wayland_socket=
if [[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" && \
      -d "$XDG_RUNTIME_DIR" ]]; then
    case "$WAYLAND_DISPLAY" in
        /*) wayland_socket=$WAYLAND_DISPLAY ;;
        *) wayland_socket="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ;;
    esac
fi
if [[ -n "$wayland_socket" && -S "$wayland_socket" ]]; then
    :
elif [[ -n "${XDG_RUNTIME_DIR:-}" && -d "$XDG_RUNTIME_DIR" && \
        ! -L "$XDG_RUNTIME_DIR/wayland-0" && \
        -S "$XDG_RUNTIME_DIR/wayland-0" && \
        "$(vm_stat_uid "$XDG_RUNTIME_DIR/wayland-0")" == "$(id -u)" ]]; then
    printf 'sart-vm: stale/missing Wayland hint; using %s\n' \
        "$XDG_RUNTIME_DIR/wayland-0"
    export WAYLAND_DISPLAY=wayland-0
elif [[ -n "${DISPLAY:-}" ]]; then
    unset WAYLAND_DISPLAY
else
    vm_die 'no live graphical session found; set WAYLAND_DISPLAY or DISPLAY'
fi

qemu_supports_display() {
    local executable=$1 backend=$2
    "$executable" -display help 2>&1 | grep -Fx -- "$backend" >/dev/null
}

# Automated lanes deliberately use the flake's headless QEMU. The interactive
# target first honors that configured executable when it has a real window
# backend, then tries the flake-provided GUI-only wrapper, and finally the
# conventional host QEMU path. Never treat `dbus` as a window backend: it needs
# a separate display client and was the reason a hard-coded GTK request failed.
configured_qemu_physical="$(vm_resolve_qemu "$configured_qemu")"
declare -a qemu_candidates=("$configured_qemu_physical")
if command -v -- sart-qemu-gui >/dev/null 2>&1; then
    qemu_candidates+=(sart-qemu-gui)
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
[[ -n "$qemu" ]] || vm_die \
    'no window-capable QEMU found; configured QEMU offers only headless backends'
qemu_identity="$(vm_executable_identity "$qemu")"
vm_assert_executable_identity "$qemu" "$qemu_identity" 'GUI QEMU executable'

printf 'sart-vm: launching test-only Sart windowed lifecycle: %s\n' "$run_dir"
printf 'sart-vm: GUI QEMU: %s (%s)\n' "$qemu" "$display_backend"
printf '%s\n' 'sart-vm: preview exits after the animation; close the window or press Ctrl-C to stop early'

# This visual path has no guest disk at all: it boots the same immutable ISO
# kernel plus a private initramfs containing the exact static Sart ELF.
# Networking and host filesystem sharing are explicitly absent. This target is
# a visual lifecycle preview only and never counts as adapter-pair evidence.
# GTK may invoke its system pixbuf loader while creating the window, so this
# interactive-only lane cannot use QEMU's `spawn=deny`; automated lanes retain
# their stricter launch policy.
set +e
timeout --signal=TERM --kill-after=2s 20s "$qemu" \
    -name sart-gui \
    -nodefaults \
    -no-user-config \
    -machine q35,accel=tcg \
    -display "$display_backend" \
    -vga std \
    -m 256M \
    -smp 1 \
    -cpu max \
    -serial null \
    -monitor none \
    -nic none \
    -no-reboot \
    -kernel "$run_dir/kernel" \
    -initrd "$run_dir/initramfs.cpio.gz" \
    -append 'console=tty0 rdinit=/init panic=-1 quiet sart.vm.gui=1' \
    -sandbox on,obsolete=deny,elevateprivileges=deny,spawn=allow,resourcecontrol=deny
qemu_status=$?
set -e
case "$qemu_status" in
    0)
        exit 0
        ;;
    124|137)
        printf '%s\n' 'sart-vm: preview deadline reached; QEMU was terminated'
        exit 0
        ;;
    *)
        exit "$qemu_status"
        ;;
esac
