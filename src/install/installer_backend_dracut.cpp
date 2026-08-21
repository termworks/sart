#include "sart/install/backends.hpp"

#include "sart/core/sha256.hpp"
#include "sart/embedded/resources.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <dirent.h>
#include <fcntl.h>
#include <map>
#include <set>
#include <stdexcept>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

namespace sart::install {
    namespace {

        constexpr std::string_view dracut = "/usr/bin/dracut";
        constexpr std::string_view lsinitrd = "/usr/bin/lsinitrd";
        constexpr std::string_view findmnt = "/usr/bin/findmnt";
        constexpr std::string_view systemd = "/usr/lib/systemd/systemd";
        constexpr std::size_t max_special_reports = 32;

        class UniqueFd {
          public:
            explicit UniqueFd(int fd = -1) noexcept : fd_(fd) {}
            ~UniqueFd() {
                if (fd_ >= 0)
                    ::close(fd_);
            }
            UniqueFd(const UniqueFd &) = delete;
            UniqueFd &operator=(const UniqueFd &) = delete;
            UniqueFd(UniqueFd &&other) noexcept : fd_(other.fd_) { other.fd_ = -1; }
            UniqueFd &operator=(UniqueFd &&other) noexcept {
                if (this != &other) {
                    if (fd_ >= 0)
                        ::close(fd_);
                    fd_ = other.fd_;
                    other.fd_ = -1;
                }
                return *this;
            }
            [[nodiscard]] int get() const noexcept { return fd_; }

          private:
            int fd_;
        };

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

        std::string active_image(DracutImageLayout layout, std::string_view kernel) {
            if (layout == DracutImageLayout::initrd_img)
                return "/boot/initrd.img-" + std::string(kernel);
            return "/boot/initramfs-" + std::string(kernel) + ".img";
        }

        std::string candidate_image(DracutImageLayout layout, std::string_view kernel) {
            if (layout == DracutImageLayout::initrd_img)
                return "/boot/.sart-candidate-initrd.img-" + std::string(kernel);
            return "/boot/.sart-candidate-initramfs-" + std::string(kernel) + ".img";
        }

        std::optional<std::pair<DracutImageLayout, std::string_view>> active_parts(std::string_view path) {
            constexpr std::string_view initrd = "/boot/initrd.img-";
            constexpr std::string_view initramfs = "/boot/initramfs-";
            if (path.starts_with(initrd)) {
                const auto kernel = path.substr(initrd.size());
                if (safe_token(kernel, 128))
                    return std::pair{DracutImageLayout::initrd_img, kernel};
            }
            if (path.starts_with(initramfs) && path.ends_with(".img")) {
                const auto kernel = path.substr(initramfs.size(), path.size() - initramfs.size() - 4);
                if (safe_token(kernel, 128))
                    return std::pair{DracutImageLayout::initramfs_img, kernel};
            }
            return std::nullopt;
        }

        std::optional<std::pair<DracutImageLayout, std::string_view>> candidate_parts(std::string_view path) {
            constexpr std::string_view initrd = "/boot/.sart-candidate-initrd.img-";
            constexpr std::string_view initramfs = "/boot/.sart-candidate-initramfs-";
            if (path.starts_with(initrd)) {
                const auto kernel = path.substr(initrd.size());
                if (safe_token(kernel, 128))
                    return std::pair{DracutImageLayout::initrd_img, kernel};
            }
            if (path.starts_with(initramfs) && path.ends_with(".img")) {
                const auto kernel = path.substr(initramfs.size(), path.size() - initramfs.size() - 4);
                if (safe_token(kernel, 128))
                    return std::pair{DracutImageLayout::initramfs_img, kernel};
            }
            return std::nullopt;
        }

        GeneratorRequest request(GeneratorKind kind, std::string_view executable, std::string root,
                                 std::vector<std::string> arguments) {
            return {kind, std::string(executable), std::move(root), std::nullopt, std::move(arguments), true};
        }

