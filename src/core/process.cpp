#include "sart/core/process.hpp"

namespace sart::core {

    bool process_is_allowed(std::uint32_t process_id) noexcept { return process_id != 1; }

} // namespace sart::core
