#include "sart/installer_backends.hpp"

#include "sart/embedded.hpp"
#include "sart/sha256.hpp"

#include <algorithm>
#include <array>
#include <map>
#include <set>
#include <sstream>
#include <stdexcept>

namespace sart::install {
    namespace {

        constexpr std::string_view mkinitcpio = "/usr/bin/mkinitcpio";
        constexpr std::string_view lsinitcpio = "/usr/bin/lsinitcpio";
        constexpr std::string_view config_path = "/etc/mkinitcpio.conf";

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

        std::string active_image(std::string_view package) {
            return "/boot/initramfs-" + std::string(package) + ".img";
        }

        std::string candidate_image(std::string_view package) {
            return "/boot/.sart-candidate-initramfs-" + std::string(package) + ".img";
        }

        std::optional<std::string_view> candidate_package(std::string_view path) {
            constexpr std::string_view prefix = "/boot/.sart-candidate-initramfs-";
            if (!path.starts_with(prefix) || !path.ends_with(".img"))
                return std::nullopt;
            const auto package = path.substr(prefix.size(), path.size() - prefix.size() - 4);
            return safe_token(package, 64) ? std::optional(package) : std::nullopt;
        }

        bool safe_transaction(std::string_view value) {
            return !value.empty() && value.size() <= 128 && std::ranges::all_of(value, [](unsigned char byte) {
                return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z') ||
                       byte == '-';
            });
        }

        std::string preset_path(std::string_view package) {
            return "/etc/mkinitcpio.d/" + std::string(package) + ".preset";
        }

        void validate_preset(std::string_view source, std::string_view package) {
            if (source.empty() || source.size() > 65536 || source.contains('\0')) {
                throw std::runtime_error("mkinitcpio preset is empty or oversized");
            }
            const std::array kernels{"ALL_kver='/boot/vmlinuz-" + std::string(package) + "'",
                                     "ALL_kver=\"/boot/vmlinuz-" + std::string(package) + "\""};
            const std::array images{"default_image='/boot/initramfs-" + std::string(package) + ".img'",
                                    "default_image=\"/boot/initramfs-" + std::string(package) + ".img\""};
            std::size_t kernel_count = 0, image_count = 0, preset_count = 0;
            std::istringstream input{std::string(source)};
            std::string line;
            while (std::getline(input, line)) {
                const auto begin = line.find_first_not_of(" \t");
                const auto end = line.find_last_not_of(" \t\r");
                const auto trimmed = begin == std::string::npos ? std::string{} : line.substr(begin, end - begin + 1);
                kernel_count += std::ranges::find(kernels, trimmed) != kernels.end();
                image_count += std::ranges::find(images, trimmed) != images.end();
                preset_count += trimmed == "PRESETS=('default')" || trimmed == "PRESETS=(\"default\")" ||
                                trimmed == "PRESETS=('default' 'fallback')" ||
                                trimmed == "PRESETS=(\"default\" \"fallback\")";
            }
            if (kernel_count != 1 || image_count != 1 || preset_count != 1) {
                throw std::runtime_error("mkinitcpio preset differs from the reviewed contract");
            }
        }

        std::vector<std::byte> bytes(std::string_view text) {
            return {reinterpret_cast<const std::byte *>(text.data()),
                    reinterpret_cast<const std::byte *>(text.data() + text.size())};
        }

    } // namespace

