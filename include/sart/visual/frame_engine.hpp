#pragma once

#include "sart/display/backend.hpp"
#include "sart/visual/animation.hpp"
#include "sart/visual/art.hpp"
#include "sart/visual/terminal.hpp"

namespace sart::visual {

    class FrameEngine {
      public:
        FrameEngine(const Art &art, std::uint64_t seed);
        [[nodiscard]] Size art_size() const noexcept;
        [[nodiscard]] Scene render(TerminalSize terminal_size, float progress, bool no_color,
                                   std::size_t iteration) const;

      private:
        const Art &art_;
        AnimationMetadata metadata_;
    };

} // namespace sart::visual

namespace sart {
    using visual::FrameEngine;
}
