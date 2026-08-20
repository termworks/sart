#include "sart/splash/engine.hpp"

#include "sart/password_coordinator.hpp"

#include <algorithm>
#include <format>
#include <limits>
#include <stdexcept>
#include <utility>

namespace sart::splash {
    namespace {

        struct SensitiveLayout {
            std::uint16_t row;
            std::uint16_t column;
            std::uint16_t columns;
            Style style;
        };

        struct VisiblePrefix {
            std::string_view text;
            std::size_t cells;
        };

        std::pair<std::string_view, Color> mode_presentation(Mode mode);
        std::string progress_line(std::uint8_t progress, std::size_t width);

        void clear_row(Scene &scene, std::uint16_t row) {
            for (std::uint16_t column = 0; column < scene.dimensions().columns(); ++column) {
                scene.set(column, row, Cell());
            }
        }

        void write_glyphs_at(Scene &scene, std::uint16_t row, std::uint16_t column, std::u32string_view glyphs,
                             Style style) {
            const auto available = static_cast<std::size_t>(scene.dimensions().columns() - column);
            const auto count = std::min(available, glyphs.size());
            for (std::size_t offset = 0; offset < count; ++offset) {
                scene.set(static_cast<std::uint16_t>(column + offset), row, Cell(glyphs[offset], style));
            }
        }

        void write_text_at(Scene &scene, std::uint16_t row, std::uint16_t column, std::string_view text, Style style) {
            write_glyphs_at(scene, row, column, decode_utf8(text), style);
        }

        void write_repeated_span(Scene &scene, std::uint16_t row, std::uint16_t column, std::size_t columns,
                                 char32_t glyph, Style style) {
            for (std::size_t offset = 0; offset < columns; ++offset) {
                scene.set(static_cast<std::uint16_t>(column + offset), row, Cell(glyph, style));
            }
        }

        void write_centered_in_span(Scene &scene, std::uint16_t row, std::uint16_t column, std::size_t columns,
                                    std::string_view text, Style style) {
            const auto glyphs = decode_utf8(text);
            const auto count = std::min(columns, glyphs.size());
            const auto start = (columns - count) / 2;
            for (std::size_t index = 0; index < count; ++index) {
                scene.set(static_cast<std::uint16_t>(column + start + index), row, Cell(glyphs[index], style));
            }
        }

        void write_centered(Scene &scene, std::uint16_t row, std::string_view text, Style style) {
            write_centered_in_span(scene, row, 0, scene.dimensions().columns(), text, style);
        }

        VisiblePrefix visible_prefix(std::string_view text, std::size_t maximum_cells) {
            std::size_t end{};
            std::size_t cells{};
            while (end < text.size()) {
                const auto leading = static_cast<unsigned char>(text[end]);
                const auto bytes = leading < 0x80   ? std::size_t{1}
                                   : leading < 0xe0 ? std::size_t{2}
                                   : leading < 0xf0 ? std::size_t{3}
                                                    : std::size_t{4};
                const auto width = leading < 0x80 ? std::size_t{1} : std::size_t{2};
                if (cells + width > maximum_cells)
                    break;
                end += bytes;
                cells += width;
            }
            return {text.substr(0, end), cells};
        }

