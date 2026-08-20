#pragma once

#include <cstdint>

namespace bootart {

    inline constexpr int pid1_refusal_exit_code = 126;
    [[nodiscard]] bool process_is_allowed(std::uint32_t process_id) noexcept;

} // namespace bootart
