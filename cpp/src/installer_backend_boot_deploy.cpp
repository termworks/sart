#include "bootart/installer_backends.hpp"

#include "bootart/integration_patch.hpp"
#include "bootart/sha256.hpp"

#include <algorithm>
#include <limits>
#include <map>
#include <set>
#include <sstream>
#include <stdexcept>

namespace bootart::install {
    namespace {

        constexpr std::string_view mkinitfs = "/usr/sbin/mkinitfs";
        constexpr std::string_view boot_deploy = "/usr/bin/boot-deploy";
        constexpr std::string_view openrc = "/usr/sbin/openrc";
        constexpr std::string_view systemd = "/usr/lib/systemd/systemd";
        constexpr std::string_view active_image = "/boot/initramfs";
        constexpr std::string_view candidate_directory = "/boot/.bootart-candidate";
        constexpr std::string_view candidate_image = "/boot/.bootart-candidate/initramfs";
        constexpr std::string_view candidate_boot_image = "/boot/.bootart-candidate/boot.img";
        constexpr std::string_view known_good_image = "/boot/initramfs.bootart-known-good";
        constexpr std::string_view known_good_entry = "/boot/loader/entries/bootart-known-good.conf";
        constexpr std::string_view deviceinfo_path = "/etc/deviceinfo";

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

        bool safe_kernel_filename(std::string_view value) {
            if (value == "linux.efi" || value == "vmlinuz")
                return true;
            if (!value.starts_with("vmlinuz-"))
                return false;
            return safe_token(value.substr(8), 192, true);
        }

        bool safe_kernel_image(std::string_view value) {
            return value.starts_with("/boot/") && safe_kernel_filename(value.substr(6));
        }

        bool safe_loader_entry(std::string_view value) {
            constexpr std::string_view prefix = "/boot/loader/entries/";
            if (!value.starts_with(prefix) || !value.ends_with(".conf"))
                return false;
            const auto name = value.substr(prefix.size(), value.size() - prefix.size() - 5);
            return safe_token(name, 128, false);
        }

        bool safe_command_line(std::string_view value) {
            return !value.empty() && value.size() <= 8192 && !value.contains('\n') && !value.contains('\r') &&
                   std::ranges::all_of(value, [](unsigned char byte) { return byte >= 0x20 && byte != 0x7f; });
        }

        bool safe_mode(std::uint16_t mode) { return mode == 0600 || mode == 0644 || mode == 0700 || mode == 0755; }

        std::string text(std::span<const std::byte> value) {
            return {reinterpret_cast<const char *>(value.data()), value.size()};
        }

        std::vector<std::byte> bytes(std::string_view value) {
            return {reinterpret_cast<const std::byte *>(value.data()),
                    reinterpret_cast<const std::byte *>(value.data() + value.size())};
        }

        bool safe_partition(const AndroidBootPartitionFact &partition) {
            if (partition.device_number == 0 || partition.bytes == 0 || partition.bytes > max_transaction_bytes ||
                !safe_token(partition.label, 64, true) || !partition.canonical_path.starts_with("/dev/") ||
                partition.canonical_path.contains("//") || partition.canonical_path.contains("/../") ||
                partition.canonical_path.ends_with("/..") || partition.canonical_path.contains("/./"))
                return false;
            return true;
        }

        std::vector<std::byte> render_known_good(std::string_view kernel_image, std::string_view command_line) {
            if (!safe_kernel_image(kernel_image) || !safe_command_line(command_line)) {
                throw std::runtime_error("unsafe boot-deploy known-good entry input");
            }
            const std::string output = "title Bootart known-good\nlinux " + std::string(kernel_image.substr(6)) +
                                       "\ninitrd initramfs.bootart-known-good\noptions " + std::string(command_line) +
                                       " bootart=0 rd.bootart=0\n";
            return bytes(output);
        }

    } // namespace

