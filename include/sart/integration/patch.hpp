#pragma once

#include <cstdint>
#include <expected>
#include <string>
#include <string_view>

namespace sart::integration {

    enum class PatchError : std::uint8_t {
        unsupported_version,
        partial_managed_state,
        ambiguous_early_insertion_point,
        ambiguous_handoff_insertion_point,
        ambiguous_unlock_function,
        managed_content_mismatch,
    };

    inline constexpr std::string_view reviewed_mkinitfs_version = "3.14.0-r0";
    inline constexpr std::string_view reviewed_boot_deploy_initramfs_version = "3.12.0-r0";

    std::expected<std::string, PatchError> patch_mkinitfs_init(std::string_view input);
    std::expected<std::string, PatchError> patch_boot_deploy_init_functions(std::string_view input,
                                                                            std::string_view version);

} // namespace sart::integration