    std::expected<std::string, std::string> activate_mkinitcpio_hooks(std::string_view source) {
        if (source.empty() || source.size() > 65536 || source.contains('\0')) {
            return std::unexpected("mkinitcpio configuration is empty or oversized");
        }
        std::istringstream input{std::string(source)};
        std::vector<std::string> lines;
        std::string line;
        std::optional<std::size_t> hook_line;
        while (std::getline(input, line)) {
            lines.push_back(line);
            auto trimmed = std::string_view(lines.back());
            trimmed.remove_prefix(std::min(trimmed.find_first_not_of(" \t"), trimmed.size()));
            if (trimmed.starts_with("HOOKS=")) {
                if (hook_line)
                    return std::unexpected("mkinitcpio configuration has multiple HOOKS assignments");
                hook_line = lines.size() - 1;
            }
        }
        if (!hook_line)
            return std::unexpected("mkinitcpio configuration has no HOOKS assignment");
        auto trimmed = std::string_view(lines[*hook_line]);
        const auto indent_size = std::min(trimmed.find_first_not_of(" \t"), trimmed.size());
        const auto indent = trimmed.substr(0, indent_size);
        trimmed.remove_prefix(indent_size);
        if (!trimmed.starts_with("HOOKS=(") || !trimmed.ends_with(')')) {
            return std::unexpected("mkinitcpio HOOKS uses an unsupported array spelling");
        }
        trimmed.remove_prefix(7);
        trimmed.remove_suffix(1);
        std::istringstream hook_input{std::string(trimmed)};
        std::vector<std::string> hooks;
        while (hook_input >> line) {
            if (!safe_token(line, 64))
                return std::unexpected("mkinitcpio HOOKS contains an unsafe token");
            hooks.push_back(line);
        }
        const std::set<std::string> unique(hooks.begin(), hooks.end());
        if (hooks.empty() || unique.size() != hooks.size())
            return std::unexpected("mkinitcpio HOOKS contains a duplicate hook");
        if (unique.contains("systemd") || unique.contains("sd-encrypt")) {
            return std::unexpected("mkinitcpio HOOKS is not the BusyBox encrypt mechanism");
        }
        const auto position = [&hooks](std::string_view value) -> std::optional<std::size_t> {
            const auto at = std::ranges::find(hooks, value);
            return at == hooks.end() ? std::nullopt : std::optional<std::size_t>(at - hooks.begin());
        };
        const auto base = position("base"), udev = position("udev"), block = position("block");
        const auto encrypt = position("encrypt"), filesystems = position("filesystems"), fsck = position("fsck");
        if (!base || !udev || !block || !encrypt || !filesystems || !fsck ||
            !(*base < *udev && *udev < *block && *block < *encrypt && *encrypt < *filesystems &&
              *filesystems < *fsck)) {
            return std::unexpected("mkinitcpio HOOKS ordering differs from the reviewed contract");
        }
        if (const auto sart = position("sart")) {
            if (*sart != *encrypt + 1)
                return std::unexpected("mkinitcpio sart hook is in an unsafe position");
            return std::string(source);
        }
        hooks.insert(hooks.begin() + static_cast<std::ptrdiff_t>(*encrypt + 1), "sart");
        lines[*hook_line] = std::string(indent) + "HOOKS=(";
        for (std::size_t index = 0; index < hooks.size(); ++index) {
            if (index != 0)
                lines[*hook_line] += ' ';
            lines[*hook_line] += hooks[index];
        }
        lines[*hook_line] += ')';
        std::string output;
        for (const auto &output_line : lines)
            output += output_line + '\n';
        if (!source.ends_with('\n'))
            output.pop_back();
        return output;
    }