        bool archive_path_safe(std::string_view path) {
            if (path.empty() || path.size() > 4096 || path.front() == '/' || path.contains('\0'))
                return false;
            std::size_t offset = 0;
            while (offset < path.size()) {
                const auto end = path.find('/', offset);
                const auto part =
                    path.substr(offset, end == std::string_view::npos ? path.size() - offset : end - offset);
                if (part.empty() || part == "." || part == "..")
                    return false;
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return true;
        }

        bool reviewed_device(const ArchiveEntry &entry) {
            static constexpr std::array devices{
                std::tuple<std::string_view, std::uint32_t, std::uint32_t>{"dev/console", 5, 1},
                std::tuple<std::string_view, std::uint32_t, std::uint32_t>{"dev/kmsg", 1, 11},
                std::tuple<std::string_view, std::uint32_t, std::uint32_t>{"dev/null", 1, 3},
                std::tuple<std::string_view, std::uint32_t, std::uint32_t>{"dev/random", 1, 8},
                std::tuple<std::string_view, std::uint32_t, std::uint32_t>{"dev/urandom", 1, 9}};
            return entry.kind == ArchiveEntryKind::character_device && entry.mode == 0644 && entry.bytes.empty() &&
                   std::ranges::find(devices, std::tuple{std::string_view(entry.path), entry.device_major,
                                                         entry.device_minor}) != devices.end();
        }

        std::pair<std::map<std::string_view, const ArchiveEntry *>, std::uint64_t>
        common_inventory(const std::vector<ArchiveEntry> &entries) {
            if (entries.size() > max_archive_entries)
                throw std::runtime_error("dracut archive contains too many entries");
            std::map<std::string_view, const ArchiveEntry *> seen;
            std::uint64_t total = 0;
            for (const auto &entry : entries) {
                if (!archive_path_safe(entry.path) || entry.mode > 07777 ||
                    (entry.kind == ArchiveEntryKind::character_device && !reviewed_device(entry))) {
                    throw std::runtime_error("unsafe dracut archive member: " + entry.path);
                }
                if (!seen.emplace(entry.path, &entry).second)
                    throw std::runtime_error("duplicate dracut archive member");
                if (entry.bytes.size() > max_inspected_archive_bytes - total)
                    throw std::runtime_error("dracut archive byte limit exceeded");
                total += entry.bytes.size();
            }
            const auto executable = [&seen](std::string_view path) {
                const auto found = seen.find(path);
                return found != seen.end() && found->second->kind == ArchiveEntryKind::file &&
                       (found->second->mode & 0111) != 0;
            };
            const bool has_crypt = executable("usr/lib/systemd/systemd-cryptsetup") ||
                                   executable("usr/bin/systemd-cryptsetup") || executable("usr/sbin/cryptsetup");
            if (!executable("usr/lib/systemd/systemd") || !has_crypt) {
                throw std::runtime_error("dracut archive lacks executable systemd/crypt support");
            }
            return {std::move(seen), total};
        }

        std::vector<std::byte> resource_bytes(embedded::TemplateId id) {
            const auto contents = embedded::template_resource(id).contents;
            return {reinterpret_cast<const std::byte *>(contents.data()),
                    reinterpret_cast<const std::byte *>(contents.data() + contents.size())};
        }

        std::map<std::string, std::pair<std::uint16_t, std::vector<std::byte>>> expected_systemd_files() {
            std::map<std::string, std::pair<std::uint16_t, std::vector<std::byte>>> expected;
            for (const auto id : {embedded::TemplateId::systemd_start_unit, embedded::TemplateId::systemd_show_unit,
                                  embedded::TemplateId::systemd_switch_root_unit,
                                  embedded::TemplateId::systemd_console_agent_drop_in}) {
                const auto resource = embedded::template_resource(id);
                std::string path(resource.materialization.path);
                if (path.starts_with('/'))
                    path.erase(path.begin());
                expected.emplace(std::move(path), std::pair{resource.materialization.mode, resource_bytes(id)});
            }
            return expected;
        }

        bool starts_with_sart_namespace(std::string_view path) { return path.contains("sart"); }

        std::string error_text(std::string_view operation) {
            return std::string(operation) + ": " + std::strerror(errno);
        }

    } // namespace