        std::optional<SensitiveLayout> apply_prompt_overlay(Scene &scene, std::string_view prompt,
                                                            std::optional<password::InputFeedback> feedback) {
            constexpr std::size_t box_rows = 4;
            constexpr std::size_t minimum_box_columns = 28;
            constexpr std::size_t maximum_box_columns = 54;
            constexpr std::size_t maximum_field_columns = 24;
            const auto dimensions = scene.dimensions();
            const auto screen_columns = static_cast<std::size_t>(dimensions.columns());
            const auto screen_rows = static_cast<std::size_t>(dimensions.rows());
            const auto panel_style = Style{Color::bright_white, Color::black, true};
            if (screen_columns < 8 || screen_rows < box_rows) {
                const auto prompt_row = (screen_rows - 1) / 2;
                const auto input_row = std::min(prompt_row + 1, screen_rows - 1);
                clear_row(scene, static_cast<std::uint16_t>(prompt_row));
                write_centered(scene, static_cast<std::uint16_t>(prompt_row), prompt, panel_style);
                clear_row(scene, static_cast<std::uint16_t>(input_row));
                if (!feedback)
                    return std::nullopt;
                if (feedback->echo_mode == password::EchoMode::obscured) {
                    write_centered(scene, static_cast<std::uint16_t>(input_row),
                                   std::string(std::min(feedback->character_count, screen_columns), '*'), panel_style);
                    return std::nullopt;
                }
                if (feedback->echo_mode == password::EchoMode::visible) {
                    return SensitiveLayout{static_cast<std::uint16_t>(input_row), 0, dimensions.columns(), panel_style};
                }
                return std::nullopt;
            }

            const auto prompt_columns = decode_utf8(prompt).size() + 4;
            const auto box_columns =
                std::min(std::clamp(prompt_columns, minimum_box_columns, maximum_box_columns), screen_columns);
            const auto left = (screen_columns - box_columns) / 2;
            const auto top = (screen_rows - box_rows) / 2;
            const auto inner_columns = box_columns - 2;
            std::u32string top_border(1, U'╭');
            top_border.append(inner_columns, U'─');
            top_border.push_back(U'╮');
            std::u32string bottom_border(1, U'╰');
            bottom_border.append(inner_columns, U'─');
            bottom_border.push_back(U'╯');

            for (auto row = top; row < top + box_rows; ++row) {
                clear_row(scene, static_cast<std::uint16_t>(row));
                write_repeated_span(scene, static_cast<std::uint16_t>(row), static_cast<std::uint16_t>(left),
                                    box_columns, U' ', panel_style);
            }
            write_glyphs_at(scene, static_cast<std::uint16_t>(top), static_cast<std::uint16_t>(left), top_border,
                            panel_style);
            write_glyphs_at(scene, static_cast<std::uint16_t>(top + box_rows - 1), static_cast<std::uint16_t>(left),
                            bottom_border, panel_style);
            write_text_at(scene, static_cast<std::uint16_t>(top + 1), static_cast<std::uint16_t>(left), "│",
                          panel_style);
            write_text_at(scene, static_cast<std::uint16_t>(top + 1),
                          static_cast<std::uint16_t>(left + box_columns - 1), "│", panel_style);
            write_centered_in_span(scene, static_cast<std::uint16_t>(top + 1), static_cast<std::uint16_t>(left + 1),
                                   inner_columns, prompt, panel_style);

            const auto field_columns = std::clamp(inner_columns - 2, std::size_t{2}, maximum_field_columns);
            const auto field_left = left + 1 + (inner_columns - field_columns) / 2;
            const auto field_inner_columns = field_columns - 2;
            const auto input_row = top + 2;
            write_text_at(scene, static_cast<std::uint16_t>(input_row), static_cast<std::uint16_t>(left), "│",
                          panel_style);
            write_text_at(scene, static_cast<std::uint16_t>(input_row),
                          static_cast<std::uint16_t>(left + box_columns - 1), "│", panel_style);
            write_text_at(scene, static_cast<std::uint16_t>(input_row), static_cast<std::uint16_t>(field_left), "[",
                          panel_style);
            write_text_at(scene, static_cast<std::uint16_t>(input_row),
                          static_cast<std::uint16_t>(field_left + field_columns - 1), "]", panel_style);

            if (!feedback)
                return std::nullopt;
            if (feedback->echo_mode == password::EchoMode::obscured) {
                write_centered_in_span(scene, static_cast<std::uint16_t>(input_row),
                                       static_cast<std::uint16_t>(field_left + 1), field_inner_columns,
                                       std::string(std::min(feedback->character_count, field_inner_columns), '*'),
                                       panel_style);
                return std::nullopt;
            }
            if (feedback->echo_mode == password::EchoMode::visible) {
                return SensitiveLayout{static_cast<std::uint16_t>(input_row),
                                       static_cast<std::uint16_t>(field_left + 1),
                                       static_cast<std::uint16_t>(field_inner_columns), panel_style};
            }
            return std::nullopt;
        }

        std::optional<SensitiveLayout> apply_overlays_with_feedback(Scene &scene, const SplashState &state,
                                                                    std::optional<password::InputFeedback> feedback) {
            if (const auto *metadata = state.view().prompt_metadata())
                return apply_prompt_overlay(scene, metadata->text(), feedback);

            struct Overlay {
                std::string text;
                Style style;
            };
            std::vector<Overlay> lines;
            if (state.message())
                lines.push_back({*state.message(), {Color::bright_yellow, {}, true}});
            if (state.status())
                lines.push_back({*state.status(), {Color::white, {}, false}});
            if (state.progress()) {
                lines.push_back(
                    {progress_line(*state.progress(), scene.dimensions().columns()), {Color::bright_cyan, {}, false}});
            }
            const auto [label, color] = mode_presentation(state.mode());
            lines.push_back({std::string(label), {color, {}, true}});
            const auto count = std::min<std::size_t>(lines.size(), scene.dimensions().rows());
            for (std::size_t offset = 0; offset < count; ++offset) {
                const auto row = static_cast<std::uint16_t>(scene.dimensions().rows() - 1 - offset);
                clear_row(scene, row);
                write_centered(scene, row, lines[offset].text, lines[offset].style);
            }
            return std::nullopt;
        }

