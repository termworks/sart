#pragma once

#include "bootart/splash/protocol.hpp"
#include "bootart/splash/state.hpp"

#include <filesystem>
#include <string>

namespace bootart::splash {

    class RootTransition {
      public:
        virtual ~RootTransition() = default;
        virtual void transition(const std::filesystem::path &new_root) = 0;
    };

    class DeferredRootTransition final : public RootTransition {
      public:
        void transition(const std::filesystem::path &new_root) override;
    };

    class LinuxSelfRootTransition final : public RootTransition {
      public:
        void transition(const std::filesystem::path &new_root) override;
    };

    struct CommandOutcome {
        Frame response;
        bool should_quit{};
        bool retain_splash{};
        bool fatal_root_transition{};
    };

    [[nodiscard]] bool is_mutating(Opcode opcode) noexcept;
    [[nodiscard]] CommandOutcome handle_request(SplashState &state, const Frame &request);
    [[nodiscard]] CommandOutcome handle_request(SplashState &state, const Frame &request,
                                                RootTransition &root_transition);
    [[nodiscard]] std::string state_json(const SplashState &state);
    [[nodiscard]] std::string_view mode_name(Mode mode) noexcept;

} // namespace bootart::splash