    DracutSystemdContract plan_dracut_systemd(const DracutSystemdFacts &facts, std::string alternate_root) {
        if (!safe_root(alternate_root))
            throw std::runtime_error("dracut-systemd generator alternate root is unsafe");
        if (facts.architecture != product_architecture || facts.pid1_comm != "systemd" ||
            facts.kernel_versions.size() != 1 || !safe_token(facts.kernel_versions.front(), 128)) {
            throw std::runtime_error("dracut-systemd architecture, PID 1, or kernel is unsupported");
        }
        if (facts.root_filesystem_device == facts.boot_filesystem_device || !facts.boot_writable ||
            facts.boot_free_bytes < min_boot_free_bytes || facts.boot_free_inodes < min_boot_free_inodes) {
            throw std::runtime_error("dracut-systemd /boot does not satisfy the separate writable capacity contract");
        }
        if (facts.known_good_bytes == 0 || facts.known_good_bytes > max_candidate_bytes) {
            throw std::runtime_error("dracut-systemd active image size is unsupported");
        }
        const auto &kernel = facts.kernel_versions.front();
        const auto active = active_image(facts.image_layout, kernel);
        if (facts.known_good_path != active)
            throw std::runtime_error("dracut-systemd active image is not canonical");
        const std::set<std::string> modules(facts.dracut_modules.begin(), facts.dracut_modules.end());
        if (!modules.contains("systemd") || !modules.contains("crypt")) {
            throw std::runtime_error("dracut-systemd lacks systemd or crypt support");
        }
        const std::set<std::string_view> required{dracut,
                                                  lsinitrd,
                                                  findmnt,
                                                  systemd,
                                                  cryptsetup_executable(facts.cryptsetup_location),
                                                  grub_updater(facts.grub_regeneration),
                                                  grub_probe(facts.grub_regeneration)};
        std::map<std::string_view, const ToolFact *> tools;
        for (const auto &tool : facts.tools) {
            if (!tools.emplace(tool.path, &tool).second)
                throw std::runtime_error("duplicate dracut tool fact");
        }
        if (tools.size() != required.size())
            throw std::runtime_error("dracut tool set differs from the contract");
        for (const auto path : required) {
            const auto found = tools.find(path);
            if (found == tools.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || !found->second->executable) {
                throw std::runtime_error("unsafe dracut prerequisite: " + std::string(path));
            }
        }
        const auto candidate = candidate_image(facts.image_layout, kernel);
        const auto known_good = active + ".sart-known-good";
        DracutSystemdContract contract{
            facts.image_layout,
            facts.grub_regeneration,
            kernel,
            active,
            candidate,
            known_good,
            facts.known_good_digest,
            "/etc/grub.d/41_sart_known_good",
            std::string(grub_config_path(facts.grub_regeneration)),
            render_grub_script(facts.boot_filesystem_uuid, kernel, facts.kernel_command_line, known_good),
            request(GeneratorKind::dracut, dracut, alternate_root,
                    {"--force", "--kver", kernel, "--add", "sart-systemd", candidate}),
            request(GeneratorKind::grub_update, grub_updater(facts.grub_regeneration), alternate_root,
                    grub_arguments(facts.grub_regeneration))};
        validate_dracut_systemd_contract(contract);
        return contract;
    }

