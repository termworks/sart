#include "bootart/splash/engine.hpp"

#include "bootart/password_coordinator.hpp"

#include <algorithm>
#include <format>
#include <limits>
#include <stdexcept>
#include <utility>

namespace bootart::splash {
    namespace {

        void clear_row(Scene &scene, std::uint16_t row) {
            for (std::uint16_t column = 0; column < scene.dimensions().columns(); ++column) {
                scene.set(column, row, Cell());
            }
        }

        void write_centered(Scene &scene, std::uint16_t row, std::string_view text, Style style) {
            const auto glyphs = decode_utf8(text);
            const auto width = static_cast<std::size_t>(scene.dimensions().columns());
            const auto count = std::min(width, glyphs.size());
            const auto start = (width - count) / 2;
            for (std::size_t index = 0; index < count; ++index) {
                scene.set(static_cast<std::uint16_t>(start + index), row, Cell(glyphs[index], style));
            }
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
        apply_overlays(scene, state);
        std::optional<std::pair<std::uint16_t, std::uint16_t>> sensitive;
        if (state.view().prompt_metadata() && prompt) {
            const auto row = static_cast<std::uint16_t>(dimensions->rows() >= 2 ? dimensions->rows() - 2 : 0);
            clear_row(scene, row);
            if (const auto feedback = prompt->feedback()) {
                const auto style = Style{Color::bright_white, Color::black, true};
                if (feedback->echo_mode == password::EchoMode::obscured) {
                    write_centered(scene, row, std::string(feedback->character_count, '*'), style);
                } else if (feedback->echo_mode == password::EchoMode::visible) {
                    sensitive = std::pair(row, dimensions->columns());
                }
            }
        }
        backend_->render(scene);
        if (sensitive && prompt) {
            prompt->with_visible_text([&](std::string_view text) {
                const auto glyph_count = std::min<std::size_t>(decode_utf8(text).size(), sensitive->second);
                const auto column = static_cast<std::uint16_t>((sensitive->second - glyph_count) / 2);
                backend_->render_sensitive_text(sensitive->first, column, text,
                                                {Color::bright_white, Color::black, true});
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
        struct Overlay {
            std::string text;
            Style style;
        };
        std::vector<Overlay> lines;
        if (state.view().prompt_metadata()) {
            lines.push_back({state.view().prompt_metadata()->text(), {Color::bright_white, Color::black, true}});
        } else {
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
        }
        const auto count = std::min<std::size_t>(lines.size(), scene.dimensions().rows());
        for (std::size_t offset = 0; offset < count; ++offset) {
            const auto row = static_cast<std::uint16_t>(scene.dimensions().rows() - 1 - offset);
            clear_row(scene, row);
            write_centered(scene, row, lines[offset].text, lines[offset].style);
        }
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

} // namespace bootart::splash
