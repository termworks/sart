#include "sart/install/backends.hpp"

#include "sart/core/sha256.hpp"
#include "sart/integration/patch.hpp"
#include "sart/integration/resources.hpp"

#include <algorithm>
#include <charconv>
#include <map>
#include <set>
#include <sstream>
#include <stdexcept>

namespace sart::install {
    namespace {

        constexpr std::string_view mkinitfs = "/sbin/mkinitfs";
        constexpr std::string_view update_extlinux = "/sbin/update-extlinux";
        constexpr std::string_view initramfs_init_path = "/usr/share/mkinitfs/initramfs-init";
        constexpr std::string_view config_path = "/etc/mkinitfs/mkinitfs.conf";
        constexpr std::string_view update_extlinux_config = "/etc/update-extlinux.conf";
        constexpr std::string_view extlinux_config = "/boot/extlinux.conf";
        constexpr std::string_view extlinux_fragment = "/etc/update-extlinux.d/50-sart-known-good";

        bool ascii_alnum(unsigned char byte) {
            return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z');
        }

        bool safe_token(std::string_view value, std::size_t maximum, bool dots) {
            return !value.empty() && value.size() <= maximum && std::ranges::all_of(value, [dots](unsigned char byte) {
                return ascii_alnum(byte) || byte == '_' || byte == '+' || byte == '-' || (dots && byte == '.');
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

        bool safe_command_line(std::string_view value) {
            return !value.empty() && value.size() <= 4096 && !value.contains('\n') && !value.contains('\r') &&
                   std::ranges::all_of(value, [](unsigned char byte) {
                       return byte == '\t' || byte == ' ' || (byte >= '!' && byte <= '~');
                   });
        }

        std::optional<std::string_view> kernel_flavor(std::string_view kernel) {
            const auto dash = kernel.rfind('-');
            if (dash == std::string_view::npos || dash + 1 == kernel.size())
                return std::nullopt;
            const auto flavor = kernel.substr(dash + 1);
            return safe_token(flavor, 64, false) ? std::optional(flavor) : std::nullopt;
        }

        std::string kernel_image(std::string_view flavor) { return "/boot/vmlinuz-" + std::string(flavor); }
        std::string active_image(std::string_view flavor) { return "/boot/initramfs-" + std::string(flavor); }
        std::string candidate_image(std::string_view flavor) {
            return "/boot/.sart-candidate-initramfs-" + std::string(flavor);
        }

        std::vector<std::byte> bytes(std::string_view text) {
            return {reinterpret_cast<const std::byte *>(text.data()),
                    reinterpret_cast<const std::byte *>(text.data() + text.size())};
        }

        std::string text(std::span<const std::byte> value) {
            return {reinterpret_cast<const char *>(value.data()), value.size()};
        }

        std::vector<std::byte> render_features(std::string_view source, const std::vector<std::string> &features) {
            std::string replacement = "features=\"";
            for (std::size_t index = 0; index < features.size(); ++index) {
                if (index != 0)
                    replacement += ' ';
                replacement += features[index];
            }
            replacement += '"';
            std::string output;
            bool replaced = false;
            std::size_t offset = 0;
            while (offset < source.size()) {
                const auto newline_at = source.find('\n', offset);
                const bool newline = newline_at != std::string_view::npos;
                const auto body = source.substr(offset, newline ? newline_at - offset : source.size() - offset);
                const auto first = body.find_first_not_of(" \t\r");
                const auto last = body.find_last_not_of(" \t\r");
                const auto trimmed =
                    first == std::string_view::npos ? std::string_view{} : body.substr(first, last - first + 1);
                if (!trimmed.empty() && !trimmed.starts_with('#')) {
                    if (replaced)
                        throw std::runtime_error("mkinitfs configuration has multiple assignments");
                    output.append(body.substr(0, first));
                    output += replacement;
                    if (last + 1 < body.size())
                        output.append(body.substr(last + 1));
                    replaced = true;
                } else {
                    output.append(body);
                }
                if (newline)
                    output += '\n';
                if (!newline)
                    break;
                offset = newline_at + 1;
            }
            if (!replaced)
                throw std::runtime_error("mkinitfs configuration omits its features assignment");
            return bytes(output);
        }

        std::vector<std::byte> deactivate_feature(std::string_view source) {
            auto features = parse_mkinitfs_features(source);
            const auto found = std::ranges::find(features, "sart");
            if (found == features.end())
                throw std::runtime_error("mkinitfs configuration omits sart");
            features.erase(found);
            if (features.empty())
                throw std::runtime_error("mkinitfs configuration contains only sart");
            return render_features(source, features);
        }

        std::optional<std::string_view> assignment_value(std::string_view line, std::string_view key) {
            if (!line.starts_with(key) || line.size() <= key.size() || line[key.size()] != '=')
                return std::nullopt;
            auto value = line.substr(key.size() + 1);
            if (value.size() >= 2 &&
                ((value.front() == '"' && value.back() == '"') || (value.front() == '\'' && value.back() == '\''))) {
                value.remove_prefix(1);
                value.remove_suffix(1);
            }
            return value;
        }

        std::vector<std::byte> render_extlinux(std::string_view kernel_path, std::string_view known_good_path,
                                               std::string_view command_line) {
            if (!kernel_path.starts_with("/boot/") || !known_good_path.starts_with("/boot/") ||
                !safe_token(kernel_path.substr(6), 64, false) || !safe_token(known_good_path.substr(6), 192, true) ||
                !safe_command_line(command_line)) {
                throw std::runtime_error("unsafe mkinitfs extlinux input");
            }
            const std::string output = "LABEL sart-known-good\n"
                                       "  MENU LABEL Sart known-good\n"
                                       "  LINUX " +
                                       std::string(kernel_path.substr(6)) +
                                       "\n"
                                       "  INITRD " +
                                       std::string(known_good_path.substr(6)) +
                                       "\n"
                                       "  APPEND " +
                                       std::string(command_line) + " sart=0 rd.sart=0\n";
            return bytes(output);
        }

        std::uint64_t hex_field(std::span<const std::byte> input) {
            const std::string_view field(reinterpret_cast<const char *>(input.data()), input.size());
            std::uint64_t value{};
            const auto [end, error] = std::from_chars(field.data(), field.data() + field.size(), value, 16);
            if (error != std::errc{} || end != field.data() + field.size())
                throw std::runtime_error("invalid newc field");
            return value;
        }

        std::size_t align4(std::size_t value) {
            if (value > std::numeric_limits<std::size_t>::max() - 3)
                throw std::runtime_error("newc offset overflow");
            return (value + 3) & ~std::size_t(3);
        }

        std::optional<std::string> normalized_path(std::string_view name) {
            if (name.starts_with("./"))
                name.remove_prefix(2);
            if (name == ".")
                return std::string{};
            if (name.empty() || name.size() > 4096 || name.front() == '/' || name.contains('\0'))
                return std::nullopt;
            std::string output;
            std::size_t offset = 0;
            while (offset < name.size()) {
                const auto end = name.find('/', offset);
                const auto part =
                    name.substr(offset, end == std::string_view::npos ? name.size() - offset : end - offset);
                if (part.empty() || part == "." || part == "..")
                    return std::nullopt;
                if (!output.empty())
                    output += '/';
                output.append(part);
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return output;
        }

    } // namespace

    std::vector<std::string> parse_mkinitfs_features(std::string_view source) {
        if (source.size() > 65536 || source.contains('\0'))
            throw std::runtime_error("mkinitfs configuration is oversized");
        std::vector<std::string_view> records;
        std::size_t offset = 0;
        while (offset <= source.size()) {
            const auto end = source.find('\n', offset);
            auto line = source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset);
            const auto first = line.find_first_not_of(" \t\r");
            const auto last = line.find_last_not_of(" \t\r");
            if (first != std::string_view::npos) {
                line = line.substr(first, last - first + 1);
                if (!line.starts_with('#'))
                    records.push_back(line);
            }
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        if (records.size() != 1 || !records.front().starts_with("features=\"") || !records.front().ends_with('"')) {
            throw std::runtime_error("mkinitfs features assignment is not canonical");
        }
        auto value = records.front().substr(10, records.front().size() - 11);
        std::istringstream input{std::string(value)};
        std::vector<std::string> features;
        std::string feature;
        while (input >> feature)
            features.push_back(feature);
        const std::set<std::string> unique(features.begin(), features.end());
        if (features.empty() || features.size() > 64 || unique.size() != features.size() ||
            std::ranges::any_of(features, [](const auto &item) { return !safe_token(item, 64, false); })) {
            throw std::runtime_error("mkinitfs feature set is unsafe or ambiguous");
        }
        return features;
    }

    std::vector<std::byte> activate_mkinitfs_sart_feature(std::string_view source) {
        auto features = parse_mkinitfs_features(source);
        if (std::ranges::find(features, "sart") != features.end()) {
            throw std::runtime_error("mkinitfs configuration contains unmanaged sart");
        }
        features.emplace_back("sart");
        return render_features(source, features);
    }

    ExtlinuxSettings parse_update_extlinux_settings(std::string_view source) {
        if (source.size() > 65536 || source.contains('\0'))
            throw std::runtime_error("update-extlinux configuration is oversized");
        std::map<std::string, std::string> values;
        std::size_t offset = 0;
        while (offset <= source.size()) {
            const auto end = source.find('\n', offset);
            auto line = source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset);
            const auto first = line.find_first_not_of(" \t\r");
            const auto last = line.find_last_not_of(" \t\r");
            if (first != std::string_view::npos) {
                line = line.substr(first, last - first + 1);
                if (!line.starts_with('#')) {
                    const auto equal = line.find('=');
                    const auto key = equal == std::string_view::npos ? std::string_view{} : line.substr(0, equal);
                    if (key.empty() ||
                        !std::ranges::all_of(key,
                                             [](unsigned char byte) { return ascii_alnum(byte) || byte == '_'; }) ||
                        !values.emplace(key, line).second) {
                        throw std::runtime_error("unsafe update-extlinux assignment");
                    }
                }
            }
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        const auto required = [&values](std::string_view key) -> std::string_view {
            const auto found = values.find(std::string(key));
            if (found == values.end())
                throw std::runtime_error("update-extlinux setting is missing");
            const auto parsed = assignment_value(found->second, key);
            if (!parsed)
                throw std::runtime_error("update-extlinux setting is malformed");
            return *parsed;
        };
        const auto overwrite = required("overwrite") == "1";
        const auto label = required("default");
        const auto root = required("root");
        const auto modules = required("modules");
        const auto options = required("default_kernel_opts");
        if (!safe_token(label, 64, false) || root.empty() || modules.empty() || !safe_command_line(root) ||
            !safe_command_line(modules) || !safe_command_line(options)) {
            throw std::runtime_error("unsafe update-extlinux boot settings");
        }
        std::string command =
            "root=" + std::string(root) + " modules=" + std::string(modules) + " " + std::string(options);
        if (!safe_command_line(command))
            throw std::runtime_error("unsafe update-extlinux command line");
        return {overwrite, std::string(label), std::move(command)};
    }

    std::string parse_extlinux_entry_command_line(std::string_view source, std::string_view label) {
        if (source.size() > 1024 * 1024 || source.contains('\0') || !safe_token(label, 64, false)) {
            throw std::runtime_error("extlinux configuration is unsafe");
        }
        bool in_entry = false;
        std::optional<std::string> command;
        std::size_t offset = 0;
        while (offset <= source.size()) {
            const auto end = source.find('\n', offset);
            auto line = source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset);
            const auto first = line.find_first_not_of(" \t\r");
            const auto last = line.find_last_not_of(" \t\r");
            line = first == std::string_view::npos ? std::string_view{} : line.substr(first, last - first + 1);
            if (line.starts_with("LABEL "))
                in_entry = line.substr(6) == label;
            else if (in_entry && line.starts_with("APPEND ")) {
                if (command)
                    throw std::runtime_error("extlinux entry has duplicate APPEND records");
                command = line.substr(7);
            }
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        if (!command || !safe_command_line(*command))
            throw std::runtime_error("extlinux entry has no safe command line");
        return *command;
    }

    MkinitfsOpenRcContract plan_mkinitfs_openrc(const MkinitfsOpenRcFacts &facts, std::string alternate_root) {
        if (!safe_root(alternate_root) || facts.architecture != product_architecture || facts.pid1_comm != "init" ||
            facts.kernel_versions.size() != 1 || !safe_token(facts.kernel_versions.front(), 128, true)) {
            throw std::runtime_error("mkinitfs-openrc root, architecture, PID 1, or kernel is unsupported");
        }
        const auto flavor = kernel_flavor(facts.kernel_versions.front());
        if (!flavor || !facts.boot_writable || facts.boot_free_bytes < min_boot_free_bytes ||
            facts.boot_free_inodes < min_boot_free_inodes) {
            throw std::runtime_error("mkinitfs-openrc flavor or /boot contract is unsupported");
        }
        const auto active = active_image(*flavor);
        if (facts.known_good_path != active || facts.known_good_bytes == 0 ||
            facts.known_good_bytes > max_candidate_bytes) {
            throw std::runtime_error("mkinitfs-openrc active image differs from the contract");
        }
        const std::set<std::string_view> required_tools{mkinitfs, update_extlinux, "/sbin/extlinux", "/sbin/openrc"};
        std::map<std::string_view, const ToolFact *> tools;
        for (const auto &tool : facts.tools) {
            if (!tools.emplace(tool.path, &tool).second)
                throw std::runtime_error("duplicate mkinitfs tool fact");
        }
        if (tools.size() != required_tools.size())
            throw std::runtime_error("mkinitfs tool set differs");
        for (const auto path : required_tools) {
            const auto found = tools.find(path);
            if (found == tools.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || !found->second->executable) {
                throw std::runtime_error("unsafe mkinitfs prerequisite: " + std::string(path));
            }
        }
        const auto kernel_path = kernel_image(*flavor);
        const std::set<std::string> required_files{std::string(initramfs_init_path), std::string(config_path),
                                                   std::string(update_extlinux_config), std::string(extlinux_config),
                                                   kernel_path};
        std::map<std::string_view, const MkinitfsOpenRcPathFact *> files;
        for (const auto &file : facts.contract_files) {
            if (!files.emplace(file.path, &file).second)
                throw std::runtime_error("duplicate mkinitfs file fact");
        }
        if (files.size() != required_files.size())
            throw std::runtime_error("mkinitfs prerequisite set differs");
        for (const auto &path : required_files) {
            const auto found = files.find(path);
            if (found == files.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || found->second->executable || (found->second->mode & 0022) != 0 ||
                (found->second->mode & 0400) == 0) {
                throw std::runtime_error("unsafe mkinitfs prerequisite: " + path);
            }
        }
        if (files.at(initramfs_init_path)->digest != sha256(facts.initramfs_init_source) ||
            !integration::patch_mkinitfs_init(facts.initramfs_init_source)) {
            throw std::runtime_error("mkinitfs initramfs-init differs from the patch contract");
        }
        if (files.at(config_path)->digest != sha256(facts.mkinitfs_config_source) ||
            parse_mkinitfs_features(facts.mkinitfs_config_source) != facts.mkinitfs_features) {
            throw std::runtime_error("mkinitfs configuration differs from its descriptor fact");
        }
        const bool already = std::ranges::find(facts.mkinitfs_features, "sart") != facts.mkinitfs_features.end();
        const auto original =
            already ? deactivate_feature(facts.mkinitfs_config_source) : bytes(facts.mkinitfs_config_source);
        const auto activated = already ? bytes(facts.mkinitfs_config_source)
                                       : activate_mkinitfs_sart_feature(facts.mkinitfs_config_source);
        if (!facts.extlinux_overwrite || facts.extlinux_default_label != *flavor ||
            !safe_command_line(facts.kernel_command_line)) {
            throw std::runtime_error("mkinitfs extlinux settings differ from the active kernel contract");
        }
        const auto candidate = candidate_image(*flavor);
        const auto known_good = active + ".sart-known-good";
        MkinitfsOpenRcContract contract{
            facts.kernel_versions.front(),
            std::string(*flavor),
            kernel_path,
            active,
            candidate,
            known_good,
            facts.known_good_digest,
            std::string(extlinux_fragment),
            std::string(extlinux_config),
            render_extlinux(kernel_path, known_good, facts.kernel_command_line),
            std::string(config_path),
            files.at(config_path)->mode,
            original,
            activated,
            already,
            facts.contract_files,
            {GeneratorKind::mkinitfs,
             std::string(mkinitfs),
             alternate_root,
             std::nullopt,
             {"-C", "none", "-o", candidate, facts.kernel_versions.front()},
             true},
            {GeneratorKind::extlinux_update, std::string(update_extlinux), alternate_root, std::nullopt, {}, true}};
        validate_mkinitfs_openrc_contract(contract);
        return contract;
    }

    void validate_mkinitfs_openrc_generator_request(const GeneratorRequest &value) {
        if (!safe_root(value.alternate_root) || !value.clear_environment || value.working_directory) {
            throw std::runtime_error("unsafe mkinitfs generator context");
        }
        if (value.generator == GeneratorKind::mkinitfs && value.executable == mkinitfs) {
            if (value.arguments.size() != 5 || value.arguments[0] != "-C" || value.arguments[1] != "none" ||
                value.arguments[2] != "-o")
                throw std::runtime_error("mkinitfs argv differs");
            const auto flavor = kernel_flavor(value.arguments[4]);
            if (!flavor || value.arguments[3] != candidate_image(*flavor))
                throw std::runtime_error("mkinitfs candidate differs");
            return;
        }
        if (value.generator == GeneratorKind::extlinux_update && value.executable == update_extlinux &&
            value.arguments.empty())
            return;
        throw std::runtime_error("unreviewed mkinitfs generator request");
    }

    void validate_mkinitfs_openrc_contract(const MkinitfsOpenRcContract &contract) {
        const auto flavor = kernel_flavor(contract.kernel_version);
        if (!flavor || *flavor != contract.kernel_flavor ||
            contract.kernel_image != kernel_image(contract.kernel_flavor) ||
            contract.active_image != active_image(contract.kernel_flavor) ||
            contract.candidate_image != candidate_image(contract.kernel_flavor) ||
            contract.known_good_image != contract.active_image + ".sart-known-good" ||
            contract.extlinux_fragment_path != extlinux_fragment || contract.extlinux_config_path != extlinux_config ||
            contract.mkinitfs_config_path != config_path ||
            contract.generate.alternate_root != contract.update_extlinux.alternate_root) {
            throw std::runtime_error("mkinitfs contract mixes incompatible capabilities");
        }
        validate_mkinitfs_openrc_generator_request(contract.generate);
        validate_mkinitfs_openrc_generator_request(contract.update_extlinux);
        if (contract.generate.arguments[3] != contract.candidate_image ||
            contract.generate.arguments[4] != contract.kernel_version) {
            throw std::runtime_error("mkinitfs generation is not bound to the contract");
        }
        const auto config =
            std::ranges::find(contract.prerequisites, std::string(config_path), &MkinitfsOpenRcPathFact::path);
        if (config == contract.prerequisites.end() || contract.mkinitfs_config_original.empty() ||
            contract.mkinitfs_config_original.size() > 65536 || contract.mkinitfs_config_mode != config->mode ||
            activate_mkinitfs_sart_feature(text(contract.mkinitfs_config_original)) !=
                contract.mkinitfs_config_activated) {
            throw std::runtime_error("mkinitfs configuration preimage differs from the contract");
        }
        if ((contract.mkinitfs_config_already_active &&
             (sha256(contract.mkinitfs_config_activated) != config->digest ||
              deactivate_feature(text(contract.mkinitfs_config_activated)) != contract.mkinitfs_config_original)) ||
            (!contract.mkinitfs_config_already_active && sha256(contract.mkinitfs_config_original) != config->digest)) {
            throw std::runtime_error("mkinitfs configuration digest differs from the contract");
        }
        const auto fragment = text(contract.extlinux_fragment);
        if (!fragment.starts_with("LABEL sart-known-good\n") ||
            !fragment.contains("  LINUX " + contract.kernel_image.substr(6) + "\n") ||
            !fragment.contains("  INITRD " + contract.known_good_image.substr(6) + "\n") ||
            !fragment.contains(" sart=0 rd.sart=0\n") || fragment.contains('@')) {
            throw std::runtime_error("mkinitfs extlinux fragment differs from the contract");
        }
        const std::set<std::string> expected{std::string(initramfs_init_path), std::string(config_path),
                                             std::string(update_extlinux_config), std::string(extlinux_config),
                                             contract.kernel_image};
        std::set<std::string> actual;
        for (const auto &prerequisite : contract.prerequisites)
            actual.insert(prerequisite.path);
        if (actual != expected || actual.size() != contract.prerequisites.size()) {
            throw std::runtime_error("mkinitfs prerequisite set differs from the contract");
        }
    }

    ArchiveInspection inspect_mkinitfs_openrc_archive(std::span<const std::byte> candidate,
                                                      std::span<const std::byte> expected_sart) {
        if (candidate.empty() || candidate.size() > max_candidate_bytes)
            throw std::runtime_error("mkinitfs archive size is unsupported");
        std::size_t offset = 0;
        std::size_t entries = 0;
        std::uint64_t inspected = 0;
        std::set<std::string> seen;
        std::optional<std::vector<std::byte>> sart, runtime, findfs, init;
        bool trailer = false;
        while (offset < candidate.size()) {
            if (std::ranges::all_of(candidate.subspan(offset), [](std::byte byte) { return byte == std::byte{}; }))
                break;
            if (candidate.size() - offset < 110)
                throw std::runtime_error("truncated mkinitfs newc header");
            const auto header = candidate.subspan(offset, 110);
            const std::string_view magic(reinterpret_cast<const char *>(header.data()), 6);
            if (magic != "070701" && magic != "070702")
                throw std::runtime_error("mkinitfs archive is not newc");
            const auto mode = static_cast<std::uint32_t>(hex_field(header.subspan(14, 8)));
            const auto uid = hex_field(header.subspan(22, 8));
            const auto size = hex_field(header.subspan(54, 8));
            const auto name_size = hex_field(header.subspan(94, 8));
            if (name_size == 0 || name_size > 4096 || size > max_inspected_archive_bytes ||
                name_size > candidate.size() - (offset + 110))
                throw std::runtime_error("mkinitfs member exceeds a bound");
            const auto name_start = offset + 110;
            const auto name_bytes = candidate.subspan(name_start, static_cast<std::size_t>(name_size));
            if (name_bytes.back() != std::byte{} ||
                std::ranges::find(name_bytes.first(name_bytes.size() - 1), std::byte{}) !=
                    name_bytes.first(name_bytes.size() - 1).end()) {
                throw std::runtime_error("mkinitfs member name is not canonical");
            }
            const std::string_view name(reinterpret_cast<const char *>(name_bytes.data()), name_bytes.size() - 1);
            if (name == "TRAILER!!!") {
                if (size != 0 || trailer)
                    throw std::runtime_error("malformed mkinitfs trailer");
                trailer = true;
                offset = align4(name_start + name_bytes.size());
                continue;
            }
            if (trailer)
                throw std::runtime_error("mkinitfs archive has members after trailer");
            const auto path = normalized_path(name);
            if (!path || !seen.insert(*path).second)
                throw std::runtime_error("unsafe or duplicate mkinitfs path");
            if (++entries > max_archive_entries || size > max_inspected_archive_bytes - inspected) {
                throw std::runtime_error("mkinitfs archive inspection bound exceeded");
            }
            inspected += size;
            const auto data_start = align4(name_start + name_bytes.size());
            if (data_start > candidate.size() || size > candidate.size() - data_start) {
                throw std::runtime_error("truncated mkinitfs member data");
            }
            const auto data = candidate.subspan(data_start, static_cast<std::size_t>(size));
            const auto file_type = mode & 0170000;
            if (path->contains("sart") && *path != "usr/bin/sart" && *path != "usr/libexec/sart" &&
                *path != "usr/libexec/sart/mkinitfs-runtime" && *path != "usr/libexec/sart/mkinitfs-findfs") {
                throw std::runtime_error("foreign Sart mkinitfs member");
            }
            const auto capture = [&](std::optional<std::vector<std::byte>> &output, bool exact_exec) {
                if (file_type != 0100000 || uid != 0 || (exact_exec ? (mode & 07777) != 0755 : (mode & 0111) == 0)) {
                    throw std::runtime_error("unsafe mkinitfs member metadata");
                }
                output = std::vector<std::byte>(data.begin(), data.end());
            };
            if (*path == "usr/bin/sart")
                capture(sart, true);
            else if (*path == "usr/libexec/sart/mkinitfs-runtime")
                capture(runtime, true);
            else if (*path == "usr/libexec/sart/mkinitfs-findfs")
                capture(findfs, true);
            else if (*path == "init")
                capture(init, false);
            offset = align4(data_start + data.size());
        }
        if (!trailer || !sart || !runtime || !findfs || !init ||
            *sart != std::vector<std::byte>(expected_sart.begin(), expected_sart.end()) ||
            *runtime != bytes(integration::mkinitfs::runtime_hook) ||
            *findfs != bytes(integration::mkinitfs::findfs_wrapper)) {
            throw std::runtime_error("mkinitfs archive omits or changes a required resource");
        }
        const auto init_text = text(*init);
        const auto count = [&init_text](std::string_view needle) {
            std::size_t result = 0, at = 0;
            while ((at = init_text.find(needle, at)) != std::string::npos) {
                ++result;
                at += needle.size();
            }
            return result;
        };
        if (count(integration::mkinitfs::early_call_snippet) != 1 ||
            count(integration::mkinitfs::handoff_call_snippet) != 1) {
            throw std::runtime_error("mkinitfs init omits managed lifecycle calls");
        }
        return {sha256(expected_sart), entries, inspected};
    }

    DracutSystemdImageRecord verified_mkinitfs_openrc_image_record(const MkinitfsOpenRcContract &contract,
                                                                   std::span<const std::byte> candidate,
                                                                   const ArchiveInspection &inspection,
                                                                   std::span<const std::byte> expected_sart) {
        validate_mkinitfs_openrc_contract(contract);
        if (candidate.empty() || candidate.size() > max_candidate_bytes ||
            inspection.sart_digest != sha256(expected_sart) || inspection.inspected_entries == 0 ||
            inspection.inspected_bytes == 0) {
            throw std::runtime_error("mkinitfs candidate inspection is incomplete");
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
                                        contract.extlinux_fragment_path,
                                        sha256(contract.extlinux_fragment),
                                        contract.extlinux_config_path,
                                        sha256(expected_sart)};
        validate_mkinitfs_openrc_image_record(record);
        return record;
    }

    void validate_mkinitfs_openrc_image_record(const DracutSystemdImageRecord &record) {
        const auto flavor = kernel_flavor(record.kernel_version);
        if (!flavor || record.active_image != active_image(*flavor) ||
            record.candidate_image != candidate_image(*flavor) ||
            record.known_good_image != record.active_image + ".sart-known-good" ||
            record.grub_script_path != extlinux_fragment || record.grub_config_path != extlinux_config ||
            record.candidate_bytes == 0 || record.candidate_bytes > max_candidate_bytes ||
            record.active_digest != record.candidate_digest) {
            throw std::runtime_error("mkinitfs image record violates the fixed contract");
        }
    }

    bool mkinitfs_openrc_managed_image_path(std::string_view path) {
        if (path == config_path || path == extlinux_fragment || path == extlinux_config)
            return true;
        constexpr std::string_view active = "/boot/initramfs-";
        constexpr std::string_view candidate = "/boot/.sart-candidate-initramfs-";
        if (path.starts_with(candidate))
            return safe_token(path.substr(candidate.size()), 64, false);
        if (!path.starts_with(active))
            return false;
        auto flavor = path.substr(active.size());
        if (flavor.ends_with(".sart-known-good"))
            flavor.remove_suffix(std::string_view(".sart-known-good").size());
        return safe_token(flavor, 64, false);
    }

} // namespace sart::install
