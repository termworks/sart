#include "bootart/process.hpp"

namespace bootart {

    bool process_is_allowed(std::uint32_t process_id) noexcept { return process_id != 1; }

} // namespace bootart
