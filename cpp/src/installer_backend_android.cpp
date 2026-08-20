#include "bootart/installer_backends.hpp"

#include "bootart/sha256.hpp"

#include <algorithm>
#include <charconv>
#include <limits>
#include <map>
#include <stdexcept>

namespace bootart::install {
    namespace {

        bool ascii_alnum(unsigned char byte) {
            return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z');
        }

        bool safe_label(std::string_view value) {
            return !value.empty() && value.size() <= 64 && std::ranges::all_of(value, [](unsigned char byte) {
                return ascii_alnum(byte) || byte == '_' || byte == '-' || byte == '.';
            });
        }

        std::string trim(std::string_view value) {
            const auto first = value.find_first_not_of(" \t\r\n");
            if (first == std::string_view::npos)
                return {};
            const auto last = value.find_last_not_of(" \t\r\n");
            return std::string(value.substr(first, last - first + 1));
        }

        std::string literal_value(std::string_view input) {
            auto value = trim(input);
            if (value.size() >= 2 &&
                ((value.front() == '"' && value.back() == '"') || (value.front() == '\'' && value.back() == '\''))) {
                value = value.substr(1, value.size() - 2);
            } else if (std::ranges::any_of(value, [](unsigned char byte) { return byte == ' ' || byte == '\t'; })) {
                throw std::runtime_error("deviceinfo has unquoted whitespace");
            }
            if (value.size() > 4096 || std::ranges::any_of(value, [](unsigned char byte) {
                    return byte > 0x7f || byte == 0 || byte == '\n' || byte == '\r' || byte == '\'' || byte == '"' ||
                           byte == '\\' || byte == '$' || byte == '`';
                })) {
                throw std::runtime_error("deviceinfo value uses unsupported shell syntax");
            }
            return value;
        }

        std::map<std::string, std::string> literals(std::string_view source) {
            if (source.size() > 256 * 1024 || source.contains('\0'))
                throw std::runtime_error("deviceinfo is oversized");
            std::map<std::string, std::string> values;
            std::size_t offset = 0;
            while (offset <= source.size()) {
                const auto end = source.find('\n', offset);
                auto line =
                    trim(source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset));
                if (!line.empty() && line.front() != '#') {
                    if (line.starts_with("export "))
                        line = trim(std::string_view(line).substr(7));
                    const auto equal = line.find('=');
                    if (equal == std::string::npos)
                        throw std::runtime_error("deviceinfo contains a non-assignment");
                    const auto name = trim(std::string_view(line).substr(0, equal));
                    if (!name.starts_with("deviceinfo_") || name.size() > 128 ||
                        !std::ranges::all_of(name,
                                             [](unsigned char byte) { return ascii_alnum(byte) || byte == '_'; })) {
                        throw std::runtime_error("deviceinfo assignment name is unsafe");
                    }
                    values[name] = literal_value(std::string_view(line).substr(equal + 1));
                }
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return values;
        }

        bool literal_bool(std::string_view source, std::string_view key) {
            if (source.contains('\0'))
                throw std::runtime_error("deviceinfo contains NUL");
            std::size_t offset = 0;
            while (offset <= source.size()) {
                const auto end = source.find('\n', offset);
                auto line =
                    trim(source.substr(offset, end == std::string_view::npos ? source.size() - offset : end - offset));
                if (!line.empty() && line.front() != '#' && line.contains(key)) {
                    if (line.starts_with("export "))
                        line = trim(std::string_view(line).substr(7));
                    const auto equal = line.find('=');
                    if (equal == std::string::npos || trim(std::string_view(line).substr(0, equal)) != key) {
                        throw std::runtime_error("deviceinfo boolean uses unreviewed shell syntax");
                    }
                    const auto value = trim(std::string_view(line).substr(equal + 1));
                    if (value == "true" || value == "\"true\"" || value == "'true'")
                        return true;
                    if (!value.empty() && value != "false" && value != "\"false\"" && value != "'false'" &&
                        value != "\"\"" && value != "''") {
                        throw std::runtime_error("deviceinfo boolean is dynamic");
                    }
                }
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return false;
        }

        std::uint32_t little_u32(std::span<const std::byte> image, std::size_t offset) {
            if (offset > image.size() || image.size() - offset < 4)
                throw std::runtime_error("truncated Android u32");
            std::uint32_t value = 0;
            for (std::size_t index = 0; index < 4; ++index)
                value |= std::to_integer<std::uint32_t>(image[offset + index]) << (8 * index);
            return value;
        }

        std::uint64_t little_u64(std::span<const std::byte> image, std::size_t offset) {
            if (offset > image.size() || image.size() - offset < 8)
                throw std::runtime_error("truncated Android u64");
            std::uint64_t value = 0;
            for (std::size_t index = 0; index < 8; ++index)
                value |= std::to_integer<std::uint64_t>(image[offset + index]) << (8 * index);
            return value;
        }

        std::size_t aligned(std::size_t value, std::size_t page) {
            if (page == 0 || value > std::numeric_limits<std::size_t>::max() - (page - 1)) {
                throw std::runtime_error("Android layout overflow");
            }
            return (value + page - 1) / page * page;
        }

        std::uint64_t rounded(std::uint64_t value, std::uint64_t unit) {
            if (value == 0 || unit == 0 || value > std::numeric_limits<std::uint64_t>::max() - (unit - 1)) {
                throw std::runtime_error("boot capacity input is invalid");
            }
            const auto blocks = (value + unit - 1) / unit;
            if (blocks > std::numeric_limits<std::uint64_t>::max() / unit)
                throw std::runtime_error("boot capacity overflow");
            return blocks * unit;
        }

    } // namespace

