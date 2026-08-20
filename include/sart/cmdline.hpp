#pragma once

#include <filesystem>
#include <string_view>

namespace sart::cmdline {

    inline constexpr std::string_view proc_cmdline = "/proc/cmdline";

    [[nodiscard]] bool splash_disabled(std::string_view command_line) noexcept;
    [[nodiscard]] bool splash_disabled_at(const std::filesystem::path &path);
    [[nodiscard]] bool early_boot_enabled_at(const std::filesystem::path &path) noexcept;

} // namespace sart::cmdline
