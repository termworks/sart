#include "sart/visual/animation.hpp"

#include <algorithm>
#include <array>
#include <bit>
#include <cmath>
#include <limits>
#include <numbers>
#include <string_view>

namespace sart::visual {
    namespace {

        constexpr std::size_t missing_index = std::numeric_limits<std::size_t>::max();

        std::int32_t angle_effective_position(std::size_t x, std::size_t y, std::size_t width, std::size_t height,
                                              std::size_t angle_index) {
            const auto center_x = static_cast<float>(x) - static_cast<float>(width) / 2.0F;
            const auto center_y = static_cast<float>(y) * 2.0F - static_cast<float>(height);
            const auto angle = static_cast<float>(angle_index % 12) * std::numbers::pi_v<float> / 6.0F;
            return static_cast<std::int32_t>(center_x * std::cos(angle) + center_y * std::sin(angle));
        }

    } // namespace

    void append_ansi_color(std::string &output, AnsiColor color) {
        static constexpr std::array<std::string_view, 16> sequences{
            "\x1b[0m",  "\x1b[90m", "\x1b[31m", "\x1b[32m", "\x1b[33m", "\x1b[34m", "\x1b[35m", "\x1b[36m",
            "\x1b[37m", "\x1b[91m", "\x1b[92m", "\x1b[93m", "\x1b[94m", "\x1b[95m", "\x1b[96m", "\x1b[97m",
        };
        output.append(sequences.at(static_cast<std::size_t>(color)));
    }

    std::uint64_t cell_hash(std::uint64_t seed, std::size_t x, std::size_t y) noexcept {
        auto value = seed ^ ((static_cast<std::uint64_t>(x) << 32) | static_cast<std::uint64_t>(y));
        value += 0x9E3779B97F4A7C15ULL;
        value = (value ^ (value >> 30)) * 0xBF58476D1CE4E5B9ULL;
        value = (value ^ (value >> 27)) * 0x94D049BB133111EBULL;
        return value ^ (value >> 31);
    }

    float normalized_hash(std::uint64_t seed, std::size_t x, std::size_t y) noexcept {
        return static_cast<float>(static_cast<double>(cell_hash(seed, x, y)) /
                                  static_cast<double>(std::numeric_limits<std::uint64_t>::max()));
    }

    float smoothstep(float progress) noexcept {
        const auto clamped = std::clamp(progress, 0.0F, 1.0F);
        return clamped * clamped * (3.0F - 2.0F * clamped);
    }

    AnimationMetadata::AnimationMetadata(const Art &art, std::uint64_t seed)
        : seed_(seed), width_(art.width()), height_(art.height()) {
        cell_indices_.assign(width_ * height_, missing_index);
        for (std::size_t y = 0; y < height_; ++y) {
            for (std::size_t x = 0; x < width_; ++x) {
                const auto glyph = art.cell(x, y);
                if (glyph == U' ') {
                    continue;
                }
                cell_indices_[y * width_ + x] = cells_.size();
                cells_.push_back({x, y, glyph, 0.0F, 0});
            }
        }
    }

    const AnimatedCell *AnimationMetadata::cell_at(std::size_t x, std::size_t y) const noexcept {
        if (x >= width_ || y >= height_) {
            return nullptr;
        }
        const auto index = cell_indices_[y * width_ + x];
        return index == missing_index ? nullptr : &cells_[index];
    }

    std::uint64_t AnimationMetadata::seed() const noexcept { return seed_; }

    std::size_t AnimationMetadata::width() const noexcept { return width_; }

    std::size_t AnimationMetadata::height() const noexcept { return height_; }

    const std::vector<AnimatedCell> &AnimationMetadata::cells() const noexcept { return cells_; }

    std::optional<ColoredGlyph> cell_color_at(const AnimatedCell &cell, float smooth_progress, std::size_t art_width,
                                              std::size_t art_height, bool no_color, std::size_t iteration) {
        if (no_color) {
            return ColoredGlyph{cell.glyph, AnsiColor::light_gray};
        }

        const auto span_source = art_width + art_height * 2 + 30;
        const auto maximum_span = static_cast<std::int32_t>(static_cast<float>(span_source) * 0.75F);
        const auto first_direction = static_cast<std::size_t>(cell_hash(iteration ^ 0x3000, 0, 0) % 12);
        const auto second_direction = static_cast<std::size_t>((cell_hash(iteration ^ 0x4000, 0, 0) + 1) % 12);
        static constexpr std::array<AnsiColor, 12> palette{
            AnsiColor::dark_gray,     AnsiColor::red,   AnsiColor::bright_red,   AnsiColor::yellow,
            AnsiColor::bright_yellow, AnsiColor::green, AnsiColor::bright_green, AnsiColor::cyan,
            AnsiColor::bright_cyan,   AnsiColor::blue,  AnsiColor::bright_blue,  AnsiColor::magenta,
        };

        std::array<AnsiColor, 7> first_colors{};
        const auto first_shift = (iteration * 5 + 1) % palette.size();
        for (std::size_t index = 0; index < 6; ++index) {
            first_colors[index] = palette[(index + first_shift) % palette.size()];
        }
        first_colors[6] = AnsiColor::bright_white;

        std::array<AnsiColor, 7> second_colors{};
        const auto second_shift = (iteration * 5 + 7) % palette.size();
        for (std::size_t index = 0; index < 6; ++index) {
            second_colors[index] = palette[palette.size() - 1 - (index + second_shift) % palette.size()];
        }
        second_colors[6] = AnsiColor::reset;

        const auto swept_color = [&](float sub_progress, std::size_t direction, const auto &colors) {
            const auto sweep =
                static_cast<std::int32_t>(sub_progress * (static_cast<float>(maximum_span) * 2.0F + 30.0F));
            const auto position = angle_effective_position(cell.x, cell.y, art_width, art_height, direction);
            const auto front = position - maximum_span + sweep;
            const auto offset =
                static_cast<std::int32_t>((cell_hash(42 ^ static_cast<std::uint64_t>(sweep), cell.x, cell.y) % 15) + 1);
            for (std::int32_t index = 6; index >= 0; --index) {
                if (front > index * 3 + offset) {
                    return std::optional<AnsiColor>{colors[static_cast<std::size_t>(index)]};
                }
            }
            return std::optional<AnsiColor>{};
        };

        if (smooth_progress < 0.45F) {
            const auto color = swept_color(smooth_progress / 0.45F, first_direction, first_colors);
            return color ? std::optional<ColoredGlyph>{{cell.glyph, *color}} : std::nullopt;
        }
        if (smooth_progress < 0.55F) {
            return ColoredGlyph{cell.glyph, AnsiColor::bright_white};
        }
        if (smooth_progress < 0.95F) {
            const auto color = swept_color((smooth_progress - 0.55F) / 0.40F, second_direction, second_colors);
            if (!color) {
                return ColoredGlyph{cell.glyph, AnsiColor::bright_white};
            }
            if (*color == AnsiColor::reset) {
                return std::nullopt;
            }
            return ColoredGlyph{cell.glyph, *color};
        }
        return std::nullopt;
    }

} // namespace sart::visual