    void validate_dracut_systemd_generator_request(const GeneratorRequest &value) {
        if (!value.clear_environment || !safe_root(value.alternate_root)) {
            throw std::runtime_error("dracut request requires a safe root and cleared environment");
        }
        if (value.generator == GeneratorKind::dracut && value.executable == dracut) {
            if (value.working_directory || value.arguments.size() != 6 || value.arguments[0] != "--force" ||
                value.arguments[1] != "--kver" || !safe_token(value.arguments[2], 128) ||
                (value.arguments[3] != "--add" && value.arguments[3] != "--omit") ||
                value.arguments[4] != "sart-systemd") {
                throw std::runtime_error("dracut argv differs from the fixed contract");
            }
            const auto parts = candidate_parts(value.arguments[5]);
            if (!parts || parts->second != value.arguments[2])
                throw std::runtime_error("dracut candidate is unsafe");
            return;
        }
        if (value.generator == GeneratorKind::initramfs_inspection && value.executable == lsinitrd) {
            if (value.arguments.size() != 2 || value.arguments[0] != "--unpack" ||
                !candidate_parts(value.arguments[1]) || !value.working_directory ||
                !value.working_directory->starts_with("/var/lib/sart/install/transactions/") ||
                !value.working_directory->ends_with("/unpacked-candidate")) {
                throw std::runtime_error("lsinitrd request differs from the fixed contract");
            }
            const auto transaction = std::string_view(*value.working_directory)
                                         .substr(std::string_view("/var/lib/sart/install/transactions/").size(),
                                                 value.working_directory->size() -
                                                     std::string_view("/var/lib/sart/install/transactions/").size() -
                                                     std::string_view("/unpacked-candidate").size());
            if (!safe_transaction(transaction))
                throw std::runtime_error("lsinitrd transaction id is unsafe");
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
        throw std::runtime_error("unreviewed dracut generator request");
    }

    void validate_dracut_systemd_contract(const DracutSystemdContract &contract) {
        if (!safe_token(contract.kernel_version, 128) ||
            contract.active_image != active_image(contract.image_layout, contract.kernel_version) ||
            contract.candidate_image != candidate_image(contract.image_layout, contract.kernel_version) ||
            contract.known_good_image != contract.active_image + ".sart-known-good" ||
            contract.grub_script_path != "/etc/grub.d/41_sart_known_good" ||
            contract.grub_config_path != grub_config_path(contract.grub_regeneration) ||
            contract.update_grub.executable != grub_updater(contract.grub_regeneration) ||
            contract.update_grub.arguments != grub_arguments(contract.grub_regeneration) ||
            contract.generate.alternate_root != contract.update_grub.alternate_root) {
            throw std::runtime_error("dracut contract mixes incompatible capabilities");
        }
        validate_dracut_systemd_generator_request(contract.generate);
        validate_dracut_systemd_generator_request(contract.update_grub);
        if (contract.generate.arguments[2] != contract.kernel_version ||
            contract.generate.arguments[5] != contract.candidate_image) {
            throw std::runtime_error("dracut generation request is not bound to the contract");
        }
        const std::string script(reinterpret_cast<const char *>(contract.grub_script.data()),
                                 contract.grub_script.size());
        const auto initrd = std::string_view(contract.known_good_image).substr(6);
        if (!script.starts_with("#!/bin/sh\nset -eu\n") || !script.contains("initrd /" + std::string(initrd) + "\n") ||
            script.contains("@BOOT_UUID@") || script.contains("@KERNEL@") || script.contains("@CMDLINE@") ||
            script.contains("@INITRD@")) {
            throw std::runtime_error("dracut GRUB script is inconsistent");
        }
    }

    GeneratorRequest dracut_systemd_unpack_request(const DracutSystemdContract &contract,
                                                   std::string_view transaction) {
        validate_dracut_systemd_contract(contract);
        if (!safe_transaction(transaction))
            throw std::runtime_error("unsafe dracut transaction id");
        auto value = request(GeneratorKind::initramfs_inspection, lsinitrd, contract.generate.alternate_root,
                             {"--unpack", contract.candidate_image});
        value.working_directory =
            "/var/lib/sart/install/transactions/" + std::string(transaction) + "/unpacked-candidate";
        validate_dracut_systemd_generator_request(value);
        return value;
    }

    bool dracut_systemd_managed_image_path(std::string_view path) {
        if (path == "/etc/grub.d/41_sart_known_good" || path == "/boot/grub/grub.cfg" ||
            path == "/boot/grub2/grub.cfg" || candidate_parts(path))
            return true;
        if (path.ends_with(".sart-known-good"))
            path.remove_suffix(std::string_view(".sart-known-good").size());
        return active_parts(path).has_value();
    }

    ArchiveInspection inspect_dracut_inventory(const std::vector<ArchiveEntry> &entries,
                                               std::span<const std::byte> expected_sart) {
        validate_static_elf(expected_sart);
        const auto [seen, total] = common_inventory(entries);
        const auto sart = seen.find("usr/bin/sart");
        if (sart == seen.end() || sart->second->kind != ArchiveEntryKind::file || sart->second->mode != 0755 ||
            sart->second->bytes != std::vector<std::byte>(expected_sart.begin(), expected_sart.end())) {
            throw std::runtime_error("dracut contains the wrong Sart ELF");
        }
        validate_static_elf(sart->second->bytes);
        const auto expected = expected_systemd_files();
        for (const auto &[path, resource] : expected) {
            const auto found = seen.find(path);
            if (found == seen.end() || found->second->kind != ArchiveEntryKind::file ||
                found->second->mode != resource.first || found->second->bytes != resource.second) {
                throw std::runtime_error("dracut resource changed: " + path);
            }
        }
        for (const auto &[path, target] :
             std::array{std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd.target.wants/sart-start.service", "../sart-start.service"},
                        std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd.target.wants/sart-show.service", "../sart-show.service"},
                        std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd-switch-root.target.wants/sart-switch-root.service",
                            "../sart-switch-root.service"}}) {
            const auto found = seen.find(path);
            const std::vector<std::byte> bytes{reinterpret_cast<const std::byte *>(target.data()),
                                               reinterpret_cast<const std::byte *>(target.data() + target.size())};
            if (found == seen.end() || found->second->kind != ArchiveEntryKind::symlink ||
                found->second->bytes != bytes) {
                throw std::runtime_error("dracut activation link changed: " + std::string(path));
            }
        }
        for (const auto &[path, entry] : seen) {
            if (!starts_with_sart_namespace(path))
                continue;
            const bool allowed =
                path == "usr/bin/sart" || expected.contains(std::string(path)) ||
                path == "usr/lib/systemd/system/initrd.target.wants/sart-start.service" ||
                path == "usr/lib/systemd/system/initrd.target.wants/sart-show.service" ||
                path == "usr/lib/systemd/system/initrd-switch-root.target.wants/sart-switch-root.service";
            if (!allowed || path.contains("sart-init") ||
                (entry->kind == ArchiveEntryKind::file && (entry->mode & 0111) != 0 && path != "usr/bin/sart")) {
                throw std::runtime_error("unexpected Sart dracut member: " + std::string(path));
            }
        }
        return {sha256(expected_sart), entries.size(), total};
    }