    std::string android_slot_partition_label(std::string_view command_line, std::string_view base_label) {
        if (!safe_label(base_label))
            throw std::runtime_error("Android partition base label is unsafe");
        std::optional<std::string_view> suffix;
        std::size_t offset = 0;
        while (offset < command_line.size()) {
            while (offset < command_line.size() && (command_line[offset] == ' ' || command_line[offset] == '\t'))
                ++offset;
            const auto end = command_line.find_first_of(" \t", offset);
            const auto token = command_line.substr(offset, end == std::string_view::npos ? command_line.size() - offset
                                                                                         : end - offset);
            constexpr std::string_view prefix = "androidboot.slot_suffix=";
            if (token.starts_with(prefix)) {
                const auto value = token.substr(prefix.size());
                if ((value != "_a" && value != "_b") || suffix)
                    throw std::runtime_error("Android slot suffix is ambiguous");
                suffix = value;
            }
            if (end == std::string_view::npos)
                break;
            offset = end + 1;
        }
        return std::string(base_label) + std::string(suffix.value_or(""));
    }

    std::optional<AndroidBootDeviceInfo> parse_android_boot_deviceinfo(std::string_view vendor_source,
                                                                       std::optional<std::string_view> override_source,
                                                                       bool accept_existing_bootart_guard) {
        auto values = literals(vendor_source);
        auto overrides = override_source ? literals(*override_source) : std::map<std::string, std::string>{};
        for (const auto &[name, value] : overrides)
            values[name] = value;
        const auto enabled = [&values](std::string_view key) {
            const auto found = values.find(std::string(key));
            return found != values.end() && found->second == "true";
        };
        if (!enabled("deviceinfo_generate_bootimg") && !enabled("deviceinfo_flash_kernel_on_update"))
            return std::nullopt;
        const bool managed_guard = accept_existing_bootart_guard && enabled("deviceinfo_generate_bootimg") &&
                                   !enabled("deviceinfo_flash_kernel_on_update") &&
                                   overrides.find("deviceinfo_flash_kernel_on_update") != overrides.end() &&
                                   overrides.at("deviceinfo_flash_kernel_on_update") == "false";
        if ((!enabled("deviceinfo_generate_bootimg") || !enabled("deviceinfo_flash_kernel_on_update")) &&
            !managed_guard) {
            throw std::runtime_error("Android boot generation and flashing are not one capability");
        }
        const auto required = [&values](std::string_view key) -> std::string {
            const auto found = values.find(std::string(key));
            if (found == values.end() || found->second.empty())
                throw std::runtime_error("Android deviceinfo field is missing");
            return found->second;
        };
        const auto architecture = required("deviceinfo_arch");
        const auto codename = required("deviceinfo_codename");
        const auto flash_method = required("deviceinfo_flash_method");
        if (flash_method != "fastboot")
            throw std::runtime_error("Android flash method is unsupported");
        const auto header_text = required("deviceinfo_header_version");
        std::uint32_t header{};
        const auto [end, error] = std::from_chars(header_text.data(), header_text.data() + header_text.size(), header);
        if (error != std::errc{} || end != header_text.data() + header_text.size() || header != 2) {
            throw std::runtime_error("Android header version is unsupported");
        }
        const auto partition_found = values.find("deviceinfo_flash_fastboot_partition_kernel");
        const auto partition = partition_found == values.end() || partition_found->second.empty()
                                   ? std::string("boot")
                                   : partition_found->second;
        if (!safe_label(partition))
            throw std::runtime_error("Android partition label is unsafe");
        const auto dtb = required("deviceinfo_dtb");
        bool safe_dtb = !dtb.empty() && dtb.size() <= 256 && dtb.front() != '/';
        std::size_t component_start = 0;
        while (safe_dtb && component_start < dtb.size()) {
            const auto slash = dtb.find('/', component_start);
            const auto component = std::string_view(dtb).substr(
                component_start, slash == std::string::npos ? dtb.size() - component_start : slash - component_start);
            safe_dtb = !component.empty() && component != "." && component != ".." &&
                       std::ranges::all_of(component, [](unsigned char byte) {
                           return ascii_alnum(byte) || byte == '_' || byte == '-' || byte == '.' || byte == '+';
                       });
            if (slash == std::string::npos)
                break;
            component_start = slash + 1;
        }
        if (!safe_dtb) {
            throw std::runtime_error("Android DTB name is unsafe");
        }
        values["deviceinfo_flash_kernel_on_update"] = "false";
        std::string no_flash;
        for (const auto &[name, value] : values)
            no_flash += name + "='" + value + "'\n";
        return AndroidBootDeviceInfo{architecture,
                                     codename,
                                     flash_method,
                                     partition,
                                     dtb,
                                     header,
                                     {reinterpret_cast<const std::byte *>(no_flash.data()),
                                      reinterpret_cast<const std::byte *>(no_flash.data() + no_flash.size())}};
    }