    std::pair<std::string, std::string> parse_mkinitfs_boot_deploy_loader_entry(std::string_view source) {
        if (source.empty() || source.size() > 16384 || source.contains('\r')) {
            throw std::runtime_error("boot-deploy loader entry is oversized");
        }
        std::optional<std::string> kernel, options;
        std::size_t initramfs_count = 0;
        std::size_t offset = 0;
        while (offset <= source.size()) {
            const auto end = source.find('\n', offset);
            auto line = source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset);
            const auto first = line.find_first_not_of(" \t");
            const auto last = line.find_last_not_of(" \t");
            line = first == std::string_view::npos ? std::string_view{} : line.substr(first, last - first + 1);
            if (!line.empty() && !line.starts_with('#')) {
                if (line.starts_with("linux ")) {
                    if (kernel)
                        throw std::runtime_error("duplicate boot-deploy linux record");
                    kernel = std::string(line.substr(6));
                } else if (line.starts_with("initrd ") && line.substr(7) == "initramfs") {
                    ++initramfs_count;
                } else if (line.starts_with("options ")) {
                    if (options)
                        throw std::runtime_error("duplicate boot-deploy options record");
                    options = std::string(line.substr(8));
                }
            }
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        if (!kernel || !safe_kernel_filename(*kernel) || !options || !safe_command_line(*options) ||
            initramfs_count != 1) {
            throw std::runtime_error("boot-deploy loader entry differs from the contract");
        }
        return {"/boot/" + *kernel, *options};
    }

    std::vector<std::byte> activate_mkinitfs_boot_deploy_loader_entry(std::string_view source) {
        static_cast<void>(parse_mkinitfs_boot_deploy_loader_entry(source));
        std::string output;
        std::size_t records = 0, splash = 0, offset = 0;
        while (offset < source.size()) {
            const auto end = source.find('\n', offset);
            const bool newline = end != std::string_view::npos;
            const auto body = source.substr(offset, newline ? end - offset : source.size() - offset);
            const auto first = body.find_first_not_of(" \t");
            const auto last = body.find_last_not_of(" \t");
            const auto trimmed =
                first == std::string_view::npos ? std::string_view{} : body.substr(first, last - first + 1);
            if (!trimmed.starts_with("options ")) {
                output.append(body);
            } else {
                ++records;
                std::vector<std::string_view> kept;
                auto options = trimmed.substr(8);
                std::size_t token_offset = 0;
                while (token_offset < options.size()) {
                    while (token_offset < options.size() &&
                           (options[token_offset] == ' ' || options[token_offset] == '\t'))
                        ++token_offset;
                    const auto token_end = options.find_first_of(" \t", token_offset);
                    const auto token =
                        options.substr(token_offset, token_end == std::string_view::npos ? options.size() - token_offset
                                                                                         : token_end - token_offset);
                    if (token == "splash")
                        ++splash;
                    else if (!token.empty())
                        kept.push_back(token);
                    if (token_end == std::string_view::npos)
                        break;
                    token_offset = token_end + 1;
                }
                if (kept.empty())
                    throw std::runtime_error("boot-deploy loader options become empty");
                output.append(body.substr(0, first));
                output += "options ";
                for (std::size_t index = 0; index < kept.size(); ++index) {
                    if (index != 0)
                        output += ' ';
                    output.append(kept[index]);
                }
            }
            if (newline)
                output += '\n';
            if (!newline)
                break;
            offset = end + 1;
        }
        if (records != 1 || splash > 1 || output.empty() || output.size() > 16384) {
            throw std::runtime_error("ambiguous boot-deploy splash takeover");
        }
        return bytes(output);
    }

    std::string parse_mkinitfs_boot_deploy_version(std::string_view source) {
        const std::string marker =
            "INITRAMFS_PKG_VERSION=\"" + std::string(integration::reviewed_boot_deploy_initramfs_version) + "\"";
        std::size_t marker_count = 0, assignment_count = 0, offset = 0;
        while (offset <= source.size()) {
            const auto end = source.find('\n', offset);
            auto line = source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset);
            const auto first = line.find_first_not_of(" \t");
            if (first != std::string_view::npos)
                line.remove_prefix(first);
            if (line == marker)
                ++marker_count;
            if (line.starts_with("INITRAMFS_PKG_VERSION="))
                ++assignment_count;
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        if (marker_count != 1 || assignment_count != 1)
            throw std::runtime_error("boot-deploy version differs");
        return std::string(integration::reviewed_boot_deploy_initramfs_version);
    }

