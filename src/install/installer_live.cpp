#include "sart/install/live.hpp"

#include "sart/core/sha256.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <format>
#include <fstream>
#include <limits>
#include <optional>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <tuple>
#include <unistd.h>
#include <utility>

namespace sart::install {
    namespace {

        struct ReadFile {
            std::vector<std::byte> bytes;
            std::uint16_t mode;
            struct stat status;
        };

        struct BootFilesystem {
            std::uint64_t root_device;
            std::uint64_t boot_device;
            bool writable;
            std::uint64_t free_bytes;
            std::uint64_t allocation_unit;
            std::uint64_t total_inodes;
            std::uint64_t free_inodes;
        };

        std::runtime_error path_error(std::string_view action, std::string_view path) {
            return std::runtime_error(std::format("{} {}: {}", action, path, std::strerror(errno)));
        }

        std::string as_text(std::span<const std::byte> bytes) {
            if (std::ranges::find(bytes, std::byte{}) != bytes.end()) {
                throw std::runtime_error("installer fact contains NUL bytes");
            }
            return {reinterpret_cast<const char *>(bytes.data()), bytes.size()};
        }

        std::string trim_line(std::string value) {
            while (!value.empty() && (value.back() == '\n' || value.back() == '\r'))
                value.pop_back();
            if (value.empty() || std::ranges::any_of(value, [](unsigned char byte) {
                    return byte == '\0' || byte == '\n' || byte == '\r';
                })) {
                throw std::runtime_error("installer fact is empty or malformed");
            }
            return value;
        }

