#!/usr/bin/env bash

set -Eeuo pipefail
umask 077

[[ $# -eq 1 ]] || {
    printf 'usage: host-safety-policy-tests.sh REPOSITORY_ROOT\n' >&2
    exit 2
}
repo_root=$1
policy="$repo_root/scripts/host-safety-policy.sh"
[[ -f "$policy" && ! -L "$policy" ]] || {
    printf 'host safety policy is missing or symlinked\n' >&2
    exit 2
}

tmp_parent=${TMPDIR:-/tmp}
tmp="$(mktemp -d "$tmp_parent/bootart-host-policy-tests.XXXXXXXXXX")"
cleanup() {
    case "$tmp" in
        "$tmp_parent"/bootart-host-policy-tests.*) rm -rf -- "$tmp" ;;
        *) printf 'refusing unsafe fixture cleanup: %s\n' "$tmp" >&2 ;;
    esac
}
trap cleanup EXIT HUP INT TERM

new_fixture() {
    local name=$1 root="$tmp/$1"
    mkdir -p -- "$root/scripts/vm/scripts" "$root/scripts/vm/runners" \
        "$root/.github/workflows"
    printf 'all:\n\t@true\n' > "$root/Makefile"
    printf 'all:\n\t@true\n' > "$root/scripts/vm/Makefile"
    printf '{}\n' > "$root/flake.nix"
    printf '%s\n' "$root"
}

expect_rejected() {
    local root=$1
    if bash "$policy" "$root" >/dev/null 2>&1; then
        printf 'unsafe fixture unexpectedly passed: %s\n' "$root" >&2
        exit 1
    fi
}

bash "$policy" "$repo_root" >/dev/null

fixture="$(new_fixture privilege)"
printf 'bad:\n\t@%s true\n' "su""do" >> "$fixture/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture nested)"
printf '#!/usr/bin/env bash\n%s true\n' "do""as" > "$fixture/scripts/vm/scripts/bad.sh"
expect_rejected "$fixture"

fixture="$(new_fixture runner-tree)"
mkdir -p -- "$fixture/scripts/vm/runners/example"
printf '#!/usr/bin/env bash\n%s true\n' "do""as" > "$fixture/scripts/vm/runners/example/bad.sh"
expect_rejected "$fixture"

fixture="$(new_fixture extensionless)"
printf '#!/usr/bin/env bash\nenv %s true\n' "su""do" > "$fixture/scripts/no-extension"
expect_rejected "$fixture"

fixture="$(new_fixture assignment-wrapper)"
printf '#!/usr/bin/env bash\nSAFE=1 command /usr/bin/%s true\n' "su""do" > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

fixture="$(new_fixture envrc)"
printf 'export SAFE=1\n/usr/bin/%s true\n' "su""do" > "$fixture/.envrc"
expect_rejected "$fixture"

fixture="$(new_fixture storage)"
printf '#!/usr/bin/env bash\n%s if=/dev/zero of=/dev/%s\n' \
    'd''d' 's''da' > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