    MkinitcpioSystemdContract plan_mkinitcpio_systemd(const MkinitcpioSystemdFacts &facts, std::string alternate_root) {
        if (!safe_root(alternate_root) || facts.architecture != product_architecture || facts.pid1_comm != "systemd" ||
            facts.kernel_versions.size() != 1 || !safe_token(facts.kernel_versions.front(), 128) ||
            !safe_token(facts.package_base, 64)) {
            throw std::runtime_error("mkinitcpio architecture, PID 1, kernel, or package base is unsupported");
        }
        if (facts.root_filesystem_device == facts.boot_filesystem_device || !facts.boot_writable ||
            facts.boot_free_bytes < min_boot_free_bytes || facts.boot_free_inodes < min_boot_free_inodes) {
            throw std::runtime_error("mkinitcpio /boot does not satisfy the separate writable capacity contract");
        }
        const auto active = active_image(facts.package_base);
        if (facts.known_good_path != active || facts.known_good_bytes == 0 ||
            facts.known_good_bytes > max_candidate_bytes || facts.config_mode != 0644) {
            throw std::runtime_error("mkinitcpio active image or configuration differs from the contract");
        }
        validate_preset(facts.preset_source, facts.package_base);
        const auto activated = activate_mkinitcpio_hooks(facts.config_source);
        if (!activated)
            throw std::runtime_error(activated.error());

        const std::set<std::string_view> required_tools{mkinitcpio,
                                                        lsinitcpio,
                                                        "/usr/bin/findmnt",
                                                        "/usr/lib/systemd/systemd",
                                                        "/usr/bin/grub-mkconfig",
                                                        "/usr/bin/grub-probe",
                                                        cryptsetup_executable(facts.cryptsetup_location)};
        std::map<std::string_view, const ToolFact *> tools;
        for (const auto &tool : facts.tools) {
            if (!tools.emplace(tool.path, &tool).second)
                throw std::runtime_error("duplicate mkinitcpio tool fact");
        }
        if (tools.size() != required_tools.size())
            throw std::runtime_error("mkinitcpio tool set differs from the contract");
        for (const auto required : required_tools) {
            const auto found = tools.find(required);
            if (found == tools.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || !found->second->executable) {
                throw std::runtime_error("unsafe mkinitcpio prerequisite: " + std::string(required));
            }
        }
        const std::map<std::string_view, bool> required_files{{"/usr/lib/initcpio/functions", true},
                                                              {"/usr/lib/initcpio/init", false},
                                                              {"/usr/lib/initcpio/hooks/encrypt", false},
                                                              {"/usr/lib/initcpio/install/encrypt", false}};
        std::map<std::string_view, const MkinitcpioPathFact *> files;
        for (const auto &file : facts.contract_files) {
            if (!files.emplace(file.path, &file).second)
                throw std::runtime_error("duplicate mkinitcpio contract file");
        }
        if (files.size() != required_files.size())
            throw std::runtime_error("mkinitcpio contract file set differs");
        for (const auto &[path, executable] : required_files) {
            const auto found = files.find(path);
            if (found == files.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || found->second->executable != executable) {
                throw std::runtime_error("unsafe mkinitcpio contract file: " + std::string(path));
            }
        }

        const auto candidate = candidate_image(facts.package_base);
        const auto known_good = active + ".sart-known-good";
        const auto grub_script =
            render_grub_script(facts.boot_filesystem_uuid, facts.package_base, facts.kernel_command_line, known_good);
        MkinitcpioSystemdContract contract{facts.kernel_versions.front(),
                                           facts.package_base,
                                           preset_path(facts.package_base),
                                           active,
                                           candidate,
                                           known_good,
                                           facts.known_good_digest,
                                           std::string(config_path),
                                           facts.config_mode,
                                           bytes(facts.config_source),
                                           bytes(*activated),
                                           *activated == facts.config_source,
                                           GrubRegeneration::grub_mkconfig,
                                           "/etc/grub.d/41_sart_known_good",
                                           "/boot/grub/grub.cfg",
                                           grub_script,
                                           {GeneratorKind::mkinitcpio,
                                            std::string(mkinitcpio),
                                            std::move(alternate_root),
                                            std::nullopt,
                                            {"-k", facts.kernel_versions.front(), "-g", candidate},
                                            true},
                                           {GeneratorKind::grub_update,
                                            "/usr/bin/grub-mkconfig",
                                            "/",
                                            std::nullopt,
                                            {"-o", "/boot/grub/grub.cfg"},
                                            true}};
        contract.update_grub.alternate_root = contract.generate.alternate_root;
        validate_mkinitcpio_systemd_contract(contract);
        return contract;
    }