    bool deviceinfo_enables_kernel_flash(std::string_view source) {
        return literal_bool(source, "deviceinfo_flash_kernel_on_update");
    }

    bool deviceinfo_generates_android_boot_image(std::string_view source) {
        return literal_bool(source, "deviceinfo_generate_bootimg");
    }

    AndroidBootImageInspection inspect_android_boot_image_v2(std::span<const std::byte> image,
                                                             std::span<const std::byte> expected_kernel,
                                                             std::span<const std::byte> expected_ramdisk,
                                                             std::span<const std::byte> expected_dtb) {
        constexpr std::size_t header_size = 1660;
        constexpr std::array magic{std::byte{'A'}, std::byte{'N'}, std::byte{'D'}, std::byte{'R'},
                                   std::byte{'O'}, std::byte{'I'}, std::byte{'D'}, std::byte{'!'}};
        if (image.size() < header_size || image.size() > max_candidate_bytes ||
            !std::ranges::equal(image.first(8), magic))
            throw std::runtime_error("invalid Android v2 image");
        const auto kernel_size = little_u32(image, 8);
        const auto ramdisk_size = little_u32(image, 16);
        const auto second_size = little_u32(image, 24);
        const auto page_size = little_u32(image, 36);
        const auto version = little_u32(image, 40);
        const auto recovery_size = little_u32(image, 1632);
        const auto recovery_offset = little_u64(image, 1636);
        const auto declared_header = little_u32(image, 1644);
        const auto dtb_size = little_u32(image, 1648);
        if (version != 2 || declared_header != header_size || page_size < 2048 || page_size > 65536 ||
            (page_size & (page_size - 1)) != 0 || kernel_size == 0 || ramdisk_size == 0 || dtb_size == 0 ||
            second_size != 0 || recovery_size != 0 || recovery_offset != 0 || kernel_size != expected_kernel.size() ||
            ramdisk_size != expected_ramdisk.size() || dtb_size != expected_dtb.size())
            throw std::runtime_error("Android v2 component contract differs");
        const auto page = static_cast<std::size_t>(page_size);
        const auto kernel_start = page;
        if (kernel_size > image.size() - kernel_start)
            throw std::runtime_error("Android kernel range is truncated");
        const auto kernel_end = kernel_start + kernel_size;
        const auto ramdisk_start = aligned(kernel_end, page);
        if (ramdisk_start > image.size() || ramdisk_size > image.size() - ramdisk_start)
            throw std::runtime_error("Android ramdisk is truncated");
        const auto ramdisk_end = ramdisk_start + ramdisk_size;
        const auto dtb_start = aligned(ramdisk_end, page);
        if (dtb_start > image.size() || dtb_size > image.size() - dtb_start)
            throw std::runtime_error("Android DTB is truncated");
        const auto dtb_end = dtb_start + dtb_size;
        const auto padded_end = aligned(dtb_end, page);
        if (padded_end != image.size() ||
            !std::ranges::equal(image.subspan(kernel_start, kernel_size), expected_kernel) ||
            !std::ranges::equal(image.subspan(ramdisk_start, ramdisk_size), expected_ramdisk) ||
            !std::ranges::equal(image.subspan(dtb_start, dtb_size), expected_dtb)) {
            throw std::runtime_error("Android v2 bytes differ from candidate inputs");
        }
        for (const auto padding :
             {image.subspan(header_size, kernel_start - header_size),
              image.subspan(kernel_end, ramdisk_start - kernel_end),
              image.subspan(ramdisk_end, dtb_start - ramdisk_end), image.subspan(dtb_end, padded_end - dtb_end)}) {
            if (std::ranges::any_of(padding, [](std::byte byte) { return byte != std::byte{}; })) {
                throw std::runtime_error("Android v2 padding is not zero-filled");
            }
        }
        return {page_size, kernel_size, ramdisk_size, dtb_size, sha256(image)};
    }

