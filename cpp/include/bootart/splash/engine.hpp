#pragma once

#include "bootart/art.hpp"
#include "bootart/display.hpp"
#include "bootart/frame_engine.hpp"
#include "bootart/splash/state.hpp"

#include <chrono>
#include <memory>

namespace bootart::splash {}

namespace bootart::password {
    class PromptCoordinator;
}

namespace bootart::splash {

    inline constexpr std::uint16_t default_frames_per_second = 30;
    inline constexpr std::uint16_t maximum_frames_per_second = 60;
    inline constexpr auto default_animation_cycle = std::chrono::milliseconds(2500);
    inline constexpr auto minimum_animation_cycle = std::chrono::milliseconds(100);
    inline constexpr auto maximum_animation_cycle = std::chrono::milliseconds(60000);

    struct EngineConfig {
        std::uint16_t frames_per_second{default_frames_per_second};
        std::chrono::milliseconds animation_cycle{default_animation_cycle};
        std::uint64_t seed{42};
        bool no_color{};
        void validate() const;
        auto operator<=>(const EngineConfig &) const = default;
    };

    struct EngineTick {
        bool frame_rendered{};
        bool stopped{};
    };

    class SplashEngine {
      public:
        SplashEngine(std::unique_ptr<DisplayBackend> backend, const Art &main_art, const Art *small_art,
                     EngineConfig config = {});
        SplashEngine(const SplashEngine &) = delete;
        SplashEngine &operator=(const SplashEngine &) = delete;
        ~SplashEngine();

        [[nodiscard]] DisplayBackend &backend() noexcept;
        void start(SplashState &state);
        [[nodiscard]] EngineTick tick_at(SplashState &state, std::chrono::steady_clock::duration elapsed,
                                         password::PromptCoordinator *prompt = nullptr);
        [[nodiscard]] std::chrono::steady_clock::duration
        time_until_next_frame(std::chrono::steady_clock::duration elapsed) const noexcept;
        void restore();
        void shutdown(bool retain_splash);

      private:
        void reconcile_display(const SplashState &state);
        void process_input(SplashState &state, password::PromptCoordinator *prompt);
        void render_frame(const SplashState &state, std::chrono::steady_clock::duration elapsed,
                          password::PromptCoordinator *prompt);
        [[noreturn]] void fail_open(SplashState &state, const std::exception &operation);

        std::unique_ptr<DisplayBackend> backend_;
        FrameEngine main_;
        std::optional<FrameEngine> small_;
        EngineConfig config_;
        std::chrono::steady_clock::duration frame_period_;
        std::chrono::steady_clock::duration next_frame_{};
        bool acquired_{};
        bool restored_{};
    };

    void apply_overlays(Scene &scene, const SplashState &state);
    [[nodiscard]] std::pair<float, std::size_t> animation_position(std::chrono::steady_clock::duration elapsed,
                                                                   std::chrono::milliseconds cycle) noexcept;

} // namespace bootart::splash
