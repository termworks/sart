local project_root = path.normalize(path.absolute(path.join(os.scriptdir(), "../..")))
local artifact_root = path.join(project_root, "target/artifacts")
local process
local fail

local function ensure(condition, message)
    if not condition then
        fail(message)
    end
end

local function execv(program, arguments, options)
    process.execv(program, arguments, options)
end

local function in_project(action)
    action()
end

local function register(name, description, action)
    task(name)
        set_category("sart")
        set_menu({usage = "xmake " .. name, description = description, options = {}})
        on_run(function()
            process = os
            fail = raise
            in_project(action)
        end)
    task_end()
end

local function architecture()
    local value = os.arch()
    ensure(value == "x86_64" or value == "aarch64", "static artifacts require x86_64 or aarch64")
    return value
end

local function task_environment()
    return {
        SART_ROOT = project_root,
        STATIC_ROOT = artifact_root,
        STATIC_ARCH = architecture(),
        NIX_NETWORK_MODE = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline",
    }
end

local function run_locked(task_name)
    execv("bash", {"scripts/artifact-lock.sh", project_root, "xmake", task_name},
          {envs = task_environment()})
end

local function run_bash(script)
    execv("bash", {"-c", script}, {envs = task_environment()})
end

register("static-build", "Publish one immutable static-musl generation", function()
    execv("xmake", {"phase0-safety"})
    execv("xmake", {"nix-check"})
    run_locked("_static-build-locked")
end)

register("_static-build-locked", "Build a static generation while holding the artifact lock", function()
    run_bash([=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
umask 077
root=$STATIC_ROOT
generations=$root/generations
case "$root" in "$SART_ROOT"/target/*) ;; *) echo "ERROR: unsafe static root: $root" >&2; exit 1 ;; esac
test ! -L "$SART_ROOT/target" || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }
mkdir -p "$root"
test ! -L "$root" || { echo 'ERROR: static root must not be a symlink' >&2; exit 1; }
stage= outputs= pointer_stage= generation_pending=
cleanup() {
    if test -n "$stage"; then
        case "$stage" in "$root"/.stage.*) chmod -R u+w -- "$stage" 2>/dev/null || true; rm -rf -- "$stage" ;; esac
    fi
    if test -n "$generation_pending"; then
        case "$generation_pending" in "$generations"/generation.*)
            chmod -R u+w -- "$generation_pending" 2>/dev/null || true
            rm -rf -- "$generation_pending"
        ;; esac
    fi
    if test -n "$outputs"; then case "$outputs" in "$root"/.nix-outputs.*) rm -f -- "$outputs" ;; esac; fi
    if test -n "$pointer_stage"; then case "$pointer_stage" in "$root"/.pointer.*) rm -rf -- "$pointer_stage" ;; esac; fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
if test -e "$generations" || test -L "$generations"; then
    test -d "$generations" && test ! -L "$generations" || {
        echo "ERROR: generations directory is unsafe: $generations" >&2
        exit 1
    }
else
    mkdir -m 0700 -- "$generations"
fi
stage=$(mktemp -d "$root/.stage.XXXXXX")
outputs=$(mktemp "$root/.nix-outputs.XXXXXX")
mkdir -p "$stage/release" "$stage/real-root/usr/bin" "$stage/initramfs/usr/bin"
bash scripts/nix-source-command.sh "$SART_ROOT" "$NIX_NETWORK_MODE" build nix sart-static >"$outputs"
mapfile -t nix_outputs <"$outputs"
test "${#nix_outputs[@]}" -eq 1 || {
    echo "ERROR: expected one Nix output, found ${#nix_outputs[@]}" >&2
    exit 1
}
source_elf=${nix_outputs[0]}/bin/sart
test -f "$source_elf" && test -x "$source_elf" || {
    echo "ERROR: Nix output has no executable bin/sart: ${nix_outputs[0]}" >&2
    exit 1
}
install -m 0755 -- "$source_elf" "$stage/release/sart"
install -m 0755 -- "$source_elf" "$stage/real-root/usr/bin/sart"
install -m 0755 -- "$source_elf" "$stage/initramfs/usr/bin/sart"
READELF=$(command -v readelf) bash scripts/artifact-gate.sh "$STATIC_ARCH" \
    "$stage/release" "$stage/real-root/usr/bin/sart" "$stage/initramfs/usr/bin/sart"
printf '%s\n' "${nix_outputs[0]}" >"$stage/nix-output-path"
chmod -R a-w -- "$stage"
chmod u+w -- "$stage"
generation_name=generation.${stage##*.}
generation=$generations/$generation_name
test ! -e "$generation" && test ! -L "$generation" || {
    echo "ERROR: refusing to replace immutable generation: $generation" >&2
    exit 1
}
mv -T -- "$stage" "$generation"
stage=
generation_pending=$generation
chmod a-w -- "$generation"
generation_pending=
pointer_stage=$(mktemp -d "$root/.pointer.XXXXXX")
ln -s -- "generations/$generation_name" "$pointer_stage/current"
if test -e "$root/current" || test -L "$root/current"; then
    test -L "$root/current" || { echo 'ERROR: current artifact pointer must be a symlink' >&2; exit 1; }
fi
mv -T -- "$pointer_stage/current" "$root/current"
rmdir -- "$pointer_stage"
pointer_stage=
printf 'PASS: published immutable static generation %s\n' "$generation_name"
]=])
end)

register("artifact-check", "Inspect the current immutable static generation", function()
    execv("xmake", {"phase0-safety"})
    run_locked("_artifact-check-locked")
end)

register("_artifact-check-locked", "Inspect a generation while holding the artifact lock", function()
    run_bash([=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
root=$STATIC_ROOT
case "$root" in "$SART_ROOT"/target/*) ;; *) echo "ERROR: unsafe static root: $root" >&2; exit 1 ;; esac
test ! -L "$SART_ROOT/target" || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }
generation=$(bash scripts/artifact-generation.sh "$root")
READELF=$(command -v readelf) bash scripts/artifact-gate.sh "$STATIC_ARCH" \
    "$generation/release" "$generation/real-root/usr/bin/sart" "$generation/initramfs/usr/bin/sart"
]=])
end)

register("release-package", "Create a deterministic single-ELF archive", function()
    execv("xmake", {"phase0-safety"})
    execv("xmake", {"nix-check"})
    run_locked("_release-package-locked")
end)

register("_release-package-locked", "Package a generation while holding the artifact lock", function()
    execv("xmake", {"_static-build-locked"}, {envs = task_environment()})
    execv("xmake", {"_artifact-check-locked"}, {envs = task_environment()})
    run_bash([=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
umask 077
root=$STATIC_ROOT
generation=$(bash scripts/artifact-generation.sh "$root")
package_dir=$root/packages
case "$package_dir" in "$SART_ROOT"/target/artifacts/*) ;; *) echo "ERROR: unsafe package directory" >&2; exit 1 ;; esac
test ! -L "$package_dir" || { echo 'ERROR: package directory must not be a symlink' >&2; exit 1; }
mkdir -p "$package_dir"
archive=$package_dir/sart-linux-$STATIC_ARCH.tar.gz
checksum=$archive.sha256
manifest=$package_dir/sart-linux-$STATIC_ARCH.manifest
for output in "$archive" "$checksum" "$manifest"; do
    test ! -L "$output" || { echo "ERROR: refusing symlinked package output: $output" >&2; exit 1; }
done
temporary=$(mktemp "$package_dir/.sart.XXXXXX.tar.gz")
checksum_temporary=$(mktemp "$package_dir/.sart.XXXXXX.sha256")
manifest_temporary=$(mktemp "$package_dir/.sart.XXXXXX.manifest")
cleanup() { rm -f -- "${temporary:-}" "${checksum_temporary:-}" "${manifest_temporary:-}"; }
trap cleanup EXIT
tar --format=ustar --owner=0 --group=0 --numeric-owner --mode=0755 \
    --mtime='UTC 1970-01-01' -czf "$temporary" -C "$generation/release" sart
test "$(tar -tzf "$temporary")" = sart || { echo 'ERROR: archive must contain only sart' >&2; exit 1; }
elf_sha=$(sha256sum -- "$generation/release/sart"); elf_sha=${elf_sha%%[[:space:]]*}
archive_sha=$(sha256sum -- "$temporary"); archive_sha=${archive_sha%%[[:space:]]*}
generation_name=${generation##*/}
printf '%s  %s\n' "$archive_sha" "${archive##*/}" >"$checksum_temporary"
printf '%s\n' \
    'SART_RELEASE_PACKAGE_V1' \
    "arch=$STATIC_ARCH" \
    "generation=$generation_name" \
    "elf_sha256=$elf_sha" \
    "archive=${archive##*/}" \
    "archive_sha256=$archive_sha" >"$manifest_temporary"
chmod 0400 -- "$temporary" "$checksum_temporary" "$manifest_temporary"
mv -T -- "$temporary" "$archive"; temporary=
mv -T -- "$checksum_temporary" "$checksum"; checksum_temporary=
mv -T -- "$manifest_temporary" "$manifest"; manifest_temporary=
committed_generation=$(bash scripts/release-package-generation.sh "$SART_ROOT" "$root" "$STATIC_ARCH")
test "$committed_generation" = "$generation" || {
    echo 'ERROR: package manifest did not commit the generation just built' >&2
    exit 1
}
printf 'PASS: packaged one static sart as %s\n' "${archive##*/}"
]=])
end)