    void validate_mkinitcpio_systemd_contract(const MkinitcpioSystemdContract &contract) {
        if (!safe_token(contract.kernel_version, 128) || !safe_token(contract.package_base, 64) ||
            contract.preset_path != preset_path(contract.package_base) ||
            contract.active_image != active_image(contract.package_base) ||
            contract.candidate_image != candidate_image(contract.package_base) ||
            contract.known_good_image != contract.active_image + ".sart-known-good" ||
            contract.config_path != config_path || contract.config_mode != 0644 ||
            contract.grub_regeneration != GrubRegeneration::grub_mkconfig ||
            contract.grub_script_path != "/etc/grub.d/41_sart_known_good" ||
            contract.grub_config_path != "/boot/grub/grub.cfg" ||
            contract.generate.generator != GeneratorKind::mkinitcpio || contract.generate.executable != mkinitcpio ||
            contract.generate.working_directory ||
            contract.generate.arguments !=
                std::vector<std::string>{"-k", contract.kernel_version, "-g", contract.candidate_image} ||
            contract.update_grub.generator != GeneratorKind::grub_update ||
            contract.update_grub.executable != "/usr/bin/grub-mkconfig" ||
            contract.update_grub.arguments != std::vector<std::string>{"-o", "/boot/grub/grub.cfg"} ||
            contract.generate.alternate_root != contract.update_grub.alternate_root) {
            throw std::runtime_error("mkinitcpio contract mixes incompatible capabilities");
        }
        const std::string original(reinterpret_cast<const char *>(contract.config_original.data()),
                                   contract.config_original.size());
        const auto activated = activate_mkinitcpio_hooks(original);
        if (!activated || bytes(*activated) != contract.config_activated ||
            (*activated == original) != contract.config_already_active) {
            throw std::runtime_error("mkinitcpio configuration activation is not reproducible");
        }
        validate_mkinitcpio_systemd_generator_request(contract.generate);
        validate_mkinitcpio_systemd_generator_request(contract.update_grub);
    }

    void validate_mkinitcpio_systemd_generator_request(const GeneratorRequest &value) {
        if (!value.clear_environment || !safe_root(value.alternate_root)) {
            throw std::runtime_error("mkinitcpio request requires a safe root and cleared environment");
        }
        if (value.generator == GeneratorKind::mkinitcpio && value.executable == mkinitcpio) {
            if (value.working_directory || value.arguments.size() != 4 || value.arguments[0] != "-k" ||
                !safe_token(value.arguments[1], 128) || value.arguments[2] != "-g" ||
                !candidate_package(value.arguments[3])) {
                throw std::runtime_error("mkinitcpio argv differs from the fixed contract");
            }
            return;
        }
        if (value.generator == GeneratorKind::initramfs_inspection && value.executable == lsinitcpio) {
            constexpr std::string_view prefix = "/var/lib/sart/install/transactions/";
            constexpr std::string_view suffix = "/unpacked-candidate";
            if (value.arguments.size() != 2 || value.arguments[0] != "-x" || !value.working_directory ||
                !std::string_view(*value.working_directory).starts_with(prefix) ||
                !std::string_view(*value.working_directory).ends_with(suffix)) {
                throw std::runtime_error("lsinitcpio request differs from the fixed contract");
            }
            const auto transaction =
                std::string_view(*value.working_directory)
                    .substr(prefix.size(), value.working_directory->size() - prefix.size() - suffix.size());
            if (!safe_transaction(transaction))
                throw std::runtime_error("lsinitcpio transaction id is unsafe");
            return;
        }
        if (value.generator == GeneratorKind::grub_update && value.executable == "/usr/bin/grub-mkconfig" &&
            !value.working_directory && value.arguments == std::vector<std::string>{"-o", "/boot/grub/grub.cfg"})
            return;
        throw std::runtime_error("unreviewed mkinitcpio generator request");
    }

    GeneratorRequest mkinitcpio_unpack_request(const MkinitcpioSystemdContract &contract,
                                               std::string_view transaction) {
        validate_mkinitcpio_systemd_contract(contract);
        if (!safe_token(transaction, 128))
            throw std::runtime_error("unsafe inspection transaction id");
        GeneratorRequest request{GeneratorKind::initramfs_inspection,
                                 std::string(lsinitcpio),
                                 contract.generate.alternate_root,
                                 "/var/lib/sart/install/transactions/" + std::string(transaction) +
                                     "/unpacked-candidate",
                                 {"-x", contract.candidate_image},
                                 true};
        validate_mkinitcpio_systemd_generator_request(request);
        return request;
    }

