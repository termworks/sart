#!/usr/bin/env bash
# Pure process-group fixture. It launches only shell/sleep children, never QEMU.

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: check-timeout-containment.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=$1
[[ "$repo_root" == /* && -d "$repo_root" && ! -L "$repo_root" ]] || {
    printf 'bootart-vm: invalid repository root for timeout fixture\n' >&2
    exit 2
}

foreground_option='--fore''ground'
if grep -R -n -E -- "timeout[^#]*${foreground_option}" \
    "$repo_root/vm/Makefile" "$repo_root/vm/scripts" >/dev/null; then
    printf 'bootart-vm: foreground timeout would not contain descendant processes\n' >&2
    exit 1
fi

# The process-group behavior below is useful only if every host entry recipe
# actually retains the outer timeout. Pin the four lifecycle/adapter recipe
# shapes instead of merely checking the timeout utility in isolation.
awk '
    /^vm-test-lifecycle-alpine:/ { current = "lifecycle"; next }
    /^[$][(]ADAPTER_LIFECYCLE_TARGETS[)]:/ { current = "adapter-lifecycle"; next }
    /^[$][(]ADAPTER_INSTALL_TARGETS[)]:/ { current = "adapter-install"; next }
    /^[$][(]ADAPTER_PASSWORD_TARGETS[)]:/ { current = "adapter-password"; next }
    current != "" && /timeout --signal=TERM --kill-after=10s/ {
        seen[current] = 1
        current = ""
    }
    END {
        if (!seen["lifecycle"] || !seen["adapter-lifecycle"] ||
            !seen["adapter-install"] || !seen["adapter-password"]) exit 1
    }
' "$repo_root/vm/Makefile" || {
    printf 'bootart-vm: one or more VM host entry recipes lost process-group timeout containment\n' >&2
    exit 1
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bootart-timeout-containment.XXXXXXXXXX")"
record="$tmp/descendant"
worker="$tmp/worker.sh"
descendant_pid=
descendant_start=

same_live_process() {
    local pid=$1 expected_start=$2 stat_line rest
    local -a fields
    [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/stat" ]] || return 1
    stat_line="$(cat -- "/proc/$pid/stat")" || return 1
    rest="${stat_line##*) }"
    read -r -a fields <<<"$rest"
    [[ ${#fields[@]} -ge 20 && "${fields[19]}" == "$expected_start" && \
       "${fields[0]}" != Z ]]
}

cleanup() {
    if [[ -n "${descendant_pid:-}" && -n "${descendant_start:-}" ]] &&
       same_live_process "$descendant_pid" "$descendant_start"; then
        kill -KILL "$descendant_pid" 2>/dev/null || true
    fi
    case "$tmp" in
        "${TMPDIR:-/tmp}"/bootart-timeout-containment.*) rm -rf -- "$tmp" ;;
        *) printf 'bootart-vm: refusing unsafe timeout-fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

cat >"$worker" <<'EOF'
#!/usr/bin/env bash
set -eu
record=$1
# Keep both the monitored shell and its descendant alive after TERM. GNU
# timeout must deliver its later KILL to the entire child process group.
trap '' TERM
(
    trap '' TERM
    stat_line="$(cat -- "/proc/$BASHPID/stat")"
    rest="${stat_line##*) }"
    read -r -a fields <<<"$rest"
    printf '%s %s\n' "$BASHPID" "${fields[19]}" >"$record"
    while :; do sleep 30; done
) &
wait "$!"
EOF
chmod 0700 -- "$worker"

set +e
timeout --signal=TERM --kill-after=0.2s 2s bash "$worker" "$record" \
    >/dev/null 2>&1
timeout_status=$?
set -e
case "$timeout_status" in
    124|137) ;;
    *)
        printf 'bootart-vm: containment fixture returned unexpected status %s\n' \
            "$timeout_status" >&2
        exit 1
        ;;
esac

[[ -f "$record" && ! -L "$record" ]] || {
    printf 'bootart-vm: containment fixture did not publish descendant identity\n' >&2
    exit 1
}
read -r descendant_pid descendant_start <"$record"
[[ "$descendant_pid" =~ ^[1-9][0-9]*$ && "$descendant_start" =~ ^[1-9][0-9]*$ ]] || {
    printf 'bootart-vm: invalid descendant identity in containment fixture\n' >&2
    exit 1
}

for _ in 1 2 3 4 5 6 7 8 9 10; do
    same_live_process "$descendant_pid" "$descendant_start" || break
    sleep 0.05
done
if same_live_process "$descendant_pid" "$descendant_start"; then
    printf 'bootart-vm: timeout left a live descendant outside containment\n' >&2
    exit 1
fi

printf 'bootart-vm: process-group timeout containment PASS\n'
