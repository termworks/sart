#pragma once

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace bootart::integration {

    enum class AdapterId {
        dracut_systemd,
        systemd_real_root,
        dracut_classic,
        initramfs_tools_busybox,
        mkinitcpio_busybox,
        mkinitfs_busybox,
        mkinitfs_boot_deploy,
        openrc_real_root,
    };

    struct CpioInput {
        std::string_view name;
        std::span<const std::byte> bytes;
        std::uint32_t mode;
    };
    struct CpioEntry {
        std::string name;
        std::uint32_t mode;
        std::vector<std::byte> bytes;
    };
    struct CandidateReport {
        std::string candidate_digest;
        std::string elf_digest;
        std::size_t entries_count;
    };

    [[nodiscard]] std::vector<std::byte> build_cpio_archive(std::span<const CpioInput> files);
    [[nodiscard]] std::vector<CpioEntry> parse_cpio_archive(std::span<const std::byte> data);
    [[nodiscard]] CandidateReport inspect_candidate_archive(std::span<const std::byte> data,
                                                            std::string_view expected_elf_digest, AdapterId adapter);

} // namespace bootart::integration
