#include "sart/frame_engine.hpp"

#include <limits>

namespace sart {
    namespace {

        Color map_color(AnsiColor color) {
            switch (color) {
            case AnsiColor::reset:
                return Color::default_color;
            case AnsiColor::dark_gray:
                return Color::bright_black;
            case AnsiColor::red:
                return Color::red;
            case AnsiColor::green:
                return Color::green;
            case AnsiColor::yellow:
                return Color::yellow;
            case AnsiColor::blue:
                return Color::blue;
            case AnsiColor::magenta:
                return Color::magenta;
            case AnsiColor::cyan:
                return Color::cyan;
            case AnsiColor::light_gray:
                return Color::white;
            case AnsiColor::bright_red:
                return Color::bright_red;
            case AnsiColor::bright_green:
                return Color::bright_green;
            case AnsiColor::bright_yellow:
                return Color::bright_yellow;
            case AnsiColor::bright_blue:
                return Color::bright_blue;
            case AnsiColor::bright_magenta:
                return Color::bright_magenta;
            case AnsiColor::bright_cyan:
                return Color::bright_cyan;
            case AnsiColor::bright_white:
                return Color::bright_white;
            }
            return Color::default_color;
        }

    } // namespace

    FrameEngine::FrameEngine(const Art &art, std::uint64_t seed) : art_(art), metadata_(art, seed) {}

    Size FrameEngine::art_size() const noexcept { return art_.size(); }

    Scene FrameEngine::render(TerminalSize terminal_size, float progress, bool no_color, std::size_t iteration) const {
        if (terminal_size.width > std::numeric_limits<std::uint16_t>::max() ||
            terminal_size.height > std::numeric_limits<std::uint16_t>::max()) {
            throw SceneError(SceneErrorCode::too_large, "terminal dimensions are unsupported");
        }
        const Dimensions dimensions(static_cast<std::uint16_t>(terminal_size.width),
                                    static_cast<std::uint16_t>(terminal_size.height));
        Scene scene(dimensions);
        const auto placement = layout(art_.size(), {terminal_size.width, terminal_size.height});
        for (std::size_t row = 0; row < placement.visible_height; ++row) {
            for (std::size_t column = 0; column < placement.visible_width; ++column) {
                const auto *animated = metadata_.cell_at(placement.source_x + column, placement.source_y + row);
                if (!animated)
                    continue;
                const auto colored =
                    cell_color_at(*animated, progress, art_.width(), art_.height(), no_color, iteration);
                if (!colored)
                    continue;
                scene.set(static_cast<std::uint16_t>(placement.destination_x + column),
                          static_cast<std::uint16_t>(placement.destination_y + row),
                          Cell(colored->glyph, {map_color(colored->color), Color::default_color, false}));
            }
        }
        return scene;
    }

} // namespace sart
