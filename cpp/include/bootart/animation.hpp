#pragma once

#include "bootart/art.hpp"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace bootart {

    enum class AnsiColor {
        reset,
        dark_gray,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        light_gray,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    };

    void append_ansi_color(std::string &output, AnsiColor color);
    [[nodiscard]] std::uint64_t cell_hash(std::uint64_t seed, std::size_t x, std::size_t y) noexcept;
    [[nodiscard]] float normalized_hash(std::uint64_t seed, std::size_t x, std::size_t y) noexcept;
    [[nodiscard]] float smoothstep(float progress) noexcept;

    struct AnimatedCell {
        std::size_t x{};
        std::size_t y{};
        char32_t glyph{};
        float reveal_threshold{};
        std::uint8_t color_phase{};
    };

    class AnimationMetadata {
      public:
        AnimationMetadata(const Art &art, std::uint64_t seed);

        [[nodiscard]] const AnimatedCell *cell_at(std::size_t x, std::size_t y) const noexcept;
        [[nodiscard]] std::uint64_t seed() const noexcept;
        [[nodiscard]] std::size_t width() const noexcept;
        [[nodiscard]] std::size_t height() const noexcept;
        [[nodiscard]] const std::vector<AnimatedCell> &cells() const noexcept;

      private:
        std::uint64_t seed_{};
        std::size_t width_{};
        std::size_t height_{};
        std::vector<AnimatedCell> cells_;
        std::vector<std::size_t> cell_indices_;
    };

    struct ColoredGlyph {
        char32_t glyph{};
        AnsiColor color{AnsiColor::reset};
    };

    [[nodiscard]] std::optional<ColoredGlyph> cell_color_at(const AnimatedCell &cell, float smooth_progress,
                                                            std::size_t art_width, std::size_t art_height,
                                                            bool no_color, std::size_t iteration);

} // namespace bootart
