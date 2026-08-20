#include "bootart/integration.hpp"

#include "bootart/sha256.hpp"

#include <algorithm>
#include <charconv>
#include <format>
#include <stdexcept>

namespace bootart::integration {
    namespace {

        void append(std::vector<std::byte> &output, std::string_view text) {
            const auto bytes = std::as_bytes(std::span(text.data(), text.size()));
            output.insert(output.end(), bytes.begin(), bytes.end());
        }

        std::size_t hex_field(std::span<const std::byte> data, std::size_t offset) {
            if (offset + 8 > data.size())
                throw std::runtime_error("truncated cpio field");
            const auto text = std::string_view(reinterpret_cast<const char *>(data.data() + offset), 8);
            std::size_t value{};
            const auto [end, error] = std::from_chars(text.data(), text.data() + text.size(), value, 16);
            if (error != std::errc{} || end != text.data() + text.size())
                throw std::runtime_error("invalid cpio field");
            return value;
        }

        void pad(std::vector<std::byte> &output) {
            while (output.size() % 4 != 0)
                output.push_back(std::byte{});
        }

    } // namespace

    std::vector<std::byte> build_cpio_archive(std::span<const CpioInput> files) {
        std::vector<std::byte> output;
        const auto emit = [&](std::string_view name, std::span<const std::byte> bytes, std::uint32_t mode) {
            const auto header =
                std::format("070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}00000000", 0,
                            mode, 0, 0, 1, 0, bytes.size(), 0, 0, 0, 0, name.size() + 1);
            append(output, header);
            append(output, name);
            output.push_back(std::byte{});
            pad(output);
            output.insert(output.end(), bytes.begin(), bytes.end());
            pad(output);
        };
        for (const auto &file : files)
            emit(file.name, file.bytes, file.mode);
        emit("TRAILER!!!", {}, 0);
        return output;
    }

    std::vector<CpioEntry> parse_cpio_archive(std::span<const std::byte> data) {
        std::vector<CpioEntry> entries;
        std::size_t cursor{};
        while (cursor + 110 <= data.size()) {
            const auto magic = std::string_view(reinterpret_cast<const char *>(data.data() + cursor), 6);
            if (magic != "070701" && magic != "070702" && magic != "070707")
                break;
            const auto mode = hex_field(data, cursor + 14);
            const auto size = hex_field(data, cursor + 54);
            const auto name_size = hex_field(data, cursor + 94);
            cursor += 110;
            if (name_size == 0 || cursor + name_size > data.size())
                throw std::runtime_error("truncated cpio name");
            auto name = std::string(reinterpret_cast<const char *>(data.data() + cursor), name_size - 1);
            cursor += name_size;
            cursor = (cursor + 3) & ~std::size_t(3);
            if (name == "TRAILER!!!")
                break;
            if (cursor + size > data.size())
                throw std::runtime_error("truncated cpio content");
            entries.push_back({std::move(name), static_cast<std::uint32_t>(mode),
                               std::vector<std::byte>(data.begin() + cursor, data.begin() + cursor + size)});
            cursor += size;
            cursor = (cursor + 3) & ~std::size_t(3);
        }
        return entries;
    }

    CandidateReport inspect_candidate_archive(std::span<const std::byte> data, std::string_view expected,
                                              AdapterId adapter) {
        if (data.empty())
            throw std::runtime_error("empty candidate archive");
        const auto entries = parse_cpio_archive(data);
        const auto executable = std::ranges::find_if(entries, [](const CpioEntry &entry) {
            return entry.name.ends_with("bootart") || entry.name == "initramfs-init" ||
                   entry.name == "usr/share/mkinitfs/initramfs-init";
        });
        if (executable == entries.end())
            throw std::runtime_error("candidate has no bootart ELF");
        const auto elf_digest = sha256(executable->bytes);
        if (elf_digest != expected)
            throw std::runtime_error("candidate bootart digest mismatch");
        const auto has = [&](std::string_view needle) {
            return std::ranges::any_of(entries, [&](const CpioEntry &entry) { return entry.name.contains(needle); });
        };
        if ((adapter == AdapterId::dracut_systemd || adapter == AdapterId::dracut_classic) && !has("dracut")) {
            throw std::runtime_error("candidate has no dracut resource");
        }
        if (adapter == AdapterId::initramfs_tools_busybox && !has("hooks") && !has("initramfs-tools")) {
            throw std::runtime_error("candidate has no initramfs-tools resource");
        }
        if (adapter == AdapterId::mkinitcpio_busybox && !has("hooks") && !has("initcpio")) {
            throw std::runtime_error("candidate has no mkinitcpio resource");
        }
        if ((adapter == AdapterId::mkinitfs_busybox || adapter == AdapterId::mkinitfs_boot_deploy) &&
            !has("mkinitfs") && !has("init")) {
            throw std::runtime_error("candidate has no mkinitfs resource");
        }
        return {sha256(data), elf_digest, entries.size()};
    }

} // namespace bootart::integration