        std::pair<std::string_view, Color> mode_presentation(Mode mode) {
            switch (mode) {
            case Mode::boot:
                return {"BOOTING", Color::bright_cyan};
            case Mode::shutdown:
                return {"SHUTTING DOWN", Color::bright_yellow};
            case Mode::reboot:
                return {"REBOOTING", Color::bright_magenta};
            case Mode::update:
                return {"UPDATING", Color::bright_blue};
            case Mode::upgrade:
                return {"UPGRADING", Color::bright_green};
            }
            return {"BOOTING", Color::bright_cyan};
        }

        std::string progress_line(std::uint8_t progress, std::size_t width) {
            if (width < 9)
                return std::format("{}%", progress);
            const auto bar_width = std::clamp<std::size_t>(width - 8, 1, 40);
            const auto filled = bar_width * progress / 100;
            return std::format("[{}{}] {:3}%", std::string(filled, '#'), std::string(bar_width - filled, '-'),
                               progress);
        }

    } // namespace

    void EngineConfig::validate() const {
        if (frames_per_second == 0 || frames_per_second > maximum_frames_per_second) {
            throw std::invalid_argument("animation FPS must be in 1..=60");
        }
        if (animation_cycle < minimum_animation_cycle || animation_cycle > maximum_animation_cycle) {
            throw std::invalid_argument("animation cycle must be in 100..=60000 milliseconds");
        }
    }

