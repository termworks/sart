local project_root = path.normalize(path.absolute(path.join(os.scriptdir(), "../..")))
local artifact_root = path.join(project_root, "target/artifacts")
local static_binary = path.join(project_root, "target/artifacts/current/release/sart")
local aarch64_binary = path.join(project_root, "target/vm/cache/artifacts/aarch64/current")
local process

local function execv(program, arguments, options)
    process.execv(program, arguments, options)
end

local function in_project(action)
    action()
end

local function register(name, description, action)
    task(name)
        set_category("sart.vm")
        set_menu({usage = "xmake " .. name, description = description, options = {}})
        on_run(function()
            process = os
            in_project(action)
        end)
    task_end()
end

local function vm_make(target, environment)
    execv("make", {"-C", "scripts/vm", target}, {envs = environment})
end

local function locked_vm_make(target, environment)
    execv("bash", {"scripts/artifact-lock.sh", project_root, "make", "-C", "scripts/vm", target},
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
]=]}, {envs = {SART_ROOT = project_root, STATIC_ROOT = artifact_root, SART_BIN = binary}})
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
    execv("bash", {"scripts/vm/scripts/build-aarch64-artifact.sh", project_root,
                    path.join(project_root, "target/vm"), network, "nix"})
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
    execv("bash", {"scripts/artifact-lock.sh", project_root, "xmake",
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
