#pragma once

#include "sart/animation.hpp"
#include "sart/art.hpp"
#include "sart/display.hpp"
#include "sart/terminal.hpp"

namespace sart {

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

} // namespace sart