    ArchiveInspection inspect_mkinitcpio_inventory(const std::vector<ArchiveEntry> &entries,
                                                   std::span<const std::byte> expected_sart) {
        validate_static_elf(expected_sart);
        if (entries.empty() || entries.size() > max_archive_entries) {
            throw std::runtime_error("mkinitcpio inventory is empty or oversized");
        }
        std::map<std::string_view, const ArchiveEntry *> seen;
        std::uint64_t inspected = 0;
        for (const auto &entry : entries) {
            if (!seen.emplace(entry.path, &entry).second)
                throw std::runtime_error("duplicate mkinitcpio archive member");
            if (entry.bytes.size() > max_inspected_archive_bytes - inspected)
                throw std::runtime_error("mkinitcpio inventory byte limit exceeded");
            inspected += entry.bytes.size();
        }
        const auto require = [&seen](std::string_view path) -> const ArchiveEntry & {
            const auto found = seen.find(path);
            if (found == seen.end())
                throw std::runtime_error("missing mkinitcpio member: " + std::string(path));
            return *found->second;
        };
        const auto &sart = require("usr/bin/sart");
        if (sart.kind != ArchiveEntryKind::file || sart.mode != 0755 ||
            sart.bytes != std::vector<std::byte>(expected_sart.begin(), expected_sart.end())) {
            throw std::runtime_error("mkinitcpio contains the wrong Sart ELF");
        }
        for (const auto [path, id] :
             std::array{std::pair<std::string_view, embedded::TemplateId>{
                            "hooks/sart", embedded::TemplateId::mkinitcpio_runtime_hook},
                        std::pair<std::string_view, embedded::TemplateId>{
                            "usr/bin/plymouth", embedded::TemplateId::mkinitcpio_plymouth_bridge}}) {
            const auto &entry = require(path);
            const auto resource = embedded::template_resource(id);
            if (entry.kind != ArchiveEntryKind::file || entry.mode != resource.materialization.mode ||
                entry.bytes != bytes(resource.contents)) {
                throw std::runtime_error("mkinitcpio runtime resource changed: " + std::string(path));
            }
        }
        for (const auto path : {"init", "hooks/encrypt", "usr/bin/cryptsetup"}) {
            const auto &entry = require(path);
            if (entry.kind != ArchiveEntryKind::file || (entry.mode & 0111) == 0 || entry.bytes.empty()) {
                throw std::runtime_error("unsafe mkinitcpio executable: " + std::string(path));
            }
        }
        for (const auto &[path, entry] : seen) {
            static_cast<void>(entry);
            const auto slash = path.rfind('/');
            const auto name = slash == std::string_view::npos ? path : path.substr(slash + 1);
            if (name.starts_with("sart") && path != "usr/bin/sart" && path != "hooks/sart") {
                throw std::runtime_error("unexpected Sart mkinitcpio member: " + std::string(path));
            }
        }
        return {sha256(expected_sart), entries.size(), inspected};
    }

    DracutSystemdImageRecord verified_mkinitcpio_systemd_image_record(const MkinitcpioSystemdContract &contract,
                                                                      std::span<const std::byte> candidate,
                                                                      const ArchiveInspection &inspection,
                                                                      std::span<const std::byte> expected_sart) {
        validate_mkinitcpio_systemd_contract(contract);
        if (candidate.empty() || candidate.size() > max_candidate_bytes ||
            inspection.sart_digest != sha256(expected_sart)) {
            throw std::runtime_error("mkinitcpio candidate is empty, oversized, or unbound");
        }
        const auto digest = sha256(candidate);
        DracutSystemdImageRecord record{contract.package_base,
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