register("release-build", "Alias for static-build", function()
    execv("xmake", {"static-build"})
end)

register("release-readiness", "Run local verification and build the release package", function()
    execv("xmake", {"verify"})
    execv("xmake", {"nix-check"})
    run_locked("_release-readiness-locked")
end)

register("_release-readiness-locked", "Prove the packaged ELF in the Ubuntu release lanes", function()
    execv("xmake", {"_release-package-locked"}, {envs = task_environment()})
    run_bash([=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
generation=$(bash scripts/release-package-generation.sh "$SART_ROOT" "$STATIC_ROOT" "$STATIC_ARCH")
SART_BIN="$generation/release/sart" xmake _vm-test-release-ubuntu-26.04-dracut-systemd-locked
printf '%s\n' 'PASS: source, exact packaged ELF, and Ubuntu production VM gates passed'
]=])
end)

register("release", "Refuse unreviewed publication", function()
    execv("xmake", {"release-readiness"})
    fail("tag and publication mutation remain locked")
end)

register("clean-all", "Remove generated C++ outputs without touching VM state", function()
    run_locked("_clean-locked")
end)

register("_clean-locked", "Clean outputs while holding the artifact lock", function()
    run_bash([=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
test ! -L "$SART_ROOT/target" || { echo 'ERROR: target must not be a symlink' >&2; exit 1; }
generations=$STATIC_ROOT/generations
if test -e "$generations" || test -L "$generations"; then
    test -d "$generations" && test ! -L "$generations" || {
        echo "ERROR: refusing unsafe generations cleanup: $generations" >&2
        exit 1
    }
    chmod -R u+w -- "$generations"
fi
rm -rf -- "$SART_ROOT/target/cpp" "$SART_ROOT/target/xmake"
]=])
end)
