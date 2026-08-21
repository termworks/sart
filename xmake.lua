set_project("sart")
set_version("0.1.0")
set_xmakever("2.8.5")

add_rules("mode.debug", "mode.release")
set_languages("c++23")
set_policy("package.requires_lock", true)
set_policy("build.fence", true)
set_warnings("all", "extra", "pedantic", "error")
set_config("builddir", "target/xmake")

option("tests")
    set_default(true)
    set_showmenu(true)
    set_description("Build the doctest suite")
option_end()

option("musl")
    set_default(false)
    set_showmenu(true)
    set_description("Write release artifacts to the musl output directory")
option_end()

local output_mode = has_config("musl") and "musl" or (is_mode("release") and "release" or "debug")
local project_root = os.scriptdir()

local function configure_cpp_target()
    add_includedirs("include", {public = true})
    if has_config("musl") then
        if os.getenv("SART_MUSL_ZLIB") then
            add_linkdirs(path.join(os.getenv("SART_MUSL_ZLIB"), "lib"))
        end
        if os.getenv("SART_MUSL_ZSTD") then
            add_linkdirs(path.join(os.getenv("SART_MUSL_ZSTD"), "lib"))
        end
    end
    on_load(function(target)
        import("core.project.project")
        target:add("defines", 'SART_VERSION="' .. project.version() .. '"')
    end)
    add_cxxflags("-pthread")
    if is_mode("release") then
        set_optimize("smallest")
        add_defines("NDEBUG")
        add_cxxflags("-ffunction-sections", "-fdata-sections", "-fno-ident")
    else
        set_symbols("debug")
        set_optimize("none")
        add_cxxflags("-Og")
    end
end

target("sart-core")
    set_kind("static")
    set_default(false)
    set_filename("libsart.a")
    set_targetdir("target/cpp/" .. output_mode)
    add_files("src/**.cpp")
    remove_files("src/main.cpp")
    configure_cpp_target()
target_end()

target("sart")
    set_kind("binary")
    set_default(true)
    set_targetdir("target/cpp/" .. output_mode)
    add_deps("sart-core")
    add_files("src/main.cpp")
    add_syslinks("pthread", "z", "zstd")
    configure_cpp_target()
    if is_mode("release") then
        add_ldflags("-static", "-Wl,--gc-sections", "-Wl,--build-id=none", "-s", {force = true})
    end
target_end()

if has_config("tests") then
    target("sart-tests")
        set_kind("binary")
        set_default(false)
        set_targetdir("target/cpp/" .. output_mode)
        set_rundir("$(projectdir)")
        add_deps("sart", "sart-core")
        add_files("tests/*.cpp")
        if os.getenv("DOCTEST_INCLUDE_DIR") then
            add_includedirs(os.getenv("DOCTEST_INCLUDE_DIR"), {external = true})
        end
        add_defines('SART_SOURCE_ROOT="$(projectdir)"')
        add_syslinks("pthread", "z", "zstd")
        configure_cpp_target()
        add_tests("doctest", {
            runenvs = {SART_BINARY = project_root .. "/target/cpp/" .. output_mode .. "/sart"},
            timeout = 120,
            realtime_output = true,
        })
    target_end()
end

