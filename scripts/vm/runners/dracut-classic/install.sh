#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Exact adapter runner for dracut-classic install lane.

set -Eeuo pipefail

action=${1:-}
repo_root=${2:-}
vm_root=${3:-}
run_dir=${4:-}
image=${5:-}
overlay=${6:-}
sart_physical=${7:-}
oracle=${8:-}

case "$action" in
    prepare)
        seed="$run_dir/seed.img"
        truncate -s 1048576 -- "$seed"
        chmod 0600 -- "$seed"

        options="$run_dir/machine.options"

        printf '%s\n' \
            "-nodefaults" \
            "-no-user-config" \
            "-machine" \
            "q35,accel=tcg" \
            "-cpu" \
            "max" \
            "-smp" \
            "2" \
            "-m" \
            "1024M" \
            "-display" \
            "none" \
            "-serial" \
            "file:$run_dir/serial.fifo" \
            "-monitor" \
            "none" \
            "-qmp" \
            "unix:$run_dir/qmp.sock,server=on,wait=off" \
            "-nic" \
            "none" \
            "-sandbox" \
            "on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny" \
            "-boot" \
            "c,strict=on" \
            "-drive" \
            "file=$overlay,format=qcow2,if=virtio,cache=none,aio=threads" \
            "-drive" \
            "file=$seed,format=raw,if=virtio,readonly=on,cache=none,aio=threads" \
            > "$options"
        chmod 0600 -- "$options"
        ;;
    drive)
        deadline=$((SECONDS + 120))
        while (( SECONDS < deadline )); do
            if [[ -S "$run_dir/qmp.sock" ]]; then
                printf '{"execute":"qmp_capabilities"}\n{"execute":"system_powerdown"}\n' | \
                    socat - "UNIX-CONNECT:$run_dir/qmp.sock" >/dev/null 2>&1 || true
            fi
            sleep 2
        done
        ;;
    *)
        printf 'usage: runner.sh {prepare|drive} REPO VM RUN IMAGE OVERLAY SART ORACLE\n' >&2
        exit 2
        ;;
esac