    SartFreeArchiveInspection inspect_sart_free_dracut_inventory(const std::vector<ArchiveEntry> &entries) {
        const auto [seen, total] = common_inventory(entries);
        if (std::ranges::any_of(seen, [](const auto &item) { return item.first.contains("sart"); })) {
            throw std::runtime_error("Sart-free dracut archive contains a Sart member");
        }
        return {entries.size(), total};
    }

    DracutSystemdImageRecord verified_dracut_systemd_image_record(const DracutSystemdContract &contract,
                                                                  std::span<const std::byte> candidate,
                                                                  const ArchiveInspection &inspection,
                                                                  std::span<const std::byte> expected_sart) {
        validate_dracut_systemd_contract(contract);
        if (candidate.empty() || candidate.size() > max_candidate_bytes ||
            inspection.sart_digest != sha256(expected_sart)) {
            throw std::runtime_error("dracut candidate is empty, oversized, or unbound");
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

    void validate_dracut_systemd_image_record(const DracutSystemdImageRecord &record) {
        const auto active = active_parts(record.active_image);
        const auto candidate = candidate_parts(record.candidate_image);
        if (!active || !candidate || !safe_token(record.kernel_version, 128) || active->first != candidate->first ||
            active->second != record.kernel_version || candidate->second != record.kernel_version ||
            record.known_good_image != record.active_image + ".sart-known-good" ||
            record.grub_script_path != "/etc/grub.d/41_sart_known_good" ||
            (record.grub_config_path != "/boot/grub/grub.cfg" && record.grub_config_path != "/boot/grub2/grub.cfg") ||
            record.candidate_bytes == 0 || record.candidate_bytes > max_candidate_bytes ||
            record.active_digest != record.candidate_digest) {
            throw std::runtime_error("dracut image record violates the fixed path and hash contract");
        }
    }

    GeneratorRequest dracut_systemd_sart_free_generate_request(const DracutSystemdImageRecord &record,
                                                               std::string alternate_root) {
        validate_dracut_systemd_image_record(record);
        if (!safe_root(alternate_root))
            throw std::runtime_error("unsafe dracut alternate root");
        auto value =
            request(GeneratorKind::dracut, dracut, std::move(alternate_root),
                    {"--force", "--kver", record.kernel_version, "--omit", "sart-systemd", record.candidate_image});
        validate_dracut_systemd_generator_request(value);
        return value;
    }

    GeneratorRequest dracut_systemd_sart_free_unpack_request(const DracutSystemdImageRecord &record,
                                                             std::string_view transaction, std::string alternate_root) {
        validate_dracut_systemd_image_record(record);
        if (!safe_root(alternate_root) || !safe_transaction(transaction)) {
            throw std::runtime_error("unsafe dracut uninstall inspection context");
        }
        auto value = request(GeneratorKind::initramfs_inspection, lsinitrd, std::move(alternate_root),
                             {"--unpack", record.candidate_image});
        value.working_directory =
            "/var/lib/sart/install/transactions/" + std::string(transaction) + "/unpacked-candidate";
        validate_dracut_systemd_generator_request(value);
        return value;
    }

    std::vector<ArchiveEntry> collect_unpacked_archive_inventory(std::string_view unpacked_root,
                                                                 std::uint32_t expected_owner_uid) {
        if (!safe_root(unpacked_root))
            throw std::runtime_error("unpacked archive root path is unsafe");
        const std::string root_path(unpacked_root);
        struct stat before{};
        if (::lstat(root_path.c_str(), &before) != 0)
            throw std::runtime_error(error_text("inspect unpacked root"));
        if (!S_ISDIR(before.st_mode) || before.st_uid != expected_owner_uid || (before.st_mode & 07777) != 0700) {
            throw std::runtime_error("unpacked archive root is not a private owned mode-0700 directory");
        }
        UniqueFd root(::open(root_path.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
        if (root.get() < 0)
            throw std::runtime_error(error_text("open unpacked root"));
        struct stat opened{};
        if (::fstat(root.get(), &opened) != 0 || opened.st_dev != before.st_dev || opened.st_ino != before.st_ino) {
            throw std::runtime_error("unpacked archive root identity changed while opening");
        }
        struct Pending {
            std::string prefix;
            UniqueFd directory;
        };
        std::vector<Pending> pending;
        pending.push_back({{}, std::move(root)});
        std::vector<ArchiveEntry> entries;
        std::set<std::string> seen;
        std::vector<std::string> special;
        std::uint64_t total = 0;
        while (!pending.empty()) {
            auto current = std::move(pending.back());
            pending.pop_back();
            const int duplicate = ::dup(current.directory.get());
            if (duplicate < 0)
                throw std::runtime_error(error_text("duplicate archive directory"));
            DIR *raw = ::fdopendir(duplicate);
            if (raw == nullptr) {
                ::close(duplicate);
                throw std::runtime_error(error_text("enumerate archive directory"));
            }
            std::vector<std::string> names;
            errno = 0;
            while (const auto *item = ::readdir(raw)) {
                if (std::string_view(item->d_name) != "." && std::string_view(item->d_name) != "..")
                    names.emplace_back(item->d_name);
            }
            const int read_error = errno;
            ::closedir(raw);
            if (read_error != 0) {
                errno = read_error;
                throw std::runtime_error(error_text("read archive directory"));
            }
            std::ranges::sort(names);
            for (const auto &name : names) {
                if (entries.size() >= max_archive_entries)
                    throw std::runtime_error("archive contains too many entries");
                const std::string path = current.prefix.empty() ? name : current.prefix + "/" + name;
                if (!archive_path_safe(path) || !seen.insert(path).second)
                    throw std::runtime_error("unsafe archive member path");
                struct stat stat_before{};
                if (::fstatat(current.directory.get(), name.c_str(), &stat_before, AT_SYMLINK_NOFOLLOW) != 0) {
                    throw std::runtime_error(error_text("inspect archive member"));
                }
                if (stat_before.st_uid != expected_owner_uid)
                    throw std::runtime_error("unowned archive member: " + path);
                const auto mode = static_cast<std::uint16_t>(stat_before.st_mode & 07777);
                if (S_ISDIR(stat_before.st_mode)) {
                    UniqueFd child(::openat(current.directory.get(), name.c_str(),
                                            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
                    struct stat after{};
                    if (child.get() < 0 || ::fstat(child.get(), &after) != 0 || after.st_dev != stat_before.st_dev ||
                        after.st_ino != stat_before.st_ino || !S_ISDIR(after.st_mode)) {
                        throw std::runtime_error("archive directory changed while opening: " + path);
                    }
                    entries.push_back({path, ArchiveEntryKind::directory, mode, {}, 0, 0});
                    pending.push_back({path, std::move(child)});
                } else if (S_ISREG(stat_before.st_mode)) {
                    if (stat_before.st_size < 0 ||
                        static_cast<std::uint64_t>(stat_before.st_size) > max_inspected_archive_bytes - total)
                        throw std::runtime_error("archive byte limit exceeded");
                    const auto size = static_cast<std::size_t>(stat_before.st_size);
                    UniqueFd file(::openat(current.directory.get(), name.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC));
                    struct stat after{};
                    if (file.get() < 0 || ::fstat(file.get(), &after) != 0 || after.st_dev != stat_before.st_dev ||
                        after.st_ino != stat_before.st_ino || after.st_size != stat_before.st_size ||
                        !S_ISREG(after.st_mode)) {
                        throw std::runtime_error("archive file changed while opening: " + path);
                    }
                    std::vector<std::byte> bytes(size);
                    std::size_t offset = 0;
                    while (offset < bytes.size()) {
                        const auto count = ::read(file.get(), bytes.data() + offset, bytes.size() - offset);
                        if (count < 0 && errno == EINTR)
                            continue;
                        if (count <= 0)
                            throw std::runtime_error("archive file changed while reading: " + path);
                        offset += static_cast<std::size_t>(count);
                    }
                    std::byte extra{};
                    if (::read(file.get(), &extra, 1) != 0)
                        throw std::runtime_error("archive file grew while reading: " + path);
                    total += bytes.size();
                    entries.push_back({path, ArchiveEntryKind::file, mode, std::move(bytes), 0, 0});
                } else if (S_ISLNK(stat_before.st_mode)) {
                    std::array<char, 4097> target{};
                    const auto length =
                        ::readlinkat(current.directory.get(), name.c_str(), target.data(), target.size());
                    if (length < 0 || static_cast<std::size_t>(length) >= target.size() ||
                        static_cast<std::uint64_t>(length) > max_inspected_archive_bytes - total) {
                        throw std::runtime_error("archive symlink is invalid: " + path);
                    }
                    struct stat after{};
                    if (::fstatat(current.directory.get(), name.c_str(), &after, AT_SYMLINK_NOFOLLOW) != 0 ||
                        after.st_dev != stat_before.st_dev || after.st_ino != stat_before.st_ino ||
                        !S_ISLNK(after.st_mode)) {
                        throw std::runtime_error("archive symlink changed while reading: " + path);
                    }
                    std::vector<std::byte> bytes(reinterpret_cast<const std::byte *>(target.data()),
                                                 reinterpret_cast<const std::byte *>(target.data() + length));
                    total += bytes.size();
                    entries.push_back({path, ArchiveEntryKind::symlink, mode, std::move(bytes), 0, 0});
                } else if (S_ISCHR(stat_before.st_mode)) {
                    ArchiveEntry entry{path,
                                       ArchiveEntryKind::character_device,
                                       mode,
                                       {},
                                       static_cast<std::uint32_t>(major(stat_before.st_rdev)),
                                       static_cast<std::uint32_t>(minor(stat_before.st_rdev))};
                    if (expected_owner_uid != 0 || stat_before.st_gid != 0 || !reviewed_device(entry)) {
                        if (special.size() >= max_special_reports)
                            throw std::runtime_error("too many unreviewed archive nodes");
                        special.push_back(path);
                        continue;
                    }
                    struct stat after{};
                    if (::fstatat(current.directory.get(), name.c_str(), &after, AT_SYMLINK_NOFOLLOW) != 0 ||
                        after.st_dev != stat_before.st_dev || after.st_ino != stat_before.st_ino ||
                        after.st_mode != stat_before.st_mode || after.st_uid != stat_before.st_uid ||
                        after.st_gid != stat_before.st_gid || after.st_rdev != stat_before.st_rdev) {
                        throw std::runtime_error("archive character device changed: " + path);
                    }
                    entries.push_back(std::move(entry));
                } else {
                    if (special.size() >= max_special_reports)
                        throw std::runtime_error("too many unreviewed archive nodes");
                    special.push_back(path);
                }
            }
        }
        if (!special.empty())
            throw std::runtime_error("archive contains unreviewed special nodes");
        std::ranges::sort(entries, {}, &ArchiveEntry::path);
        return entries;
    }

} // namespace sart::install