    MkinitfsBootDeployContract plan_mkinitfs_boot_deploy(const MkinitfsBootDeployFacts &facts, bool systemd_real_root,
                                                         std::string alternate_root) {
        const std::string_view expected_pid = systemd_real_root ? "systemd" : "init";
        if (!safe_root(alternate_root) || facts.architecture != product_architecture ||
            facts.pid1_comm != expected_pid || facts.root_filesystem_device == facts.boot_filesystem_device ||
            !facts.boot_writable) {
            throw std::runtime_error("boot-deploy root, architecture, supervisor, or filesystem is unsupported");
        }
        const auto required_bytes = mkinitfs_boot_deploy_initial_boot_bytes(facts.kernel_bytes, facts.known_good_bytes,
                                                                            facts.boot_allocation_unit);
        if (facts.boot_free_bytes < required_bytes || (facts.boot_total_inodes == 0 && facts.boot_free_inodes != 0) ||
            (facts.boot_total_inodes != 0 &&
             (facts.boot_free_inodes > facts.boot_total_inodes || facts.boot_free_inodes < min_boot_free_inodes))) {
            throw std::runtime_error("boot-deploy /boot capacity or inode accounting is unsupported");
        }
        if (facts.initramfs_version != integration::reviewed_boot_deploy_initramfs_version) {
            throw std::runtime_error("boot-deploy initramfs version differs");
        }
        const auto patched =
            integration::patch_boot_deploy_init_functions(facts.init_functions_2nd, facts.initramfs_version);
        if (!patched)
            throw std::runtime_error("boot-deploy init functions differ from the patch contract");
        const std::set<std::string_view> required_tools =
            systemd_real_root ? std::set<std::string_view>{mkinitfs, boot_deploy, systemd}
                              : std::set<std::string_view>{mkinitfs, boot_deploy, openrc};
        std::map<std::string_view, const ToolFact *> tools;
        for (const auto &tool : facts.tools) {
            if (!tools.emplace(tool.path, &tool).second)
                throw std::runtime_error("duplicate boot-deploy tool fact");
        }
        if (tools.size() != required_tools.size())
            throw std::runtime_error("boot-deploy tool set differs");
        for (const auto path : required_tools) {
            const auto found = tools.find(path);
            if (found == tools.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || !found->second->executable) {
                throw std::runtime_error("unsafe boot-deploy prerequisite: " + std::string(path));
            }
        }
        const std::map<std::string_view, bool> required_files{{"/usr/share/initramfs/init.sh", true},
                                                              {"/usr/share/initramfs/init_2nd.sh", true},
                                                              {"/usr/share/initramfs/init_functions_2nd.sh", false},
                                                              {"/usr/share/boot-deploy/boot-deploy-functions.sh", true},
                                                              {"/usr/share/boot-deploy/os-customization", false}};
        std::map<std::string_view, const MkinitfsBootDeployPathFact *> files;
        for (const auto &file : facts.contract_files) {
            if (!files.emplace(file.path, &file).second)
                throw std::runtime_error("duplicate boot-deploy file fact");
        }
        if (files.size() != required_files.size())
            throw std::runtime_error("boot-deploy file set differs");
        for (const auto &[path, executable] : required_files) {
            const auto found = files.find(path);
            if (found == files.end() || !found->second->root_owned || !found->second->regular ||
                found->second->symlink || found->second->executable != executable) {
                throw std::runtime_error("unsafe boot-deploy contract file: " + std::string(path));
            }
        }
        if (facts.active_image != active_image || facts.known_good_bytes == 0 ||
            facts.known_good_bytes > max_candidate_bytes || facts.kernel_bytes == 0 ||
            facts.kernel_bytes > max_candidate_bytes || !safe_kernel_image(facts.kernel_image) ||
            !safe_loader_entry(facts.active_loader_entry) || !safe_mode(facts.active_loader_entry_mode)) {
            throw std::runtime_error("boot-deploy active layout differs from the contract");
        }
        const auto source = text(facts.active_loader_entry_bytes);
        const auto [loader_kernel, loader_options] = parse_mkinitfs_boot_deploy_loader_entry(source);
        if (loader_kernel != facts.kernel_image || loader_options != facts.kernel_command_line) {
            throw std::runtime_error("boot-deploy loader entry changed after discovery");
        }
        std::optional<AndroidBootGenerationContract> android;
        if (facts.android_boot) {
            if (facts.android_boot->deviceinfo.architecture != facts.architecture ||
                !safe_partition(facts.android_boot->partition)) {
                throw std::runtime_error("Android boot facts differ from the running product");
            }
            android = AndroidBootGenerationContract{facts.android_boot->deviceinfo,   facts.android_boot->partition,
                                                    facts.android_boot->dtb_path,     facts.android_boot->dtb_digest,
                                                    facts.android_boot->dtb_bytes,    std::string(deviceinfo_path),
                                                    std::string(candidate_boot_image)};
        }
        const auto kernel_name = facts.kernel_image.substr(6);
        MkinitfsBootDeployContract contract{facts.kernel_image,
                                            facts.kernel_bytes,
                                            facts.active_image,
                                            facts.known_good_bytes,
                                            facts.active_image_compression,
                                            facts.boot_filesystem_device,
                                            facts.boot_allocation_unit,
                                            std::string(candidate_directory),
                                            std::string(candidate_image),
                                            std::string(candidate_directory) + "/" + kernel_name,
                                            std::string(known_good_image),
                                            facts.known_good_digest,
                                            std::string(known_good_entry),
                                            facts.active_loader_entry_mode,
                                            render_known_good(facts.kernel_image, facts.kernel_command_line),
                                            facts.active_loader_entry,
                                            facts.active_loader_entry_mode,
                                            facts.active_loader_entry_bytes,
                                            activate_mkinitfs_boot_deploy_loader_entry(source),
                                            bytes(*patched),
                                            {GeneratorKind::mkinitfs_boot_deploy,
                                             std::string(mkinitfs),
                                             alternate_root,
                                             std::nullopt,
                                             {"-d", std::string(candidate_directory)},
                                             true},
                                            android};
        validate_mkinitfs_boot_deploy_contract(contract);
        return contract;
    }

