#pragma once

#include <cstdint>

namespace sart::core {

    inline constexpr int pid1_refusal_exit_code = 126;
    [[nodiscard]] bool process_is_allowed(std::uint32_t process_id) noexcept;

} // namespace sart::core

namespace sart {
    using core::pid1_refusal_exit_code;
    using core::process_is_allowed;
} // namespace sart
