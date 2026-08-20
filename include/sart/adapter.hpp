#pragma once

#include "sart/embedded.hpp"

#include <array>
#include <cstdint>
#include <span>
#include <string_view>

namespace sart {

    enum class AdapterId : std::uint8_t {
        dracut_systemd,
        systemd_real_root,
        dracut_classic,
        initramfs_tools_busybox,
        mkinitcpio_busybox,
        mkinitfs_busybox,
        mkinitfs_boot_deploy,
        openrc_real_root,
    };

    inline constexpr std::array adapter_ids{
        AdapterId::dracut_systemd,          AdapterId::systemd_real_root,  AdapterId::dracut_classic,
        AdapterId::initramfs_tools_busybox, AdapterId::mkinitcpio_busybox, AdapterId::mkinitfs_busybox,
        AdapterId::mkinitfs_boot_deploy,    AdapterId::openrc_real_root,
    };

    enum class AdapterKind : std::uint8_t {
        initramfs_runtime,
        real_root_supervisor,
    };

    enum class SupportStatus : std::uint8_t {
        experimental_unproven,
        proven_supported,
    };

    enum class PasswordBrokerStatus : std::uint8_t {
        not_applicable,
        not_integrated,
        integrated_unproven,
    };

    struct AdapterMetadata {
        AdapterId id;
        std::string_view name;
        AdapterKind kind;
        PasswordBrokerStatus password_broker;
        std::span<const embedded::TemplateId> resources;
        std::string_view limitation;
    };

    struct AdapterPairMetadata {
        std::string_view proof_slug;
        AdapterId initramfs;
        AdapterId real_root;
        SupportStatus status;
        std::span<const std::string_view> proof_gates;
        std::string_view limitation;
    };

    const AdapterMetadata &adapter_metadata(AdapterId id);
    std::span<const AdapterPairMetadata> adapter_pairs();
    const AdapterPairMetadata *adapter_pair(AdapterId initramfs, AdapterId real_root);
    std::string_view adapter_name(AdapterId id);

} // namespace sart
