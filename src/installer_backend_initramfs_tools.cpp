#include "sart/installer_backends.hpp"

#include "sart/embedded.hpp"
#include "sart/sha256.hpp"

#include <algorithm>
#include <array>
#include <fcntl.h>
#include <filesystem>
#include <map>
#include <set>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>

namespace sart::install {
    namespace {

        constexpr std::string_view mkinitramfs = "/usr/sbin/mkinitramfs";
        constexpr std::string_view unmkinitramfs = "/usr/bin/unmkinitramfs";
        constexpr std::string_view findmnt = "/usr/bin/findmnt";
        constexpr std::string_view systemd = "/usr/lib/systemd/systemd";

        bool safe_token(std::string_view value, std::size_t maximum) {
            return !value.empty() && value.size() <= maximum && std::ranges::all_of(value, [](unsigned char byte) {
                const bool alnum =
                    (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z');
                return alnum || byte == '.' || byte == '_' || byte == '+' || byte == '-';
            });
        }

        bool safe_root(std::string_view value) {
            if (value.empty() || value.size() > 4096 || value.front() != '/' || value.contains('\0'))
                return false;
            std::size_t offset = 1;
            while (offset < value.size()) {
                const auto end = value.find('/', offset);
                const auto part =
                    value.substr(offset, end == std::string_view::npos ? value.size() - offset : end - offset);
                if (part.empty() || part == "." || part == "..")
                    return false;
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return true;
        }

        bool safe_transaction(std::string_view value) {
            return !value.empty() && value.size() <= 128 && std::ranges::all_of(value, [](unsigned char byte) {
                return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z') ||
                       byte == '-';
            });
        }

        std::string active_image(std::string_view kernel) { return "/boot/initrd.img-" + std::string(kernel); }
        std::string candidate_image(std::string_view kernel) {
            return "/boot/.sart-candidate-initrd.img-" + std::string(kernel);
        }

        std::optional<std::string_view> active_kernel(std::string_view path) {
            constexpr std::string_view prefix = "/boot/initrd.img-";
            if (!path.starts_with(prefix))
                return std::nullopt;
            const auto kernel = path.substr(prefix.size());
            return safe_token(kernel, 128) ? std::optional(kernel) : std::nullopt;
        }

        std::optional<std::string_view> candidate_kernel(std::string_view path) {
            constexpr std::string_view prefix = "/boot/.sart-candidate-initrd.img-";
            if (!path.starts_with(prefix))
                return std::nullopt;
            const auto kernel = path.substr(prefix.size());
            return safe_token(kernel, 128) ? std::optional(kernel) : std::nullopt;
        }

        GeneratorRequest request(GeneratorKind kind, std::string_view executable, std::string root,
                                 std::vector<std::string> arguments) {
            return {kind, std::string(executable), std::move(root), std::nullopt, std::move(arguments), true};
        }

        std::vector<std::byte> bytes(std::string_view text) {
            return {reinterpret_cast<const std::byte *>(text.data()),
                    reinterpret_cast<const std::byte *>(text.data() + text.size())};
        }

        std::map<std::string, std::pair<std::uint16_t, std::vector<std::byte>>> expected_files() {
            std::map<std::string, std::pair<std::uint16_t, std::vector<std::byte>>> expected;
            for (const auto [path, id] : std::array{
                     std::pair<std::string_view, embedded::TemplateId>{
                         "main/scripts/init-top/sart", embedded::TemplateId::initramfs_tools_early_hook},
                     std::pair<std::string_view, embedded::TemplateId>{
                         "main/scripts/init-bottom/sart", embedded::TemplateId::initramfs_tools_bottom_hook},
                     std::pair<std::string_view, embedded::TemplateId>{
                         "main/usr/lib/cryptsetup/askpass", embedded::TemplateId::initramfs_tools_askpass_wrapper}}) {
                const auto resource = embedded::template_resource(id);
                expected.emplace(path, std::pair{resource.materialization.mode, bytes(resource.contents)});
            }
            return expected;
        }

        bool valid_layer_path(std::string_view path) {
            const auto slash = path.find('/');
            if (slash == std::string_view::npos || slash + 1 == path.size())
                return false;
            const auto layer = path.substr(0, slash);
            if (layer != "early" && layer != "early2" && layer != "main")
                return false;
            auto rest = path.substr(slash + 1);
            if (rest.front() == '/' || rest.contains('\0'))
                return false;
            std::size_t offset = 0;
            while (offset < rest.size()) {
                const auto end = rest.find('/', offset);
                const auto part =
                    rest.substr(offset, end == std::string_view::npos ? rest.size() - offset : end - offset);
                if (part.empty() || part == "." || part == "..")
                    return false;
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return true;
        }

        std::pair<std::map<std::string_view, const ArchiveEntry *>, std::uint64_t>
        common_inventory(const std::vector<ArchiveEntry> &entries) {
            if (entries.empty() || entries.size() > max_archive_entries) {
                throw std::runtime_error("initramfs-tools inventory is empty or oversized");
            }
            std::map<std::string_view, const ArchiveEntry *> seen;
            std::uint64_t total = 0;
            for (const auto &entry : entries) {
                if (!valid_layer_path(entry.path) || !seen.emplace(entry.path, &entry).second) {
                    throw std::runtime_error("unsafe or duplicate initramfs-tools member");
                }
                if (entry.bytes.size() > max_inspected_archive_bytes - total) {
                    throw std::runtime_error("initramfs-tools inventory byte limit exceeded");
                }
                total += entry.bytes.size();
            }
            return {std::move(seen), total};
        }

        void require_executable(const std::map<std::string_view, const ArchiveEntry *> &seen, std::string_view path) {
            const auto found = seen.find(path);
            if (found == seen.end() || found->second->kind != ArchiveEntryKind::file || found->second->mode != 0755 ||
                found->second->bytes.empty()) {
                throw std::runtime_error("unsafe initramfs-tools executable: " + std::string(path));
            }
        }

        bool sart_namespace(std::string_view path) {
            const auto slash = path.rfind('/');
            const auto name = slash == std::string_view::npos ? path : path.substr(slash + 1);
            if (name == "sart" || name.starts_with("askpass.sart") || path.starts_with("main/usr/lib/sart/")) {
                return true;
            }
            for (const auto prefix : {"main/bin/", "main/sbin/", "main/usr/bin/", "main/usr/sbin/"}) {
                if (path.starts_with(prefix) && name.starts_with("sart"))
                    return true;
            }
            return false;
        }

    } // namespace

    InitramfsToolsSystemdContract plan_initramfs_tools_systemd(const InitramfsToolsSystemdFacts &facts,
                                                               std::string alternate_root) {
        if (!safe_root(alternate_root) || facts.architecture != product_architecture || facts.pid1_comm != "systemd" ||
            facts.kernel_versions.size() != 1 || !safe_token(facts.kernel_versions.front(), 128)) {
            throw std::runtime_error("initramfs-tools architecture, PID 1, root, or kernel is unsupported");
        }
        if (facts.root_filesystem_device == facts.boot_filesystem_device || !facts.boot_writable ||
            facts.boot_free_bytes < min_boot_free_bytes || facts.boot_free_inodes < min_boot_free_inodes) {
            throw std::runtime_error("initramfs-tools /boot does not satisfy the capacity contract");
        }
        const auto &kernel = facts.kernel_versions.front();
        const auto active = active_image(kernel);
        if (facts.known_good_path != active || facts.known_good_bytes == 0 ||
            facts.known_good_bytes > max_candidate_bytes) {
            throw std::runtime_error("initramfs-tools active image differs from the contract");
        }
        const std::set<std::string_view> required_tools{mkinitramfs,
                                                        unmkinitramfs,
                                                        findmnt,
                                                        systemd,
                                                        cryptsetup_executable(facts.cryptsetup_location),
                                                        grub_updater(facts.grub_regeneration),
                                                        grub_probe(facts.grub_regeneration)};
        std::map<std::string_view, const ToolFact *> tools;
        for (const auto &tool : facts.tools) {
            if (!tools.emplace(tool.path, &tool).second)
                throw std::runtime_error("duplicate initramfs-tools tool fact");
        }
        if (tools.size() != required_tools.size())
            throw std::runtime_error("initramfs-tools tool set differs");
        for (const auto path : required_tools) {
            const auto found = tools.find(path);
            if (found == tools.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || !found->second->executable) {
                throw std::runtime_error("unsafe initramfs-tools prerequisite: " + std::string(path));
            }
        }
        const std::map<std::string_view, bool> required_files{
            {"/usr/share/initramfs-tools/hook-functions", false},
            {"/usr/share/initramfs-tools/hooks/cryptroot", true},
            {"/usr/share/initramfs-tools/scripts/local-top/cryptroot", true},
            {"/usr/lib/cryptsetup/functions", false},
            {"/usr/lib/cryptsetup/askpass", true}};
        std::map<std::string_view, const InitramfsToolsPathFact *> files;
        for (const auto &file : facts.contract_files) {
            if (!files.emplace(file.path, &file).second)
                throw std::runtime_error("duplicate initramfs-tools file fact");
        }
        if (files.size() != required_files.size())
            throw std::runtime_error("initramfs-tools file set differs");
        for (const auto &[path, executable] : required_files) {
            const auto found = files.find(path);
            if (found == files.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || found->second->executable != executable) {
                throw std::runtime_error("unsafe initramfs-tools contract file: " + std::string(path));
            }
        }
        const auto candidate = candidate_image(kernel);
        const auto known_good = active + ".sart-known-good";
        InitramfsToolsSystemdContract contract{
            kernel,
            active,
            candidate,
            known_good,
            facts.known_good_digest,
            facts.grub_regeneration,
            "/etc/grub.d/41_sart_known_good",
            std::string(grub_config_path(facts.grub_regeneration)),
            render_grub_script(facts.boot_filesystem_uuid, kernel, facts.kernel_command_line, known_good),
            request(GeneratorKind::initramfs_tools, mkinitramfs, alternate_root, {"-o", candidate, kernel}),
            request(GeneratorKind::grub_update, grub_updater(facts.grub_regeneration), alternate_root,
                    grub_arguments(facts.grub_regeneration))};
        validate_initramfs_tools_systemd_contract(contract);
        return contract;
    }

    void validate_initramfs_tools_systemd_generator_request(const GeneratorRequest &value) {
        if (!value.clear_environment || !safe_root(value.alternate_root)) {
            throw std::runtime_error("initramfs-tools request requires a safe root and cleared environment");
        }
        if (value.generator == GeneratorKind::initramfs_tools && value.executable == mkinitramfs) {
            if (value.working_directory || value.arguments.size() != 3 || value.arguments[0] != "-o" ||
                candidate_kernel(value.arguments[1]) != std::optional<std::string_view>(value.arguments[2])) {
                throw std::runtime_error("mkinitramfs argv differs from the fixed contract");
            }
            return;
        }
        if (value.generator == GeneratorKind::initramfs_inspection && value.executable == unmkinitramfs) {
            constexpr std::string_view prefix = "/var/lib/sart/install/transactions/";
            constexpr std::string_view suffix = "/unpacked-candidate";
            if (value.working_directory || value.arguments.size() != 2 || !candidate_kernel(value.arguments[0]) ||
                !std::string_view(value.arguments[1]).starts_with(prefix) ||
                !std::string_view(value.arguments[1]).ends_with(suffix)) {
                throw std::runtime_error("unmkinitramfs request differs from the fixed contract");
            }
            const auto transaction =
                std::string_view(value.arguments[1])
                    .substr(prefix.size(), value.arguments[1].size() - prefix.size() - suffix.size());
            if (!safe_transaction(transaction))
                throw std::runtime_error("unmkinitramfs transaction is unsafe");
            return;
        }
        if (value.generator == GeneratorKind::grub_update) {
            for (const auto kind :
                 {GrubRegeneration::update_grub, GrubRegeneration::grub2_mkconfig, GrubRegeneration::grub_mkconfig}) {
                if (value.executable == grub_updater(kind) && !value.working_directory &&
                    value.arguments == grub_arguments(kind))
                    return;
            }
        }
        throw std::runtime_error("unreviewed initramfs-tools generator request");
    }

    void validate_initramfs_tools_systemd_contract(const InitramfsToolsSystemdContract &contract) {
        if (!safe_token(contract.kernel_version, 128) ||
            contract.active_image != active_image(contract.kernel_version) ||
            contract.candidate_image != candidate_image(contract.kernel_version) ||
            contract.known_good_image != contract.active_image + ".sart-known-good" ||
            contract.grub_script_path != "/etc/grub.d/41_sart_known_good" ||
            contract.grub_config_path != grub_config_path(contract.grub_regeneration) ||
            contract.update_grub.executable != grub_updater(contract.grub_regeneration) ||
            contract.update_grub.arguments != grub_arguments(contract.grub_regeneration) ||
            contract.generate.alternate_root != contract.update_grub.alternate_root) {
            throw std::runtime_error("initramfs-tools contract mixes incompatible capabilities");
        }
        validate_initramfs_tools_systemd_generator_request(contract.generate);
        validate_initramfs_tools_systemd_generator_request(contract.update_grub);
        if (contract.generate.arguments[1] != contract.candidate_image ||
            contract.generate.arguments[2] != contract.kernel_version) {
            throw std::runtime_error("initramfs-tools generation request is not bound to the contract");
        }
        const std::string script(reinterpret_cast<const char *>(contract.grub_script.data()),
                                 contract.grub_script.size());
        const auto initrd = std::string_view(contract.known_good_image).substr(6);
        if (!script.starts_with("#!/bin/sh\nset -eu\n") || !script.contains("initrd /" + std::string(initrd) + "\n") ||
            script.contains("@BOOT_UUID@") || script.contains("@KERNEL@") || script.contains("@CMDLINE@") ||
            script.contains("@INITRD@")) {
            throw std::runtime_error("initramfs-tools GRUB script is inconsistent");
        }
    }

    GeneratorRequest initramfs_tools_systemd_unpack_request(const InitramfsToolsSystemdContract &contract,
                                                            std::string_view transaction) {
        validate_initramfs_tools_systemd_contract(contract);
        if (!safe_transaction(transaction))
            throw std::runtime_error("unsafe initramfs-tools transaction id");
        auto value = request(GeneratorKind::initramfs_inspection, unmkinitramfs, contract.generate.alternate_root,
                             {contract.candidate_image, "/var/lib/sart/install/transactions/" +
                                                            std::string(transaction) + "/unpacked-candidate"});
        validate_initramfs_tools_systemd_generator_request(value);
        return value;
    }

    bool initramfs_tools_systemd_managed_image_path(std::string_view path) {
        if (path == "/etc/grub.d/41_sart_known_good" || path == "/boot/grub/grub.cfg" ||
            path == "/boot/grub2/grub.cfg" || candidate_kernel(path))
            return true;
        if (path.ends_with(".sart-known-good"))
            path.remove_suffix(std::string_view(".sart-known-good").size());
        return active_kernel(path).has_value();
    }

    std::vector<ArchiveEntry> collect_unpacked_initramfs_tools_inventory(std::string_view unpacked_root,
                                                                         std::uint32_t expected_owner_uid) {
        if (!safe_root(unpacked_root))
            throw std::runtime_error("unmkinitramfs root path is unsafe");
        struct stat root{};
        const std::string root_path(unpacked_root);
        if (::lstat(root_path.c_str(), &root) != 0 || !S_ISDIR(root.st_mode) || root.st_uid != expected_owner_uid ||
            (root.st_mode & 0077) != 0 || root.st_nlink < 2) {
            throw std::runtime_error("unmkinitramfs root is not owner-private");
        }
        std::vector<std::pair<std::string, std::string>> layers;
        for (const auto &entry : std::filesystem::directory_iterator(root_path)) {
            if (layers.size() >= 3)
                throw std::runtime_error("unmkinitramfs emitted too many layers");
            const auto name = entry.path().filename().string();
            if (name != "early" && name != "early2" && name != "main") {
                throw std::runtime_error("unmkinitramfs emitted an unreviewed layer");
            }
            struct stat before{};
            if (::lstat(entry.path().c_str(), &before) != 0 || !S_ISDIR(before.st_mode) ||
                before.st_uid != expected_owner_uid || (before.st_mode & 0022) != 0) {
                throw std::runtime_error("unmkinitramfs layer metadata is unsafe");
            }
            const int descriptor = ::open(entry.path().c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
            struct stat opened{};
            if (descriptor < 0 || ::fstat(descriptor, &opened) != 0 || opened.st_dev != before.st_dev ||
                opened.st_ino != before.st_ino || ::fchmod(descriptor, 0700) != 0) {
                if (descriptor >= 0)
                    ::close(descriptor);
                throw std::runtime_error("unmkinitramfs layer changed while opening");
            }
            ::close(descriptor);
            layers.emplace_back(name, entry.path().string());
        }
        std::ranges::sort(layers);
        if (std::ranges::find(layers, std::string("main"), &std::pair<std::string, std::string>::first) ==
            layers.end()) {
            throw std::runtime_error("unmkinitramfs output has no main layer");
        }
        std::vector<ArchiveEntry> inventory;
        for (const auto &[layer, path] : layers) {
            for (auto entry : collect_unpacked_archive_inventory(path, expected_owner_uid)) {
                if (inventory.size() >= max_archive_entries)
                    throw std::runtime_error("unmkinitramfs inventory is oversized");
                entry.path = layer + "/" + entry.path;
                inventory.push_back(std::move(entry));
            }
        }
        return inventory;
    }

    ArchiveInspection inspect_initramfs_tools_inventory(const std::vector<ArchiveEntry> &entries,
                                                        std::span<const std::byte> expected_sart) {
        validate_static_elf(expected_sart);
        const auto [seen, total] = common_inventory(entries);
        const auto sart = seen.find("main/usr/bin/sart");
        if (sart == seen.end() || sart->second->kind != ArchiveEntryKind::file || sart->second->mode != 0755 ||
            sart->second->bytes != std::vector<std::byte>(expected_sart.begin(), expected_sart.end())) {
            throw std::runtime_error("initramfs-tools contains the wrong Sart ELF");
        }
        const auto expected = expected_files();
        for (const auto &[path, resource] : expected) {
            const auto found = seen.find(path);
            if (found == seen.end() || found->second->kind != ArchiveEntryKind::file ||
                found->second->mode != resource.first || found->second->bytes != resource.second) {
                throw std::runtime_error("initramfs-tools resource changed: " + path);
            }
        }
        for (const auto path :
             {"main/init", "main/scripts/local-top/cryptroot", "main/usr/lib/cryptsetup/askpass.sart-console"})
            require_executable(seen, path);
        const auto functions = seen.find("main/usr/lib/cryptsetup/functions");
        if (functions == seen.end() || functions->second->kind != ArchiveEntryKind::file ||
            functions->second->bytes.empty()) {
            throw std::runtime_error("initramfs-tools cryptsetup functions are missing");
        }
        for (const auto &[path, entry] : seen) {
            if (!sart_namespace(path))
                continue;
            const bool allowed = path == "main/usr/bin/sart" || expected.contains(std::string(path)) ||
                                 path == "main/usr/lib/cryptsetup/askpass.sart-console";
            const bool executable_allowed = path == "main/usr/bin/sart" || path == "main/scripts/init-top/sart" ||
                                            path == "main/scripts/init-bottom/sart" ||
                                            path == "main/usr/lib/cryptsetup/askpass" ||
                                            path == "main/usr/lib/cryptsetup/askpass.sart-console";
            if (!allowed || path.contains("sart-init") ||
                (entry->kind == ArchiveEntryKind::file && (entry->mode & 0111) != 0 && !executable_allowed)) {
                throw std::runtime_error("unexpected Sart initramfs-tools member: " + std::string(path));
            }
        }
        return {sha256(expected_sart), entries.size(), total};
    }

    SartFreeArchiveInspection inspect_sart_free_initramfs_tools_inventory(const std::vector<ArchiveEntry> &entries) {
        const auto [seen, total] = common_inventory(entries);
        if (std::ranges::any_of(seen, [](const auto &item) { return sart_namespace(item.first); })) {
            throw std::runtime_error("Sart-free initramfs-tools archive contains a Sart member");
        }
        for (const auto path : {"main/init", "main/scripts/local-top/cryptroot", "main/usr/lib/cryptsetup/askpass"})
            require_executable(seen, path);
        const auto functions = seen.find("main/usr/lib/cryptsetup/functions");
        if (functions == seen.end() || functions->second->kind != ArchiveEntryKind::file ||
            functions->second->bytes.empty()) {
            throw std::runtime_error("Sart-free initramfs-tools functions are missing");
        }
        return {entries.size(), total};
    }

    DracutSystemdImageRecord verified_initramfs_tools_systemd_image_record(
        const InitramfsToolsSystemdContract &contract, std::span<const std::byte> candidate,
        const ArchiveInspection &inspection, std::span<const std::byte> expected_sart) {
        validate_initramfs_tools_systemd_contract(contract);
        if (candidate.empty() || candidate.size() > max_candidate_bytes ||
            inspection.sart_digest != sha256(expected_sart)) {
            throw std::runtime_error("initramfs-tools candidate is empty, oversized, or unbound");
        }
        const auto digest = sha256(candidate);
        DracutSystemdImageRecord record{contract.kernel_version,
                                        contract.active_image,
                                        digest,
                                        contract.candidate_image,
                                        digest,
                                        candidate.size(),
                                        contract.known_good_image,
                                        contract.known_good_digest,
                                        contract.grub_script_path,
                                        sha256(contract.grub_script),
                                        contract.grub_config_path,
                                        sha256(expected_sart)};
        validate_dracut_systemd_image_record(record);
        return record;
    }

} // namespace sart::install