    void validate_mkinitfs_boot_deploy_generator_request(const GeneratorRequest &value) {
        if (value.generator != GeneratorKind::mkinitfs_boot_deploy || value.executable != mkinitfs ||
            !value.clear_environment || !safe_root(value.alternate_root) || value.working_directory ||
            value.arguments != std::vector<std::string>{"-d", std::string(candidate_directory)}) {
            throw std::runtime_error("boot-deploy generator request differs from the fixed contract");
        }
    }

    void validate_mkinitfs_boot_deploy_contract(const MkinitfsBootDeployContract &contract) {
        validate_mkinitfs_boot_deploy_generator_request(contract.generate);
        if (!safe_kernel_image(contract.kernel_image))
            throw std::runtime_error("unsafe boot-deploy kernel path");
        const auto kernel_name = contract.kernel_image.substr(6);
        if (contract.active_image != active_image || contract.kernel_bytes == 0 ||
            contract.kernel_bytes > max_candidate_bytes || contract.active_image_bytes == 0 ||
            contract.active_image_bytes > max_candidate_bytes || contract.boot_filesystem_device == 0 ||
            contract.boot_allocation_unit == 0 || contract.candidate_directory != candidate_directory ||
            contract.candidate_image != candidate_image ||
            contract.candidate_kernel != std::string(candidate_directory) + "/" + kernel_name ||
            contract.known_good_image != known_good_image || contract.known_good_entry_path != known_good_entry ||
            !safe_mode(contract.known_good_entry_mode) || !safe_loader_entry(contract.active_loader_entry) ||
            !safe_mode(contract.active_loader_entry_mode) ||
            contract.active_loader_entry_mode != contract.known_good_entry_mode ||
            contract.active_loader_entry_original.empty() || contract.active_loader_entry_original.size() > 16384 ||
            contract.active_loader_entry_activated.empty() || contract.active_loader_entry_activated.size() > 16384 ||
            contract.known_good_entry.empty() || contract.known_good_entry.size() > 16384 ||
            !text(contract.known_good_entry).ends_with(" bootart=0 rd.bootart=0\n") ||
            contract.patched_init_functions_2nd.size() > 1024 * 1024) {
            throw std::runtime_error("boot-deploy contract violates its fixed paths or bounds");
        }
        const auto original = text(contract.active_loader_entry_original);
        const auto activated = text(contract.active_loader_entry_activated);
        const auto [original_kernel, original_options] = parse_mkinitfs_boot_deploy_loader_entry(original);
        static_cast<void>(original_options);
        const auto [activated_kernel, activated_options] = parse_mkinitfs_boot_deploy_loader_entry(activated);
        std::istringstream option_input(activated_options);
        std::string option;
        bool has_splash = false;
        while (option_input >> option)
            has_splash = has_splash || option == "splash";
        if (original_kernel != contract.kernel_image || activated_kernel != contract.kernel_image || has_splash ||
            activate_mkinitfs_boot_deploy_loader_entry(original) != contract.active_loader_entry_activated) {
            throw std::runtime_error("boot-deploy loader takeover is inconsistent");
        }
        const auto patched = text(contract.patched_init_functions_2nd);
        if (!integration::patch_boot_deploy_init_functions(patched,
                                                           integration::reviewed_boot_deploy_initramfs_version)) {
            throw std::runtime_error("boot-deploy patched init functions are inconsistent");
        }
        if (contract.android_boot) {
            const auto &android = *contract.android_boot;
            const auto safe_source = text(android.deviceinfo.no_flash_deviceinfo);
            const auto parsed = parse_android_boot_deviceinfo(safe_source, safe_source, true);
            const std::string suffix = "/" + android.deviceinfo.dtb + ".dtb";
            if (!parsed || parsed->no_flash_deviceinfo != android.deviceinfo.no_flash_deviceinfo ||
                android.deviceinfo_path != deviceinfo_path || android.candidate_boot_image != candidate_boot_image ||
                !safe_partition(android.partition) || android.dtb_bytes == 0 ||
                android.dtb_bytes > max_candidate_bytes || !android.dtb_path.starts_with("/boot/dtbs") ||
                !android.dtb_path.ends_with(suffix) || android.dtb_path.contains("/../") ||
                android.dtb_path.contains("//")) {
                throw std::runtime_error("Android boot generation differs from the no-flash contract");
            }
        }
    }