        ReadFile read_regular(std::string_view path, std::uint64_t limit, bool require_nonempty = false) {
            const std::string name(path);
            struct stat before{};
            if (lstat(name.c_str(), &before) != 0)
                throw path_error("inspect", path);
            if (!S_ISREG(before.st_mode) || S_ISLNK(before.st_mode) || before.st_uid != 0 || before.st_nlink != 1 ||
                (before.st_mode & 0022) != 0 || before.st_size < 0 ||
                static_cast<std::uint64_t>(before.st_size) > limit || (require_nonempty && before.st_size == 0)) {
                throw std::runtime_error("unsafe installer fact: " + name);
            }
            const int descriptor = open(name.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw path_error("open", path);
            struct stat opened{};
            if (fstat(descriptor, &opened) != 0 || opened.st_dev != before.st_dev || opened.st_ino != before.st_ino ||
                opened.st_size != before.st_size) {
                close(descriptor);
                throw std::runtime_error("installer fact changed while opening: " + name);
            }
            ReadFile result{{}, static_cast<std::uint16_t>(opened.st_mode & 07777), opened};
            result.bytes.resize(static_cast<std::size_t>(opened.st_size));
            std::size_t offset = 0;
            while (offset < result.bytes.size()) {
                const auto count = read(descriptor, result.bytes.data() + offset, result.bytes.size() - offset);
                if (count < 0 && errno == EINTR)
                    continue;
                if (count <= 0) {
                    close(descriptor);
                    throw std::runtime_error("installer fact changed while reading: " + name);
                }
                offset += static_cast<std::size_t>(count);
            }
            std::byte extra{};
            const auto tail = read(descriptor, &extra, 1);
            if (close(descriptor) != 0)
                throw path_error("close", path);
            if (tail != 0)
                throw std::runtime_error("installer fact changed size while reading: " + name);
            return result;
        }

        ReadFile read_stream_regular(std::string_view path, std::uint64_t limit, bool require_nonempty = false) {
            const std::string name(path);
            struct stat before{};
            if (lstat(name.c_str(), &before) != 0)
                throw path_error("inspect", path);
            if (!S_ISREG(before.st_mode) || S_ISLNK(before.st_mode) || before.st_uid != 0 || before.st_nlink != 1 ||
                (before.st_mode & 0022) != 0 || before.st_size < 0 ||
                static_cast<std::uint64_t>(before.st_size) > limit) {
                throw std::runtime_error("unsafe installer fact: " + name);
            }
            const int descriptor = open(name.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw path_error("open", path);
            struct stat opened{};
            if (fstat(descriptor, &opened) != 0 || opened.st_dev != before.st_dev || opened.st_ino != before.st_ino ||
                opened.st_uid != before.st_uid || opened.st_mode != before.st_mode ||
                opened.st_nlink != before.st_nlink) {
                close(descriptor);
                throw std::runtime_error("installer fact changed while opening: " + name);
            }
            ReadFile result{{}, static_cast<std::uint16_t>(opened.st_mode & 07777), opened};
            std::array<std::byte, 4096> buffer{};
            while (true) {
                const auto count = read(descriptor, buffer.data(), buffer.size());
                if (count < 0 && errno == EINTR)
                    continue;
                if (count < 0) {
                    const auto error = path_error("read", path);
                    close(descriptor);
                    throw error;
                }
                if (count == 0)
                    break;
                if (result.bytes.size() + static_cast<std::size_t>(count) > limit) {
                    close(descriptor);
                    throw std::runtime_error("installer fact exceeds its size bound: " + name);
                }
                result.bytes.insert(result.bytes.end(), buffer.begin(), buffer.begin() + count);
            }
            struct stat after{};
            if (fstat(descriptor, &after) != 0 || after.st_dev != opened.st_dev || after.st_ino != opened.st_ino ||
                after.st_uid != opened.st_uid || after.st_mode != opened.st_mode || after.st_nlink != opened.st_nlink) {
                close(descriptor);
                throw std::runtime_error("installer fact changed while reading: " + name);
            }
            if (close(descriptor) != 0)
                throw path_error("close", path);
            if (require_nonempty && result.bytes.empty())
                throw std::runtime_error("installer fact is empty: " + name);
            return result;
        }

        bool path_exists(std::string_view path) {
            struct stat status{};
            if (lstat(std::string(path).c_str(), &status) == 0)
                return true;
            if (errno == ENOENT || errno == ENOTDIR)
                return false;
            throw path_error("inspect", path);
        }

        ToolFact inspect_tool(std::string_view path) {
            const auto file = read_regular(path, 64 * 1024 * 1024, true);
            if ((file.mode & 0111) == 0) {
                throw std::runtime_error("installer executable is not executable: " + std::string(path));
            }
            return ToolFact::exact(path);
        }

        template <typename Fact> Fact inspect_contract_file(std::string_view path, bool executable) {
            const auto file = read_regular(path, max_candidate_bytes, true);
            if (((file.mode & 0111) != 0) != executable) {
                throw std::runtime_error("installer prerequisite executable mode differs: " + std::string(path));
            }
            return Fact{std::string(path), true, true, false, executable};
        }

        std::vector<std::string> child_names(std::string_view path, bool directories, std::size_t limit) {
            struct stat parent{};
            if (lstat(std::string(path).c_str(), &parent) != 0)
                throw path_error("inspect directory", path);
            if (!S_ISDIR(parent.st_mode) || S_ISLNK(parent.st_mode) || parent.st_uid != 0 ||
                (parent.st_mode & 0022) != 0) {
                throw std::runtime_error("unsafe installer directory: " + std::string(path));
            }
            std::vector<std::string> result;
            for (const auto &entry : std::filesystem::directory_iterator(std::filesystem::path(path),
                                                                         std::filesystem::directory_options::none)) {
                const auto name = entry.path().filename().string();
                if (name.empty() || name == "." || name == ".." || name.contains('/')) {
                    throw std::runtime_error("unsafe installer directory entry");
                }
                const auto status = entry.symlink_status();
                if ((directories && status.type() != std::filesystem::file_type::directory) ||
                    (!directories && status.type() != std::filesystem::file_type::regular)) {
                    throw std::runtime_error("installer directory contains an unexpected entry: " +
                                             entry.path().string());
                }
                struct stat child{};
                if (lstat(entry.path().c_str(), &child) != 0)
                    throw path_error("inspect installer directory entry", entry.path().string());
                if (child.st_uid != 0 || (child.st_mode & 0022) != 0 ||
                    (directories && (!S_ISDIR(child.st_mode) || S_ISLNK(child.st_mode))) ||
                    (!directories && (!S_ISREG(child.st_mode) || S_ISLNK(child.st_mode) || child.st_nlink != 1))) {
                    throw std::runtime_error("installer directory contains an unsafe entry: " + entry.path().string());
                }
                if (result.size() == limit)
                    throw std::runtime_error("installer directory exceeds its entry bound");
                result.push_back(name);
            }
            std::ranges::sort(result);
            return result;
        }

        std::string pid1_comm() { return trim_line(as_text(read_stream_regular("/proc/1/comm", 4096, true).bytes)); }

        std::string running_kernel(std::string_view modules_root) {
            const auto installed = child_names(modules_root, true, 64);
            const auto running =
                trim_line(as_text(read_stream_regular("/proc/sys/kernel/osrelease", 4096, true).bytes));
            if (std::ranges::find(installed, running) == installed.end()) {
                throw std::runtime_error("running kernel has no exact installed module tree");
            }
            return running;
        }

        BootFilesystem boot_filesystem() {
            struct stat root{}, boot{};
            if (lstat("/", &root) != 0)
                throw path_error("inspect", "/");
            if (lstat("/boot", &boot) != 0)
                throw path_error("inspect", "/boot");
            if (!S_ISDIR(root.st_mode) || !S_ISDIR(boot.st_mode) || S_ISLNK(boot.st_mode) || root.st_uid != 0 ||
                boot.st_uid != 0 || (boot.st_mode & 0022) != 0) {
                throw std::runtime_error("unsafe live root or /boot directory");
            }
            const int descriptor = open("/boot", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw path_error("open", "/boot");
            struct stat opened{};
            struct statvfs filesystem{};
            if (fstat(descriptor, &opened) != 0 || opened.st_dev != boot.st_dev || opened.st_ino != boot.st_ino ||
                fstatvfs(descriptor, &filesystem) != 0) {
                close(descriptor);
                throw std::runtime_error("/boot changed while collecting filesystem facts");
            }
            close(descriptor);
            const auto allocation = filesystem.f_frsize == 0 ? filesystem.f_bsize : filesystem.f_frsize;
            if (allocation == 0)
                throw std::runtime_error("/boot reports a zero allocation unit");
            const auto available = filesystem.f_bavail > std::numeric_limits<std::uint64_t>::max() / allocation
                                       ? std::numeric_limits<std::uint64_t>::max()
                                       : static_cast<std::uint64_t>(filesystem.f_bavail) * allocation;
            return {static_cast<std::uint64_t>(root.st_dev),        static_cast<std::uint64_t>(boot.st_dev),
                    (filesystem.f_flag & ST_RDONLY) == 0,           available,
                    static_cast<std::uint64_t>(allocation),         static_cast<std::uint64_t>(filesystem.f_files),
                    static_cast<std::uint64_t>(filesystem.f_favail)};
        }

        std::string boot_uuid() {
            const auto source = as_text(read_regular("/etc/fstab", 1024 * 1024, true).bytes);
            std::optional<std::string> result;
            std::istringstream input(source);
            std::string line;
            while (std::getline(input, line)) {
                const auto first = line.find_first_not_of(" \t\r");
                if (first == std::string::npos || line[first] == '#')
                    continue;
                std::istringstream fields(line.substr(first));
                std::string device, mount;
                if (!(fields >> device >> mount) || mount != "/boot")
                    continue;
                constexpr std::string_view by_uuid = "/dev/disk/by-uuid/";
                if (device.starts_with("UUID="))
                    device.erase(0, 5);
                else if (device.starts_with(by_uuid))
                    device.erase(0, by_uuid.size());
                else
                    throw std::runtime_error("/boot fstab source is not an explicit UUID");
                if (result)
                    throw std::runtime_error("/boot has multiple fstab entries");
                result = std::move(device);
            }
            if (!result)
                throw std::runtime_error("/boot fstab entry is missing");
            return *result;
        }

        std::string kernel_command_line(std::uint64_t limit = 16 * 1024) {
            return trim_line(as_text(read_stream_regular("/proc/cmdline", limit, true).bytes));
        }

        CryptsetupLocation select_cryptsetup() {
            std::vector<CryptsetupLocation> found;
            for (const auto location : {CryptsetupLocation::usr_bin, CryptsetupLocation::usr_sbin}) {
                const auto path = cryptsetup_executable(location);
                if (!path_exists(path))
                    continue;
                static_cast<void>(inspect_tool(path));
                found.push_back(location);
            }
            if (found.size() != 1)
                throw std::runtime_error("exactly one supported cryptsetup executable is required");
            return found.front();
        }

        GrubRegeneration select_grub(bool mkinitcpio = false) {
            const std::array profiles =
                mkinitcpio ? std::array{GrubRegeneration::grub_mkconfig, GrubRegeneration::grub_mkconfig}
                           : std::array{GrubRegeneration::update_grub, GrubRegeneration::grub2_mkconfig};
            std::vector<GrubRegeneration> found;
            for (std::size_t index = 0; index < profiles.size(); ++index) {
                if (mkinitcpio && index == 1)
                    break;
                const auto profile = profiles[index];
                const bool updater = path_exists(grub_updater(profile));
                const bool probe = path_exists(grub_probe(profile));
                if (updater != probe)
                    throw std::runtime_error("GRUB capability is incomplete");
                if (updater)
                    found.push_back(profile);
            }
            if (found.size() != 1)
                throw std::runtime_error("exactly one supported GRUB capability is required");
            return found.front();
        }

        std::vector<ToolFact> inspect_tools(std::initializer_list<std::string_view> paths) {
            std::vector<ToolFact> result;
            result.reserve(paths.size());
            for (const auto path : paths)
                result.push_back(inspect_tool(path));
            return result;
        }

        DracutSystemdContract discover_dracut() {
            const auto kernel = running_kernel("/usr/lib/modules");
            std::vector<std::string> modules;
            for (const auto &directory : child_names("/usr/lib/dracut/modules.d", true, 256)) {
                if (directory.size() < 3 || directory[0] < '0' || directory[0] > '9' || directory[1] < '0' ||
                    directory[1] > '9') {
                    throw std::runtime_error("invalid dracut module directory: " + directory);
                }
                modules.push_back(directory.substr(2));
            }
            std::vector<DracutImageLayout> layouts;
            if (path_exists("/boot/initrd.img-" + kernel))
                layouts.push_back(DracutImageLayout::initrd_img);
            if (path_exists("/boot/initramfs-" + kernel + ".img"))
                layouts.push_back(DracutImageLayout::initramfs_img);
            if (layouts.size() != 1)
                throw std::runtime_error("exactly one supported dracut image layout is required");
            const auto grub = select_grub();
            const auto cryptsetup = select_cryptsetup();
            auto tools =
                inspect_tools({"/usr/bin/dracut", "/usr/bin/lsinitrd", "/usr/bin/findmnt", "/usr/lib/systemd/systemd",
                               cryptsetup_executable(cryptsetup), grub_updater(grub), grub_probe(grub)});
            const auto active = layouts.front() == DracutImageLayout::initrd_img ? "/boot/initrd.img-" + kernel
                                                                                 : "/boot/initramfs-" + kernel + ".img";
            const auto image = read_regular(active, max_candidate_bytes, true);
            const auto boot = boot_filesystem();
            return plan_dracut_systemd({std::string(product_architecture),
                                        pid1_comm(),
                                        {kernel},
                                        boot.root_device,
                                        boot.boot_device,
                                        boot.writable,
                                        boot.free_bytes,
                                        boot.free_inodes,
                                        std::move(modules),
                                        layouts.front(),
                                        grub,
                                        cryptsetup,
                                        std::move(tools),
                                        active,
                                        sha256(image.bytes),
                                        image.bytes.size(),
                                        boot_uuid(),
                                        kernel_command_line()});
        }

        InitramfsToolsSystemdContract discover_initramfs_tools() {
            const auto kernel = running_kernel("/usr/lib/modules");
            const auto grub = select_grub();
            const auto cryptsetup = select_cryptsetup();
            auto tools = inspect_tools({"/usr/sbin/mkinitramfs", "/usr/bin/unmkinitramfs", "/usr/bin/findmnt",
                                        "/usr/lib/systemd/systemd", cryptsetup_executable(cryptsetup),
                                        grub_updater(grub), grub_probe(grub)});
            std::vector<InitramfsToolsPathFact> files;
            for (const auto [path, executable] :
                 std::array{std::pair{"/usr/share/initramfs-tools/hook-functions", false},
                            std::pair{"/usr/share/initramfs-tools/hooks/cryptroot", true},
                            std::pair{"/usr/share/initramfs-tools/scripts/local-top/cryptroot", true},
                            std::pair{"/usr/lib/cryptsetup/functions", false},
                            std::pair{"/usr/lib/cryptsetup/askpass", true}}) {
                files.push_back(inspect_contract_file<InitramfsToolsPathFact>(path, executable));
            }
            const auto active = "/boot/initrd.img-" + kernel;
            const auto image = read_regular(active, max_candidate_bytes, true);
            const auto boot = boot_filesystem();
            return plan_initramfs_tools_systemd({std::string(product_architecture),
                                                 pid1_comm(),
                                                 {kernel},
                                                 boot.root_device,
                                                 boot.boot_device,
                                                 boot.writable,
                                                 boot.free_bytes,
                                                 boot.free_inodes,
                                                 grub,
                                                 cryptsetup,
                                                 std::move(tools),
                                                 std::move(files),
                                                 active,
                                                 sha256(image.bytes),
                                                 image.bytes.size(),
                                                 boot_uuid(),
                                                 kernel_command_line()});
        }

        MkinitcpioSystemdContract discover_mkinitcpio() {
            const auto kernel = running_kernel("/usr/lib/modules");
            const auto package =
                trim_line(as_text(read_regular("/usr/lib/modules/" + kernel + "/pkgbase", 4096, true).bytes));
            const auto preset = as_text(read_regular("/etc/mkinitcpio.d/" + package + ".preset", 65536, true).bytes);
            const auto config = read_regular("/etc/mkinitcpio.conf", 65536, true);
            const auto cryptsetup = select_cryptsetup();
            auto tools = inspect_tools({"/usr/bin/mkinitcpio", "/usr/bin/lsinitcpio", "/usr/bin/findmnt",
                                        "/usr/lib/systemd/systemd", "/usr/bin/grub-mkconfig", "/usr/bin/grub-probe",
                                        cryptsetup_executable(cryptsetup)});
            std::vector<MkinitcpioPathFact> files;
            for (const auto [path, executable] :
                 std::array{std::pair{"/usr/lib/initcpio/functions", true}, std::pair{"/usr/lib/initcpio/init", false},
                            std::pair{"/usr/lib/initcpio/hooks/encrypt", false},
                            std::pair{"/usr/lib/initcpio/install/encrypt", false}}) {
                files.push_back(inspect_contract_file<MkinitcpioPathFact>(path, executable));
            }
            const auto active = "/boot/initramfs-" + package + ".img";
            const auto image = read_regular(active, max_candidate_bytes, true);
            const auto boot = boot_filesystem();
            return plan_mkinitcpio_systemd({std::string(product_architecture),
                                            pid1_comm(),
                                            {kernel},
                                            package,
                                            boot.root_device,
                                            boot.boot_device,
                                            boot.writable,
                                            boot.free_bytes,
                                            boot.free_inodes,
                                            cryptsetup,
                                            std::move(tools),
                                            std::move(files),
                                            as_text(config.bytes),
                                            config.mode,
                                            preset,
                                            active,
                                            sha256(image.bytes),
                                            image.bytes.size(),
                                            boot_uuid(),
                                            kernel_command_line()});
        }

        MkinitfsOpenRcPathFact inspect_mkinitfs_file(std::string_view path) {
            const auto file = read_regular(path, max_candidate_bytes, true);
            if ((file.mode & 0111) != 0)
                throw std::runtime_error("mkinitfs contract file is executable");
            return {std::string(path), true, true, false, false, file.mode, sha256(file.bytes)};
        }

        MkinitfsOpenRcContract discover_mkinitfs_openrc() {
            const auto kernel = running_kernel("/lib/modules");
            const auto dash = kernel.rfind('-');
            if (dash == std::string::npos || dash + 1 == kernel.size()) {
                throw std::runtime_error("running kernel has no mkinitfs flavor");
            }
            const auto flavor = kernel.substr(dash + 1);
            const auto kernel_image = "/boot/vmlinuz-" + flavor;
            const auto active = "/boot/initramfs-" + flavor;
            auto tools = inspect_tools({"/sbin/mkinitfs", "/sbin/update-extlinux", "/sbin/extlinux", "/sbin/openrc"});
            std::vector<MkinitfsOpenRcPathFact> files;
            for (const auto path : {"/usr/share/mkinitfs/initramfs-init", "/etc/mkinitfs/mkinitfs.conf",
                                    "/etc/update-extlinux.conf", "/boot/extlinux.conf"}) {
                files.push_back(inspect_mkinitfs_file(path));
            }
            files.push_back(inspect_mkinitfs_file(kernel_image));
            const auto init = as_text(read_regular("/usr/share/mkinitfs/initramfs-init", 1024 * 1024, true).bytes);
            const auto config = as_text(read_regular("/etc/mkinitfs/mkinitfs.conf", 65536, true).bytes);
            const auto features = parse_mkinitfs_features(config);
            const auto update = as_text(read_regular("/etc/update-extlinux.conf", 65536, true).bytes);
            const auto settings = parse_update_extlinux_settings(update);
            const auto extlinux = as_text(read_regular("/boot/extlinux.conf", 1024 * 1024, true).bytes);
            const auto command_line = parse_extlinux_entry_command_line(extlinux, settings.default_label);
            if (command_line != settings.kernel_command_line) {
                throw std::runtime_error("active extlinux entry differs from update-extlinux settings");
            }
            const auto image = read_regular(active, max_candidate_bytes, true);
            const auto boot = boot_filesystem();
            return plan_mkinitfs_openrc({std::string(product_architecture),
                                         pid1_comm(),
                                         {kernel},
                                         boot.writable,
                                         boot.free_bytes,
                                         boot.free_inodes,
                                         std::move(tools),
                                         std::move(files),
                                         init,
                                         config,
                                         features,
                                         settings.overwrite,
                                         settings.default_label,
                                         command_line,
                                         active,
                                         sha256(image.bytes),
                                         image.bytes.size()});
        }

        MkinitfsBootDeployPathFact inspect_boot_deploy_file(std::string_view path, bool executable) {
            return inspect_contract_file<MkinitfsBootDeployPathFact>(path, executable);
        }

        std::optional<ReadFile> read_optional_regular(std::string_view path, std::uint64_t limit) {
            if (!path_exists(path))
                return std::nullopt;
            return read_regular(path, limit, true);
        }

        std::tuple<std::string, std::string, std::uint64_t> inspect_android_dtb(std::string_view relative_name) {
            std::vector<std::string> candidates;
            std::size_t count = 0;
            for (const auto &entry : std::filesystem::directory_iterator("/boot")) {
                if (++count > 256)
                    throw std::runtime_error("/boot exceeds the Android DTB root inventory bound");
                const auto name = entry.path().filename().string();
                if (!name.starts_with("dtbs"))
                    continue;
                if (name.empty() || !std::ranges::all_of(name, [](unsigned char byte) {
                        return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') ||
                               (byte >= 'a' && byte <= 'z') || byte == '.' || byte == '_' || byte == '+' || byte == '-';
                    })) {
                    throw std::runtime_error("Android DTB root has an unsafe name");
                }
                struct stat root{};
                if (lstat(entry.path().c_str(), &root) != 0)
                    throw path_error("inspect Android DTB root", entry.path().string());
                if (!S_ISDIR(root.st_mode) || S_ISLNK(root.st_mode) || root.st_uid != 0 || (root.st_mode & 0022) != 0) {
                    throw std::runtime_error("Android DTB root is unsafe");
                }
                const auto candidate = entry.path() / (std::string(relative_name) + ".dtb");
                auto parent = candidate.parent_path();
                while (parent != entry.path()) {
                    struct stat status{};
                    if (lstat(parent.c_str(), &status) != 0) {
                        if (errno == ENOENT || errno == ENOTDIR)
                            break;
                        throw path_error("inspect Android DTB directory", parent.string());
                    }
                    if (!S_ISDIR(status.st_mode) || S_ISLNK(status.st_mode) || status.st_uid != 0 ||
                        (status.st_mode & 0022) != 0) {
                        throw std::runtime_error("Android DTB directory is unsafe");
                    }
                    parent = parent.parent_path();
                }
                if (path_exists(candidate.string()))
                    candidates.push_back(candidate.string());
            }
            if (candidates.size() != 1)
                throw std::runtime_error("Android boot transaction requires exactly one matching /boot/dtbs* DTB");
            const auto dtb = read_regular(candidates.front(), max_candidate_bytes, true);
            return {candidates.front(), sha256(dtb.bytes), dtb.bytes.size()};
        }

        std::optional<AndroidBootFacts> discover_android_boot() {
            constexpr std::string_view vendor_path = "/usr/share/deviceinfo/deviceinfo";
            constexpr std::string_view override_path = "/etc/deviceinfo";
            const auto vendor = read_optional_regular(vendor_path, 256 * 1024);
            const auto override = read_optional_regular(override_path, 256 * 1024);
            const auto managed = installed_android_boot_partition();
            if (!vendor && override && !managed)
                throw std::runtime_error("postmarketOS deviceinfo override exists without vendor deviceinfo");
            if (!vendor && !managed)
                return std::nullopt;
            const auto override_text = override ? std::optional<std::string>{as_text(override->bytes)} : std::nullopt;
            if (managed && !override_text)
                throw std::runtime_error("managed Android boot installation has no deviceinfo guard");
            const auto vendor_text = vendor && !managed ? as_text(vendor->bytes) : std::string{};
            const auto deviceinfo = parse_android_boot_deviceinfo(
                vendor_text, override_text ? std::optional<std::string_view>{*override_text} : std::nullopt,
                managed.has_value());
            if (!deviceinfo)
                return std::nullopt;
            if (managed && override->bytes != deviceinfo->no_flash_deviceinfo)
                throw std::runtime_error("managed Android deviceinfo guard differs from its canonical bytes");
            const auto label = managed ? managed->label
                                       : android_slot_partition_label(kernel_command_line(64 * 1024),
                                                                      deviceinfo->boot_partition_label);
            const auto partition = inspect_android_boot_partition(label);
            if (managed && (partition.label != managed->label || partition.canonical_path != managed->canonical_path ||
                            partition.device_number != managed->device_number || partition.bytes != managed->bytes ||
                            partition.digest != managed->digest)) {
                throw std::runtime_error("managed Android boot partition differs from the manifest identity");
            }
            auto [dtb_path, dtb_digest, dtb_bytes] = inspect_android_dtb(deviceinfo->dtb);
            return AndroidBootFacts{*deviceinfo, partition, std::move(dtb_path), std::move(dtb_digest), dtb_bytes};
        }

        MkinitfsBootDeployContract discover_boot_deploy(bool systemd_root) {
            auto tools = systemd_root
                             ? inspect_tools({"/usr/sbin/mkinitfs", "/usr/bin/boot-deploy", "/usr/lib/systemd/systemd"})
                             : inspect_tools({"/usr/sbin/mkinitfs", "/usr/bin/boot-deploy", "/usr/sbin/openrc"});
            std::vector<MkinitfsBootDeployPathFact> files;
            for (const auto [path, executable] :
                 std::array{std::pair{"/usr/share/initramfs/init.sh", true},
                            std::pair{"/usr/share/initramfs/init_2nd.sh", true},
                            std::pair{"/usr/share/initramfs/init_functions_2nd.sh", false},
                            std::pair{"/usr/share/boot-deploy/boot-deploy-functions.sh", true},
                            std::pair{"/usr/share/boot-deploy/os-customization", false}}) {
                files.push_back(inspect_boot_deploy_file(path, executable));
            }
            auto android_boot = discover_android_boot();
            const auto init = as_text(read_regular("/usr/share/initramfs/init.sh", 1024 * 1024, true).bytes);
            const auto version = parse_mkinitfs_boot_deploy_version(init);
            const auto functions =
                as_text(read_regular("/usr/share/initramfs/init_functions_2nd.sh", 1024 * 1024, true).bytes);
            std::vector<std::tuple<std::string, std::string, std::string, std::uint16_t, std::vector<std::byte>>>
                loaders;
            for (const auto &name : child_names("/boot/loader/entries", false, 64)) {
                if (name == "sart-known-good.conf")
                    continue;
                if (!name.ends_with(".conf"))
                    throw std::runtime_error("loader entries contain a non-conf file");
                const auto path = "/boot/loader/entries/" + name;
                const auto file = read_regular(path, 16384, true);
                try {
                    auto [kernel, command_line] = parse_mkinitfs_boot_deploy_loader_entry(as_text(file.bytes));
                    loaders.emplace_back(path, std::move(kernel), std::move(command_line), file.mode, file.bytes);
                } catch (const std::exception &) {
                }
            }
            if (loaders.size() != 1) {
                throw std::runtime_error("boot-deploy requires exactly one loader entry for /initramfs");
            }
            auto [loader_path, kernel_path, command_line, loader_mode, loader_bytes] = std::move(loaders.front());
            const auto kernel = read_regular(kernel_path, max_candidate_bytes, true);
            const auto image = read_regular("/boot/initramfs", max_candidate_bytes, true);
            const auto boot = boot_filesystem();
            return plan_mkinitfs_boot_deploy({std::string(product_architecture),
                                              pid1_comm(),
                                              boot.root_device,
                                              boot.boot_device,
                                              boot.writable,
                                              boot.free_bytes,
                                              boot.allocation_unit,
                                              boot.total_inodes,
                                              boot.free_inodes,
                                              std::move(tools),
                                              std::move(files),
                                              version,
                                              functions,
                                              kernel_path,
                                              kernel.bytes.size(),
                                              "/boot/initramfs",
                                              detect_mkinitfs_boot_deploy_compression(image.bytes),
                                              sha256(image.bytes),
                                              image.bytes.size(),
                                              loader_path,
                                              loader_mode,
                                              std::move(loader_bytes),
                                              command_line,
                                              std::move(android_boot)},
                                             systemd_root);
        }

        template <typename Contract, typename Function>
        void attempt(std::vector<ExactInstallDiscovery> &complete, std::vector<std::string> &failures,
                     std::string_view name, AdapterId initramfs, AdapterId real_root, Function &&function) {
            try {
                complete.push_back(
                    {ExactInstallContract{std::in_place_type<Contract>, function()}, initramfs, real_root});
            } catch (const std::exception &error) {
                failures.push_back(std::string(name) + ": " + error.what());
            }
        }

        std::string request_text(const GeneratorRequest &request) {
            std::string result = request.executable;
            for (const auto &argument : request.arguments)
                result += " " + argument;
            return result;
        }

        std::string json_escape(std::string_view input) {
            std::string result;
            for (const unsigned char byte : input) {
                switch (byte) {
                case '\\':
                    result += "\\\\";
                    break;
                case '"':
                    result += "\\\"";
                    break;
                case '\n':
                    result += "\\n";
                    break;
                case '\r':
                    result += "\\r";
                    break;
                case '\t':
                    result += "\\t";
                    break;
                default:
                    if (byte < 0x20)
                        result += std::format("\\u{:04x}", byte);
                    else
                        result.push_back(static_cast<char>(byte));
                }
            }
            return result;
        }

    } // namespace

    ExactInstallDiscovery discover_exact_install_contract() {
        std::vector<ExactInstallDiscovery> complete;
        std::vector<std::string> failures;
        attempt<DracutSystemdContract>(complete, failures, "dracut-systemd", AdapterId::dracut_systemd,
                                       AdapterId::systemd_real_root, discover_dracut);
        attempt<InitramfsToolsSystemdContract>(complete, failures, "initramfs-tools-systemd",
                                               AdapterId::initramfs_tools_busybox, AdapterId::systemd_real_root,
                                               discover_initramfs_tools);
        attempt<MkinitcpioSystemdContract>(complete, failures, "mkinitcpio-systemd", AdapterId::mkinitcpio_busybox,
                                           AdapterId::systemd_real_root, discover_mkinitcpio);
        attempt<MkinitfsOpenRcContract>(complete, failures, "mkinitfs-openrc", AdapterId::mkinitfs_busybox,
                                        AdapterId::openrc_real_root, discover_mkinitfs_openrc);
        attempt<MkinitfsBootDeployContract>(complete, failures, "mkinitfs-boot-deploy-openrc",
                                            AdapterId::mkinitfs_boot_deploy, AdapterId::openrc_real_root,
                                            [] { return discover_boot_deploy(false); });
        attempt<MkinitfsBootDeployContract>(complete, failures, "mkinitfs-boot-deploy-systemd",
                                            AdapterId::mkinitfs_boot_deploy, AdapterId::systemd_real_root,
                                            [] { return discover_boot_deploy(true); });
        if (complete.size() > 1) {
            throw std::runtime_error(
                "multiple complete initramfs capability contracts were detected; refusing an ambiguous mutation");
        }
        if (complete.empty()) {
            std::string message = "no complete initramfs capability contract was detected";
            for (const auto &failure : failures)
                message += "; " + failure;
            throw std::runtime_error(message);
        }
        return std::move(complete.front());
    }

    InstallPlan build_exact_self_install_plan(const ExactInstallDiscovery &discovery) {
        const auto *pair = adapter_pair(discovery.initramfs, discovery.real_root);
        if (pair == nullptr || pair->status != SupportStatus::proven_supported) {
            throw std::runtime_error("discovered adapter pair is not proven supported");
        }
        return build_self_install_plan(discovery.initramfs, discovery.real_root);
    }

    std::string render_exact_install_plan(const InstallPlan &plan, const ExactInstallDiscovery &discovery, bool json) {
        const auto backend = std::visit(
            [](const auto &contract) {
                return std::tuple{contract.active_image, contract.candidate_image, contract.known_good_image,
                                  request_text(contract.generate)};
            },
            discovery.contract);
        const auto &[active, candidate, known_good, generator] = backend;
        if (json) {
            auto generic = render_plan_json(plan, true);
            if (generic.empty() || generic.back() != '}')
                throw std::runtime_error("invalid rendered install plan");
            generic.pop_back();
            generic += std::format(",\"active_image\":\"{}\",\"candidate_image\":\"{}\",\"known_good_image\":\"{}\","
                                   "\"generator\":\"{}\"}}\n",
                                   json_escape(active), json_escape(candidate), json_escape(known_good),
                                   json_escape(generator));
            return generic;
        }
        auto generic = render_plan_human(plan, true);
        const auto operations = generic.find("operations:\n");
        if (operations == std::string::npos)
            throw std::runtime_error("invalid rendered install plan");
        const auto details = std::format("active-image: {}\ncandidate-image: {}\nknown-good-image: {}\ngenerator: {}\n",
                                         active, candidate, known_good, generator);
        generic.insert(operations, details);
        return generic;
    }

} // namespace sart::install
