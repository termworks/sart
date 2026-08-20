#pragma once

#include "bootart/animation.hpp"
#include "bootart/art.hpp"
#include "bootart/display.hpp"
#include "bootart/terminal.hpp"

namespace bootart {

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

} // namespace bootart
