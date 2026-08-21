local project_root = path.normalize(path.absolute(path.join(os.scriptdir(), "../..")))
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

local function in_project(action)
    action()
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
            in_project(action)
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
    execv(path.join(project_root, "target/cpp/musl/sart"), {"--version"})
    execv(path.join(project_root, "target/cpp/musl/sart"), {"install", "--help"})
end)

register("cpp-nix-build", "Build the locked static Nix package", function()
    local network = os.getenv("NIX_OFFLINE") == "0" and "online" or "offline"
    execv("bash", {"scripts/nix-source-command.sh", project_root, network, "build", "nix", "sart-cpp-static"})
end)

register("cpp-clean", "Remove C++ and Xmake build outputs", function()
    os.tryrm(path.join(project_root, "target/cpp"))
    os.tryrm(path.join(project_root, "target/xmake"))
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
    execv("bash", {"scripts/nix-source-command.sh", project_root, network, "check", "nix"})
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
    ensure(not os.isfile(path.join(project_root, "tests/update_golden.cpp")), "golden mutation source is forbidden")
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