for device in disk/by-id/root loop0 dm-0 md0 zvol/pool/root nbd0 rbd0 zd0; do
    fixture="$(new_fixture "device-${device//\//-}")"
    printf '#!/usr/bin/env bash\nprintf "%%s\\n" /dev/%s\n' "$device" > "$fixture/scripts/bad.sh"
    expect_rejected "$fixture"
done

# Mutation destinations are allowlisted by validated variable roots. Literal
# host paths and ambient home expansion must fail even when they are outside
# the historical /boot, /etc, and /usr denylist.
for destination in / /home/bootart-policy-fixture /var/lib/bootart-policy-fixture \
    /opt/bootart-policy-fixture; do
    fixture="$(new_fixture "absolute-destination-${destination//\//-}")"
    printf '#!/usr/bin/env bash\n%s -rf -- %s\n' 'r''m' "$destination" \
        > "$fixture/scripts/bad.sh"
    expect_rejected "$fixture"
done

fixture="$(new_fixture home-variable-destination)"
printf '#!/usr/bin/env bash\n%s -f -- "$%s/bootart-policy-fixture"\n' \
    'r''m' 'HO''ME' > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

fixture="$(new_fixture braced-home-variable-destination)"
printf '#!/usr/bin/env bash\n%s -f -- "${%s}/bootart-policy-fixture"\n' \
    'r''m' 'HO''ME' > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

fixture="$(new_fixture literal-redirection-destination)"
printf '#!/usr/bin/env bash\nprintf unsafe > /%s/bootart-policy-fixture\n' \
    'v''ar' > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

# Private roots that are validated by their owning script remain legitimate.
fixture="$(new_fixture validated-private-destinations)"
cat > "$fixture/scripts/good.sh" <<'EOF'
#!/usr/bin/env bash
rm -rf -- "$repo_root/target/policy-fixture"
rm -rf -- "$vm_root/runs/policy-fixture"
rm -rf -- "$fixture/policy-fixture"
rm -rf -- "$tmp/policy-fixture"
printf safe > "$run_dir/policy-fixture"
EOF
bash "$policy" "$fixture" >/dev/null

fixture="$(new_fixture ambiguous-target)"
printf '%s:\n\t@true\n' 'in''stall' >> "$fixture/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture ambiguous-multi-target)"
printf 'safe %s:\n\t@true\n' 'in''stall' >> "$fixture/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture ambiguous-expanded-target)"
printf 'in$()stall:\n\t@true\n' >> "$fixture/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture symlink)"
ln -s -- /dev/null "$fixture/scripts/linked.sh"
expect_rejected "$fixture"

fixture="$(new_fixture missing-vm-makefile)"
rm -f -- "$fixture/scripts/vm/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture symlinked-vm-makefile)"
rm -f -- "$fixture/scripts/vm/Makefile"
ln -s -- /dev/null "$fixture/scripts/vm/Makefile"
expect_rejected "$fixture"

fixture="$(new_fixture unexpected-vm-surface)"
mkdir -p -- "$fixture/scripts/vm/unreviewed-host-tools"
expect_rejected "$fixture"

# Guest/staging-root mutations are expected host-side test preparation and do
# not target a literal host root.
fixture="$(new_fixture guest-root-destination)"
printf '#!/usr/bin/env bash\ninstall -m 0644 input "$root/etc/example"\n' \
    > "$fixture/scripts/vm/scripts/good.sh"
bash "$policy" "$fixture" >/dev/null

# Guest payloads, VM metadata, and documentation are data rather than
# host-executed command surfaces. The explicit VM Makefile/scripts/runners
# checks above must not expand to these paths merely because vm lives below
# the repository's scripts directory.
fixture="$(new_fixture guest-data-is-not-host-surface)"
mkdir -p -- "$fixture/scripts/vm/guest"
printf '#!/bin/sh\n%s -f\n' 'power''off' > "$fixture/scripts/vm/guest/lifecycle"
printf 'guest may use %s inside the VM\n' 'mou''nt' > "$fixture/scripts/vm/README.md"
printf 'tool=%s\n' 'mkinit''cpio' > "$fixture/scripts/vm/adapter-matrix.lock"
bash "$policy" "$fixture" >/dev/null

for destination in boot etc usr; do
    fixture="$(new_fixture "host-destination-$destination")"
    printf '#!/usr/bin/env bash\ncp -- input /%s/bootart-policy-fixture\n' \
        "$destination" > "$fixture/scripts/bad.sh"
    expect_rejected "$fixture"
done

fixture="$(new_fixture host-redirection)"
printf '#!/usr/bin/env bash\nprintf unsafe > /%s/bootart-policy-fixture\n' \
    e"tc" > "$fixture/scripts/bad.sh"
expect_rejected "$fixture"

printf 'bootart-safety: rejection fixtures PASS\n'