    void restore_android_boot_partition(std::vector<std::byte> &partition, std::span<const std::byte> original) {
        if (original.empty() || original.size() > max_transaction_bytes || partition.size() < original.size()) {
            throw std::runtime_error("raw boot restore is outside the transaction bound");
        }
        std::ranges::copy(original, partition.begin());
        if (!std::ranges::equal(std::span(partition).first(original.size()), original)) {
            throw std::runtime_error("raw boot restore verification failed");
        }
    }

    std::string activate_android_boot_partition(std::vector<std::byte> &partition, std::span<const std::byte> original,
                                                std::span<const std::byte> candidate) {
        if (original.empty() || original.size() > max_transaction_bytes || candidate.empty() ||
            candidate.size() > original.size() || partition.size() < original.size() ||
            !std::ranges::equal(std::span(partition).first(original.size()), original)) {
            throw std::runtime_error("raw boot activation is outside or differs from the journal contract");
        }
        try {
            std::ranges::copy(candidate, partition.begin());
            std::vector<std::byte> expected(original.begin(), original.end());
            std::ranges::copy(candidate, expected.begin());
            if (!std::ranges::equal(std::span(partition).first(original.size()), expected)) {
                throw std::runtime_error("raw boot activation verification failed");
            }
            return sha256(std::span(partition).first(original.size()));
        } catch (...) {
            restore_android_boot_partition(partition, original);
            throw;
        }
    }

    MkinitfsBootDeployCompression detect_mkinitfs_boot_deploy_compression(std::span<const std::byte> image) {
        const std::array gzip{std::byte{0x1f}, std::byte{0x8b}, std::byte{0x08}};
        const std::array zstd{std::byte{0x28}, std::byte{0xb5}, std::byte{0x2f}, std::byte{0xfd}};
        if (image.size() >= gzip.size() && std::ranges::equal(image.first(gzip.size()), gzip)) {
            return MkinitfsBootDeployCompression::gzip;
        }
        if (image.size() >= zstd.size() && std::ranges::equal(image.first(zstd.size()), zstd)) {
            return MkinitfsBootDeployCompression::zstandard;
        }
        throw std::runtime_error("unsupported mkinitfs boot-deploy compression");
    }

    std::uint64_t mkinitfs_boot_deploy_initial_boot_bytes(std::uint64_t kernel_bytes, std::uint64_t active_bytes,
                                                          std::uint64_t unit) {
        const auto kernel = rounded(kernel_bytes, unit);
        const auto active = rounded(active_bytes, unit);
        if (kernel > std::numeric_limits<std::uint64_t>::max() - active ||
            kernel + active > std::numeric_limits<std::uint64_t>::max() - unit) {
            throw std::runtime_error("initial boot capacity overflow");
        }
        return kernel + active + unit;
    }

    std::uint64_t mkinitfs_boot_deploy_preservation_bytes(std::uint64_t active_bytes, std::uint64_t entry_bytes,
                                                          std::uint64_t unit) {
        const auto active = rounded(active_bytes, unit);
        const auto entry = rounded(entry_bytes, unit);
        if (active > std::numeric_limits<std::uint64_t>::max() - entry)
            throw std::runtime_error("preservation capacity overflow");
        return active + entry;
    }

} // namespace bootart::install