    DracutSystemdImageRecord verified_mkinitfs_boot_deploy_image_record(const MkinitfsBootDeployContract &contract,
                                                                        std::span<const std::byte> candidate,
                                                                        const ArchiveInspection &inspection,
                                                                        std::span<const std::byte> expected_bootart) {
        validate_mkinitfs_boot_deploy_contract(contract);
        if (candidate.empty() || candidate.size() > max_candidate_bytes ||
            inspection.bootart_digest != sha256(expected_bootart) || inspection.inspected_entries == 0 ||
            inspection.inspected_bytes == 0 || inspection.inspected_bytes > max_inspected_archive_bytes) {
            throw std::runtime_error("boot-deploy candidate inspection is incomplete");
        }
        const auto digest = sha256(candidate);
        DracutSystemdImageRecord record{contract.kernel_image.substr(6),
                                        contract.active_image,
                                        digest,
                                        contract.candidate_image,
                                        digest,
                                        candidate.size(),
                                        contract.known_good_image,
                                        contract.known_good_digest,
                                        contract.known_good_entry_path,
                                        sha256(contract.known_good_entry),
                                        contract.active_loader_entry,
                                        sha256(expected_bootart)};
        validate_mkinitfs_boot_deploy_image_record(record);
        return record;
    }

    void validate_mkinitfs_boot_deploy_image_record(const DracutSystemdImageRecord &record) {
        if (!safe_kernel_filename(record.kernel_version) || record.active_image != active_image ||
            record.candidate_image != candidate_image || record.known_good_image != known_good_image ||
            record.grub_script_path != known_good_entry || !safe_loader_entry(record.grub_config_path) ||
            record.candidate_bytes == 0 || record.candidate_bytes > max_candidate_bytes ||
            record.active_digest != record.candidate_digest) {
            throw std::runtime_error("boot-deploy image record violates the fixed contract");
        }
    }

    bool mkinitfs_boot_deploy_managed_image_path(std::string_view path) {
        return path == active_image || path == known_good_image || path == known_good_entry ||
               path == candidate_directory || path.starts_with(std::string(candidate_directory) + "/");
    }

} // namespace bootart::install
