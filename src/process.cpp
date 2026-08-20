#include "sart/process.hpp"

namespace sart {

    bool process_is_allowed(std::uint32_t process_id) noexcept { return process_id != 1; }

} // namespace sart