do
    local root = path.normalize(path.absolute(os.scriptdir()))
    local process
    local fail
    local output
    local color_output

    local function ensure(condition, message)
        if not condition then
            fail(message)
        end
    end

    local function execv(program, arguments, options)
        process.execv(program, arguments, options)
    end

    local function run_xmake(arguments)
        execv("xmake", arguments)
    end

    local function configure(mode, tests, musl)
        run_xmake({"f", "-c", "-y", "-m", mode, "--tests=" .. (tests and "y" or "n"),
                   "--musl=" .. (musl and "y" or "n")})
    end

    local function register(name, description, action)
        task(name)
            set_category("sart")
            set_menu({usage = "xmake " .. name, description = description, options = {}})
            on_run(function()
                process = os
                fail = raise
                output = print
                color_output = cprint
                action()
            end)
        task_end()
    end

    register("cpp-build", "Build the debug Sart executable", function()
        configure("debug", true, false)
        run_xmake({"build", "sart"})
    end)

    register("cpp-test", "Build and run every doctest case", function()
        configure("debug", true, false)
        run_xmake({"test", "-v"})
    end)

    register("cpp-release-build", "Build the release Sart executable", function()
        configure("release", false, false)
        run_xmake({"build", "sart"})
    end)

    register("cpp-musl-build", "Build and inspect the static musl executable", function()
        local compiler = os.getenv("SART_MUSL_CXX")
        local archiver = os.getenv("SART_MUSL_AR")
        local readelf = os.getenv("SART_MUSL_READELF")
        ensure(compiler and os.isfile(compiler), "enter the Nix shell for the musl C++ compiler")
        ensure(archiver and os.isfile(archiver), "enter the Nix shell for musl binutils")
        ensure(readelf and os.isfile(readelf), "enter the Nix shell for musl readelf")
        run_xmake({"f", "-c", "-y", "-m", "release", "--tests=n", "--musl=y", "--cc=" .. compiler,
                   "--cxx=" .. compiler, "--ld=" .. compiler, "--ar=" .. archiver})
        run_xmake({"build", "sart"})
        local architecture = os.arch() == "x86_64" and "x86_64" or os.arch()
        execv("bash", {"scripts/artifact-inspect.sh", architecture, "target/cpp/musl/sart"},
              {envs = {READELF = readelf}})
    end)

    register("cpp-cli-check", "Smoke-test the static production CLI", function()
        run_xmake({"cpp-musl-build"})
        execv(path.join(root, "target/cpp/musl/sart"), {"--version"})
        execv(path.join(root, "target/cpp/musl/sart"), {"install", "--help"})
    end)

    register("cpp-nix-build", "Build the locked static Nix package", function()
        local network = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline"
        execv("bash", {"scripts/nix-source-command.sh", root, network, "build", "nix", "sart-cpp-static"})
    end)

    register("cpp-clean", "Remove C++ and Xmake build outputs", function()
        os.tryrm(path.join(root, "target/cpp"))
        os.tryrm(path.join(root, "target/xmake"))
    end)

    register("compile", "Clean and rebuild the debug executable", function()
        run_xmake({"cpp-clean"})
        run_xmake({"cpp-build"})
    end)

    for _, alias in ipairs({"check", "b"}) do
        register(alias, "Alias for cpp-build", function()
            run_xmake({"cpp-build"})
        end)
    end

    for _, alias in ipairs({"check-all", "test-all", "test-unit", "test-protocol", "test-daemon", "test-display",
                            "test-pty", "test-installer-root", "t"}) do
        register(alias, "Alias for cpp-test", function()
            run_xmake({"cpp-test"})
        end)
    end

    register("fmt", "Format all C++ sources and tests", function()
        local files = table.join(os.files("include/**.hpp"), os.files("src/**.cpp"), os.files("tests/**.cpp"))
        table.sort(files)
        execv("clang-format", table.join({"-i"}, files))
    end)

    register("fmt-check", "Check C++ formatting", function()
        local files = table.join(os.files("include/**.hpp"), os.files("src/**.cpp"), os.files("tests/**.cpp"))
        table.sort(files)
        execv("clang-format", table.join({"--dry-run", "--Werror"}, files))
    end)

    register("nix-check", "Evaluate the locked flake", function()
        local network = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline"
        execv("bash", {"scripts/nix-source-command.sh", root, network, "check", "nix"})
    end)

    register("phase0-safety", "Check source and PID-1 safety invariants", function()
        execv("bash", {"-c", [[
set -eu
if find include src tests -type l -print -quit | grep -q .; then
    echo 'ERROR: symlinks are forbidden below C++ source roots' >&2
    exit 1
fi
forbidden='SART_INIT_STUB|RB_POWER_OFF|RB_HALT_SYSTEM|RB_AUTOBOOT|LINUX_REBOOT_CMD_|libc::reboot|std::process::Command|Command::new'
if find include src tests -type f \( -name '*.cpp' -o -name '*.hpp' \) -exec grep -H -n -E "$forbidden" {} + 2>/dev/null; then
    echo 'ERROR: forbidden PID-1/helper implementation remains' >&2
    exit 1
fi
printf '%s\n' 'PASS: Phase 0 host and PID-1 safety invariants hold'
]]})
    end)

    register("test-golden-guards", "Prove the C++ test lane is read-only", function()
        ensure(not os.isfile(path.join(root, "tests/update_golden.cpp")), "golden mutation source is forbidden")
        color_output("${green}PASS: doctest lanes have no golden update path")
    end)

    register("update-golden", "Refuse unreviewed golden updates", function()
        fail("C++ golden updates require an explicit reviewed implementation")
    end)

    register("verify", "Run the complete local C++ gate", function()
        run_xmake({"phase0-safety"})
        run_xmake({"test-golden-guards"})
        run_xmake({"vm-script-check"})
        run_xmake({"fmt-check"})
        run_xmake({"cpp-test"})
        run_xmake({"cpp-cli-check"})
    end)

    register("help", "Show Sart build and validation entrypoints", function()
        output("Sart uses Xmake; the root Makefile is a thin command forwarder.")
        output("")
        output("  make build             debug build")
        output("  make test              all doctest cases")
        output("  make verify            full local validation")
        output("  make static-build      publish a static-musl generation")
        output("  make artifact-check    inspect the published generation")
        output("  make release-package   create the deterministic archive")
        output("  make vm-script-check   validate VM infrastructure")
        output("  make vm-test-*         run an init-system VM lane")
        output("  make clean             remove generated outputs")
    end)

    register("h", "Alias for help", function()
        run_xmake({"help"})
    end)
