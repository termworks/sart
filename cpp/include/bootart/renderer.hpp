#pragma once

#include "bootart/animation.hpp"
#include "bootart/art.hpp"
#include "bootart/terminal.hpp"

#include <cstddef>
#include <cstdint>
#include <string>

namespace bootart {

    struct RenderOptions {
        std::uint64_t duration_milliseconds{2500};
        std::uint64_t frames_per_second{30};
        std::uint64_t seed{42};
        bool no_color{};
        bool clear_first{true};
        bool leave_final{true};
    };

    struct FrameOptions {
        float progress{};
        bool no_color{};
        bool first_frame{};
        bool clear_first{};
        std::size_t iteration{};
    };

    [[nodiscard]] const Art &select_art(const Art &art, const Art *small_art, TerminalSize terminal_size) noexcept;
    [[nodiscard]] std::string generate_frame_bytes(const Art &art, const AnimationMetadata &metadata,
                                                   const Layout &layout, FrameOptions options);
    [[nodiscard]] std::string build_exit_bytes(const Layout &layout, TerminalSize terminal_size);
    void play_animation(TerminalOutput &terminal, const Art &art, const Art *small_art, RenderOptions options,
                        std::size_t iteration);
    void render_final(TerminalOutput &terminal, const Art &art, const Art *small_art, bool no_color);

} // namespace bootart
