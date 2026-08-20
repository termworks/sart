#include "bootart/renderer.hpp"

#include "bootart/signals.hpp"

#include <algorithm>
#include <chrono>
#include <format>
#include <thread>

namespace bootart {

    const Art &select_art(const Art &art, const Art *small_art, TerminalSize terminal_size) noexcept {
        const auto fits = art.width() <= terminal_size.width && art.height() <= terminal_size.height;
        if (!fits && small_art != nullptr && small_art->width() <= terminal_size.width &&
            small_art->height() <= terminal_size.height) {
            return *small_art;
        }
        return art;
    }

    std::string generate_frame_bytes(const Art &art, const AnimationMetadata &metadata, const Layout &layout_info,
                                     FrameOptions options) {
        std::string output;
        output.reserve(4096);
        if (options.first_frame) {
            output.append("\x1b[?25l");
            append_ansi_color(output, AnsiColor::reset);
            if (options.clear_first) {
                output.append("\x1b[2J");
            }
        }

        std::optional<AnsiColor> active_color;
        for (std::size_t row = 0; row < layout_info.visible_height; ++row) {
            const auto art_y = layout_info.source_y + row;
            output.append(
                std::format("\x1b[{};{}H", layout_info.destination_y + row + 1, layout_info.destination_x + 1));
            for (std::size_t column = 0; column < layout_info.visible_width; ++column) {
                const auto art_x = layout_info.source_x + column;
                auto glyph = U' ';
                auto color = AnsiColor::reset;
                if (const auto *cell = metadata.cell_at(art_x, art_y)) {
                    if (const auto colored = cell_color_at(*cell, options.progress, art.width(), art.height(),
                                                           options.no_color, options.iteration)) {
                        glyph = colored->glyph;
                        color = colored->color;
                    }
                }
                if (!active_color || *active_color != color) {
                    append_ansi_color(output, color);
                    active_color = color;
                }
                output.append(encode_utf8(glyph));
            }
            if (!active_color || *active_color != AnsiColor::reset) {
                append_ansi_color(output, AnsiColor::reset);
                active_color = AnsiColor::reset;
            }
        }
        return output;
    }

    std::string build_exit_bytes(const Layout &layout_info, TerminalSize terminal_size) {
        std::string output;
        append_ansi_color(output, AnsiColor::reset);
        output.append("\x1b[?25h");
        const auto final_y =
            terminal_size.height > 0
                ? std::min(layout_info.destination_y + layout_info.visible_height + 1, terminal_size.height)
                : 1;
        output.append(std::format("\x1b[{};1H", final_y));
        return output;
    }

    void play_animation(TerminalOutput &terminal, const Art &art, const Art *small_art, RenderOptions options,
                        std::size_t iteration) {
        const auto terminal_size = terminal.dimensions();
        const auto &selected = select_art(art, small_art, terminal_size);
        const auto layout_info = layout(selected.size(), {terminal_size.width, terminal_size.height});
        const AnimationMetadata metadata(selected, options.seed);
        const auto frames_per_second = std::clamp<std::uint64_t>(options.frames_per_second, 1, 60);
        const auto duration = std::clamp<std::uint64_t>(options.duration_milliseconds, 100, 10000);
        const auto frame_count = std::max<std::uint64_t>(duration * frames_per_second / 1000, 1);
        const auto frame_period = std::chrono::microseconds(1'000'000 / frames_per_second);
        const auto start = std::chrono::steady_clock::now();
        std::exception_ptr render_error;
        try {
            for (std::uint64_t index = 0; index < frame_count && !signals::should_stop(); ++index) {
                const auto progress =
                    frame_count <= 1 ? 1.0F : static_cast<float>(index) / static_cast<float>(frame_count - 1);
                const auto frame = generate_frame_bytes(
                    selected, metadata, layout_info,
                    {progress, options.no_color, index == 0, options.clear_first && iteration == 0, iteration});
                terminal.write_frame(frame);
                terminal.flush();
                const auto deadline = start + frame_period * static_cast<std::int64_t>(index + 1);
                std::this_thread::sleep_until(deadline);
            }
        } catch (...) {
            render_error = std::current_exception();
        }
        try {
            terminal.write_frame(build_exit_bytes(layout_info, terminal_size));
            terminal.flush();
        } catch (...) {
            if (!render_error) {
                throw;
            }
        }
        if (render_error) {
            std::rethrow_exception(render_error);
        }
    }

    void render_final(TerminalOutput &terminal, const Art &art, const Art *small_art, bool no_color) {
        const auto terminal_size = terminal.dimensions();
        const auto &selected = select_art(art, small_art, terminal_size);
        const auto layout_info = layout(selected.size(), {terminal_size.width, terminal_size.height});
        const AnimationMetadata metadata(selected, 42);
        terminal.write_frame(generate_frame_bytes(selected, metadata, layout_info, {0.5F, no_color, true, true, 0}));
        terminal.write_frame(build_exit_bytes(layout_info, terminal_size));
        terminal.flush();
    }

} // namespace bootart