end

do
    local root = path.normalize(path.absolute(os.scriptdir()))
    local artifact_root = path.join(root, "target/artifacts")
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

    local function register(name, description, action)
        task(name)
            set_category("sart")
            set_menu({usage = "xmake " .. name, description = description, options = {}})
            on_run(function()
                process = os
                fail = raise
                action()
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
            SART_ROOT = root,
            STATIC_ROOT = artifact_root,
            STATIC_ARCH = architecture(),
            NIX_NETWORK_MODE = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline",
        }
    end

    local function run_locked(task_name)
        execv("bash", {"scripts/artifact-lock.sh", root, "xmake", task_name}, {envs = task_environment()})
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
end

do
    local root = path.normalize(path.absolute(os.scriptdir()))
    local artifact_root = path.join(root, "target/artifacts")
    local static_binary = path.join(artifact_root, "current/release/sart")
    local aarch64_binary = path.join(root, "target/vm/cache/artifacts/aarch64/current")
    local process

    local function execv(program, arguments, options)
        process.execv(program, arguments, options)
    end

    local function register(name, description, action)
        task(name)
            set_category("sart.vm")
            set_menu({usage = "xmake " .. name, description = description, options = {}})
            on_run(function()
                process = os
                action()
            end)
        task_end()
    end

    local function vm_make(target, environment)
        execv("make", {"-C", "scripts/vm", target}, {envs = environment})
    end

    local function locked_vm_make(target, environment)
        execv("bash", {"scripts/artifact-lock.sh", root, "make", "-C", "scripts/vm", target},
              {envs = environment})
    end

    local function run_release_lanes(binary)
        execv("bash", {"-c", [=[
set -euo pipefail
bash scripts/artifact-lock-assert.sh "$SART_ROOT" >/dev/null
generation=$(bash scripts/artifact-generation.sh "$STATIC_ROOT")
elf=$(readlink -f -- "$SART_BIN")
test "$elf" = "$generation/release/sart" || {
    echo 'ERROR: release VM proof did not resolve the pinned static generation' >&2
    exit 1
}
digest=$(sha256sum -- "$elf"); digest=${digest%%[[:space:]]*}
test "${#digest}" -eq 64 || { echo 'ERROR: cannot hash release VM ELF' >&2; exit 1; }
printf 'sart: normal release ELF %s\n' "$digest"
for lane in install password lifecycle recovery uninstall kernel-update; do
    make -C scripts/vm "vm-test-$lane-dracut-systemd" SART_BIN="$elf"
done
printf 'SART_VM_UBUNTU_26_04_RELEASE_ELF_PASS_V1|sha256=%s\n' "$digest"
]=]}, {envs = {SART_ROOT = root, STATIC_ROOT = artifact_root, SART_BIN = binary}})
    end

    local infrastructure_targets = {
        "vm-script-check",
        "vm-preflight",
        "vm-state-init",
        "vm-image-alpine",
        "vm-image-alpine-3.24.1",
        "vm-image-ubuntu-26.04",
        "vm-image-fedora-44",
        "vm-image-debian-13.6",
        "vm-image-arch-mkinitcpio",
        "vm-sources-postmarketos",
        "vm-review-postmarketos-sources",
        "vm-kernel-packages-ubuntu-26.04",
        "vm-kernel-packages-fedora-44",
        "vm-kernel-packages-alpine-3.24",
        "vm-kernel-packages-debian-13.6",
        "vm-kernel-packages-arch-mkinitcpio",
        "vm-reset-arch-mkinitcpio-systemd",
        "vm-provision-arch-mkinitcpio-systemd",
        "vm-verify-arch-mkinitcpio-systemd",
        "vm-reset-alpine-3.24.1-mkinitfs-openrc",
        "vm-provision-alpine-3.24.1-mkinitfs-openrc",
        "vm-verify-alpine-3.24.1-mkinitfs-openrc",
        "vm-reset-postmarketos-qemu-aarch64",
        "vm-provision-postmarketos-qemu-aarch64",
        "vm-verify-postmarketos-qemu-aarch64",
        "vm-reset-postmarketos-qemu-aarch64-systemd",
        "vm-provision-postmarketos-qemu-aarch64-systemd",
        "vm-verify-postmarketos-qemu-aarch64-systemd",
        "vm-reset-ubuntu-26.04-dracut-systemd",
        "vm-provision-ubuntu-26.04-dracut-systemd",
        "vm-verify-ubuntu-26.04-dracut-systemd",
        "vm-reset-fedora-44-dracut-systemd",
        "vm-provision-fedora-44-dracut-systemd",
        "vm-verify-fedora-44-dracut-systemd",
        "vm-reset-debian-13.6-initramfs-tools-systemd",
        "vm-provision-debian-13.6-initramfs-tools-systemd",
        "vm-verify-debian-13.6-initramfs-tools-systemd",
        "vm-adapter-policy-check",
        "vm-clean",
    }

    for _, target in ipairs(infrastructure_targets) do
        local task_name = target
        register(task_name, "Delegate to the disposable VM infrastructure", function()
            vm_make(task_name)
        end)
    end

    register("vm-artifact-aarch64", "Build the static aarch64 VM artifact", function()
        execv("xmake", {"phase0-safety"})
        execv("xmake", {"nix-check"})
        vm_make("vm-state-init")
        local network = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline"
        execv("bash", {"scripts/vm/scripts/build-aarch64-artifact.sh", root, path.join(root, "target/vm"), network,
                        "nix"})
    end)

    register("vm-test-lifecycle-alpine", "Run the bounded no-disk QEMU lifecycle gate", function()
        locked_vm_make("vm-test-lifecycle-alpine")
    end)

    local lanes = {"lifecycle", "install", "password", "recovery", "uninstall", "kernel-update"}
    local static_pairs = {"dracut-systemd", "initramfs-tools", "mkinitcpio", "mkinitfs-openrc"}
    local blocked_pairs = {"dracut-classic"}
    local aarch64_pairs = {"mkinitfs-boot-deploy-openrc", "mkinitfs-boot-deploy-systemd"}

    for _, pair in ipairs(static_pairs) do
        for _, lane in ipairs(lanes) do
            local target = "vm-test-" .. lane .. "-" .. pair
            register(target, "Run one static x86_64 init-system adapter lane", function()
                execv("xmake", {"static-build"})
                locked_vm_make(target, {SART_BIN = static_binary})
            end)
        end
    end

    for _, pair in ipairs(blocked_pairs) do
        for _, lane in ipairs(lanes) do
            local target = "vm-test-" .. lane .. "-" .. pair
            register(target, "Run one experimental init-system adapter lane", function()
                locked_vm_make(target, {SART_BIN = static_binary})
            end)
        end
    end

    for _, pair in ipairs(aarch64_pairs) do
        for _, lane in ipairs(lanes) do
            local target = "vm-test-" .. lane .. "-" .. pair
            register(target, "Run one aarch64 boot-deploy adapter lane", function()
                execv("xmake", {"vm-artifact-aarch64"})
                locked_vm_make(target, {SART_BIN = aarch64_binary})
            end)
        end
    end

    local fedora_targets = {
        "vm-test-install-fedora-44-dracut-systemd",
        "vm-test-lifecycle-fedora-44-dracut-systemd",
        "vm-test-password-fedora-44-dracut-systemd",
        "vm-test-recovery-fedora-44-dracut-systemd",
        "vm-test-uninstall-fedora-44-dracut-systemd",
        "vm-test-kernel-update-fedora-44-dracut-systemd",
    }

    for _, target in ipairs(fedora_targets) do
        local task_name = target
        register(task_name, "Run one Fedora dracut/systemd adapter lane", function()
            execv("xmake", {"static-build"})
            locked_vm_make(task_name, {SART_BIN = static_binary})
        end)
    end

    local aggregate_targets = {
        "vm-test-debian-13.6-initramfs-tools-systemd",
        "vm-test-arch-mkinitcpio-systemd",
        "vm-test-alpine-3.24.1-mkinitfs-openrc",
        "vm-test-ubuntu-26.04-dracut-systemd",
        "vm-test-fedora-44-dracut-systemd",
        "vm-test-adapters",
        "vm-test",
    }

    for _, target in ipairs(aggregate_targets) do
        local task_name = target
        register(task_name, "Run an aggregate init-system VM gate", function()
            execv("xmake", {"static-build"})
            locked_vm_make(task_name, {SART_BIN = static_binary})
        end)
    end

    register("_vm-test-release-ubuntu-26.04-dracut-systemd-locked", "Run pinned Ubuntu release ELF lanes", function()
        run_release_lanes(os.getenv("SART_BIN") or static_binary)
    end)

    register("vm-test-release-ubuntu-26.04-dracut-systemd", "Run all Ubuntu release ELF lanes", function()
        execv("xmake", {"static-build"})
        execv("bash", {"scripts/artifact-lock.sh", root, "xmake",
                        "_vm-test-release-ubuntu-26.04-dracut-systemd-locked"},
              {envs = {SART_BIN = static_binary}})
    end)

    register("vm-run-gui", "Launch the disposable graphical splash guest", function()
        execv("xmake", {"static-build"})
        locked_vm_make("vm-run-gui", {SART_BIN = static_binary})
    end)

    register("vm-run-gui-password", "Launch the graphical password guest", function()
        execv("xmake", {"static-build"})
        locked_vm_make("vm-run-gui-password", {SART_BIN = static_binary})
    end)

    local static_gui_targets = {
        "vm-run-gui-ubuntu-26.04-dracut-systemd",
        "vm-run-gui-fedora-44-dracut-systemd",
        "vm-run-gui-debian-13.6-initramfs-tools-systemd",
        "vm-run-gui-arch-mkinitcpio-systemd",
        "vm-run-gui-alpine-3.24.1-mkinitfs-openrc",
    }

    for _, target in ipairs(static_gui_targets) do
        local task_name = target
        register(task_name, "Launch a cached init-system guest with the current static ELF", function()
            locked_vm_make(task_name, {SART_BIN = static_binary})
        end)
    end

    for _, target in ipairs({"vm-run-gui-postmarketos-qemu-aarch64",
                              "vm-run-gui-postmarketos-qemu-aarch64-systemd"}) do
        local task_name = target
        register(task_name, "Launch a cached aarch64 postmarketOS guest", function()
            locked_vm_make(task_name, {SART_BIN = aarch64_binary})
        end)
    end
end