    SplashEngine::SplashEngine(std::unique_ptr<DisplayBackend> backend, const Art &main_art, const Art *small_art,
                               EngineConfig config)
        : backend_(std::move(backend)), main_(main_art, config.seed),
          small_(small_art ? std::optional<FrameEngine>(std::in_place, *small_art, config.seed) : std::nullopt),
          config_(config) {
        if (!backend_)
            throw std::invalid_argument("display backend is required");
        config_.validate();
        frame_period_ = std::chrono::nanoseconds(1'000'000'000ULL / config_.frames_per_second);
    }

    SplashEngine::~SplashEngine() {
        try {
            restore();
        } catch (...) {
        }
    }

    DisplayBackend &SplashEngine::backend() noexcept { return *backend_; }

    void SplashEngine::start(SplashState &state) {
        if (acquired_)
            return;
        if (restored_)
            throw std::runtime_error("display engine was already restored");
        try {
            backend_->acquire();
            acquired_ = true;
        } catch (const std::exception &error) {
            fail_open(state, error);
        }
    }

    EngineTick SplashEngine::tick_at(SplashState &state, std::chrono::steady_clock::duration elapsed,
                                     password::PromptCoordinator *prompt) {
        if (!acquired_)
            throw std::runtime_error("display engine was not started");
        if (restored_)
            return {false, true};
        if (state.lifecycle() == Lifecycle::quitting || state.lifecycle() == Lifecycle::stopped ||
            state.lifecycle() == Lifecycle::failed_open) {
            restore();
            return {false, true};
        }
        try {
            reconcile_display(state);
            process_input(state, prompt);
            reconcile_display(state);
            if (elapsed < next_frame_)
                return {false, false};
            next_frame_ = elapsed + frame_period_;
            if (backend_->state() == DisplayState::splash) {
                render_frame(state, elapsed, prompt);
                return {true, false};
            }
            return {false, false};
        } catch (const std::exception &error) {
            fail_open(state, error);
        }
    }

    std::chrono::steady_clock::duration
    SplashEngine::time_until_next_frame(std::chrono::steady_clock::duration elapsed) const noexcept {
        return elapsed >= next_frame_ ? std::chrono::steady_clock::duration::zero() : next_frame_ - elapsed;
    }

    void SplashEngine::restore() { shutdown(false); }

    void SplashEngine::shutdown(bool retain_splash) {
        if (restored_)
            return;
        restored_ = true;
        backend_->restore(retain_splash ? RestoreMode::retain_pixels : RestoreMode::clear);
    }

    void SplashEngine::reconcile_display(const SplashState &state) {
        switch (state.lifecycle()) {
        case Lifecycle::starting:
            backend_->hide();
            return;
        case Lifecycle::deactivated:
            backend_->hide();
            return;
        case Lifecycle::running:
            break;
        case Lifecycle::quitting:
        case Lifecycle::stopped:
        case Lifecycle::failed_open:
            restore();
            return;
        }
        if (state.view().prompt_metadata()) {
            backend_->show();
            return;
        }
        switch (*state.view().base_view()) {
        case BaseView::hidden:
            backend_->hide();
            break;
        case BaseView::details:
            backend_->details(true);
            break;
        case BaseView::splash:
            if (backend_->state() == DisplayState::details)
                backend_->details(false);
            else
                backend_->show();
            break;
        }
    }

    void SplashEngine::process_input(SplashState &state, password::PromptCoordinator *prompt) {
        for (std::size_t count = 0; count < 8; ++count) {
            auto event = backend_->poll_input(std::chrono::milliseconds(0));
            if (!event)
                return;
            if (event->kind() == InputEvent::Kind::resized) {
                next_frame_ = {};
            } else if (event->kind() == InputEvent::Kind::return_to_splash) {
                if (state.view().base_view() == BaseView::details) {
                    static_cast<void>(state.apply(HideDetails{}));
                }
            } else if (state.view().prompt_metadata() && prompt) {
                prompt->handle_input(state, event->byte_data());
            } else {
                const std::array<std::uint8_t, 1> escape{0x1b};
                if (!state.view().prompt_metadata() && event->equals_bytes(escape)) {
                    static_cast<void>(state.apply(ToggleDetails{}));
                }
            }
        }
    }

    void SplashEngine::render_frame(const SplashState &state, std::chrono::steady_clock::duration elapsed,
                                    password::PromptCoordinator *prompt) {
        const auto dimensions = backend_->dimensions();
        if (!dimensions)
            throw std::runtime_error("display backend has no dimensions");
        const auto main_size = main_.art_size();
        const auto main_fits = main_size.width <= dimensions->columns() && main_size.height <= dimensions->rows();
        const FrameEngine *renderer = &main_;
        if (!main_fits && small_) {
            const auto size = small_->art_size();
            if (size.width <= dimensions->columns() && size.height <= dimensions->rows()) {
                renderer = &*small_;
            }
        }
        const auto [progress, iteration] = animation_position(elapsed, config_.animation_cycle);
        auto scene =
            renderer->render({dimensions->columns(), dimensions->rows()}, progress, config_.no_color, iteration);
        const auto feedback = prompt ? prompt->feedback() : std::nullopt;
        const auto sensitive = apply_overlays_with_feedback(scene, state, feedback);
        backend_->render(scene);
        if (sensitive && prompt) {
            prompt->with_visible_text([&](std::string_view text) {
                const auto visible = visible_prefix(text, sensitive->columns);
                const auto column =
                    static_cast<std::uint16_t>(sensitive->column + (sensitive->columns - visible.cells) / 2);
                backend_->render_sensitive_text(sensitive->row, column, visible.text, sensitive->style);
            });
        }
    }

    [[noreturn]] void SplashEngine::fail_open(SplashState &state, const std::exception &operation) {
        try {
            static_cast<void>(state.apply(FailOpen{}));
        } catch (...) {
        }
        try {
            restore();
        } catch (const std::exception &restoration) {
            throw DisplayError(DisplayErrorCode::operation_and_restore,
                               std::format("{}; display restoration failed: {}", operation.what(), restoration.what()),
                               true);
        }
        throw std::runtime_error(operation.what());
    }

    void apply_overlays(Scene &scene, const SplashState &state) {
        static_cast<void>(apply_overlays_with_feedback(scene, state, std::nullopt));
    }

    std::pair<float, std::size_t> animation_position(std::chrono::steady_clock::duration elapsed,
                                                     std::chrono::milliseconds cycle) noexcept {
        const auto elapsed_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(elapsed).count();
        const auto cycle_ns =
            std::max<std::int64_t>(1, std::chrono::duration_cast<std::chrono::nanoseconds>(cycle).count());
        const auto safe_elapsed = std::max<std::int64_t>(0, elapsed_ns);
        return {
            static_cast<float>(static_cast<double>(safe_elapsed % cycle_ns) / cycle_ns),
            static_cast<std::size_t>(safe_elapsed / cycle_ns),
        };
    }

} // namespace sart::splash
