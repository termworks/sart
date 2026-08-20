#include "bootart/installer_backends.hpp"

#include <algorithm>
#include <array>
#include <stdexcept>

namespace bootart::install {

    ToolFact ToolFact::exact(std::string_view path) { return {std::string(path), true, true, false, true}; }

    std::string_view cryptsetup_executable(CryptsetupLocation location) {
        return location == CryptsetupLocation::usr_bin ? "/usr/bin/cryptsetup" : "/usr/sbin/cryptsetup";
    }

    std::string_view grub_updater(GrubRegeneration regeneration) {
        switch (regeneration) {
        case GrubRegeneration::update_grub:
            return "/usr/sbin/update-grub";
        case GrubRegeneration::grub2_mkconfig:
            return "/usr/bin/grub2-mkconfig";
        case GrubRegeneration::grub_mkconfig:
            return "/usr/bin/grub-mkconfig";
        }
        throw std::invalid_argument("invalid GRUB regeneration mechanism");
    }

    std::string_view grub_probe(GrubRegeneration regeneration) {
        switch (regeneration) {
        case GrubRegeneration::update_grub:
            return "/usr/sbin/grub-probe";
        case GrubRegeneration::grub2_mkconfig:
            return "/usr/bin/grub2-probe";
        case GrubRegeneration::grub_mkconfig:
            return "/usr/bin/grub-probe";
        }
        throw std::invalid_argument("invalid GRUB regeneration mechanism");
    }

    std::string_view grub_config_path(GrubRegeneration regeneration) {
        return regeneration == GrubRegeneration::grub2_mkconfig ? "/boot/grub2/grub.cfg" : "/boot/grub/grub.cfg";
    }

    std::vector<std::string> grub_arguments(GrubRegeneration regeneration) {
        if (regeneration == GrubRegeneration::update_grub)
            return {};
        return {"-o", std::string(grub_config_path(regeneration))};
    }

    std::vector<std::byte> render_grub_script(std::string_view boot_uuid, std::string_view kernel,
                                              std::string_view command_line, std::string_view known_good_image) {
        const auto safe_token = [](std::string_view value, std::size_t maximum) {
            return !value.empty() && value.size() <= maximum && std::ranges::all_of(value, [](unsigned char byte) {
                const bool alnum =
                    (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z');
                return alnum || byte == '-' || byte == '.' || byte == '_' || byte == '+';
            });
        };
        const auto safe_uuid = [](std::string_view value) {
            return !value.empty() && value.size() <= 64 && std::ranges::all_of(value, [](unsigned char byte) {
                return (byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f') || (byte >= 'A' && byte <= 'F') ||
                       byte == '-';
            });
        };
        if (!safe_uuid(boot_uuid) || !safe_token(kernel, 128) || command_line.empty() || command_line.size() > 4096 ||
            command_line.contains("BOOTART_GRUB_EOF") ||
            std::ranges::any_of(command_line,
                                [](unsigned char byte) { return byte != '\t' && (byte < ' ' || byte > '~'); }) ||
            !known_good_image.starts_with("/boot/")) {
            throw std::runtime_error("unsafe GRUB known-good input");
        }
        const auto image = known_good_image.substr(6);
        if (!safe_token(image, 256))
            throw std::runtime_error("unsafe GRUB known-good image");
        const std::string script = "#!/bin/sh\nset -eu\ncat <<'BOOTART_GRUB_EOF'\n"
                                   "menuentry 'Bootart known-good' --id bootart-known-good {\n"
                                   "    search --no-floppy --fs-uuid --set=root " +
                                   std::string(boot_uuid) +
                                   "\n"
                                   "    linux /vmlinuz-" +
                                   std::string(kernel) + " " + std::string(command_line) +
                                   "\n"
                                   "    initrd /" +
                                   std::string(image) + "\n}\nBOOTART_GRUB_EOF\n";
        return {reinterpret_cast<const std::byte *>(script.data()),
                reinterpret_cast<const std::byte *>(script.data() + script.size())};
    }

} // namespace bootart::install
