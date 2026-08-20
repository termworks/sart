#pragma once

#include <compare>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <variant>

namespace bootart::splash {

    inline constexpr std::size_t max_display_text_bytes = 4 * 1024;
    inline constexpr std::size_t max_prompt_text_bytes = 1024;
    inline constexpr std::size_t max_prompt_source_bytes = 256;

    enum class Lifecycle { starting, running, deactivated, quitting, stopped, failed_open };
    enum class BaseView { hidden, splash, details };
    enum class Mode { boot, shutdown, reboot, update, upgrade };
    enum class RootStage { initramfs, switching, real_root };
    enum class PromptOutcome { answered, cancelled, timed_out, request_gone };
    enum class TransitionResult { changed, unchanged };

    enum class TextErrorCode { empty, too_long, unsafe_character, invalid_utf8 };

    class TextError final : public std::runtime_error {
      public:
        TextError(TextErrorCode code, std::string message, std::size_t byte_index = 0, std::uint32_t codepoint = 0);
        [[nodiscard]] TextErrorCode code() const noexcept;
        [[nodiscard]] std::size_t byte_index() const noexcept;
        [[nodiscard]] std::uint32_t codepoint() const noexcept;

      private:
        TextErrorCode code_;
        std::size_t byte_index_;
        std::uint32_t codepoint_;
    };

    void validate_display_text(std::string_view value, std::size_t maximum_bytes);

    class PromptMetadata {
      public:
        PromptMetadata(std::uint64_t request_id, std::string text);
        PromptMetadata &with_source(std::string source);
        PromptMetadata &with_requester_pid(std::uint32_t requester_pid) noexcept;
        PromptMetadata &with_echo(bool echo) noexcept;
        PromptMetadata &with_silent(bool silent) noexcept;
        PromptMetadata &with_expiry(std::uint64_t expires_at_milliseconds) noexcept;

        [[nodiscard]] std::uint64_t request_id() const noexcept;
        [[nodiscard]] const std::string &text() const noexcept;
        [[nodiscard]] const std::optional<std::string> &source() const noexcept;
        [[nodiscard]] std::optional<std::uint32_t> requester_pid() const noexcept;
        [[nodiscard]] bool echo() const noexcept;
        [[nodiscard]] bool silent() const noexcept;
        [[nodiscard]] std::optional<std::uint64_t> expires_at_milliseconds() const noexcept;
        auto operator<=>(const PromptMetadata &) const = default;

      private:
        std::uint64_t request_id_;
        std::string text_;
        std::optional<std::string> source_;
        std::optional<std::uint32_t> requester_pid_;
        bool echo_{};
        bool silent_{};
        std::optional<std::uint64_t> expires_at_milliseconds_;
    };

    class View {
      public:
        explicit View(BaseView base = BaseView::splash);
        static View prompt(BaseView previous_view, PromptMetadata metadata);

        [[nodiscard]] std::optional<BaseView> base_view() const noexcept;
        [[nodiscard]] const PromptMetadata *prompt_metadata() const noexcept;
        [[nodiscard]] BaseView previous_view() const;
        auto operator<=>(const View &) const = default;

      private:
        std::optional<BaseView> base_;
        std::optional<PromptMetadata> prompt_;
        BaseView previous_{BaseView::splash};
    };

    struct MarkRunning {};
    struct Show {};
    struct Hide {};
    struct ShowDetails {};
    struct HideDetails {};
    struct ToggleDetails {};
    struct Deactivate {};
    struct Reactivate {};
    struct SetMode {
        Mode value;
    };
    struct SetRootStage {
        RootStage value;
    };
    struct SetStatus {
        std::optional<std::string> value;
    };
    struct SetMessage {
        std::optional<std::string> value;
    };
    struct SetProgress {
        std::optional<std::uint8_t> value;
    };
    struct BeginPrompt {
        PromptMetadata metadata;
    };
    struct FinishPrompt {
        std::uint64_t request_id;
        PromptOutcome outcome;
    };
    struct Quit {};
    struct MarkStopped {};
    struct FailOpen {};

    using StateAction = std::variant<MarkRunning, Show, Hide, ShowDetails, HideDetails, ToggleDetails, Deactivate,
                                     Reactivate, SetMode, SetRootStage, SetStatus, SetMessage, SetProgress, BeginPrompt,
                                     FinishPrompt, Quit, MarkStopped, FailOpen>;

    enum class StateErrorCode {
        invalid_lifecycle_transition,
        invalid_root_transition,
        invalid_progress,
        invalid_text,
        prompt_active,
        prompt_conflict,
        prompt_id_mismatch,
        no_active_prompt,
    };

    class StateError final : public std::runtime_error {
      public:
        StateError(StateErrorCode code, std::string message);
        [[nodiscard]] StateErrorCode code() const noexcept;

      private:
        StateErrorCode code_;
    };

    class SplashState {
      public:
        explicit SplashState(Mode mode = Mode::boot);
        [[nodiscard]] Lifecycle lifecycle() const noexcept;
        [[nodiscard]] const View &view() const noexcept;
        [[nodiscard]] Mode mode() const noexcept;
        [[nodiscard]] RootStage root_stage() const noexcept;
        [[nodiscard]] const std::optional<std::string> &status() const noexcept;
        [[nodiscard]] const std::optional<std::string> &message() const noexcept;
        [[nodiscard]] std::optional<std::uint8_t> progress() const noexcept;
        TransitionResult apply(StateAction action);
        auto operator<=>(const SplashState &) const = default;

      private:
        void require_running(std::string_view operation) const;
        void require_presentable(std::string_view operation) const;
        [[nodiscard]] TransitionResult set_view(BaseView target, std::string_view operation);
        [[nodiscard]] TransitionResult set_root_stage(RootStage target, std::string_view operation);
        [[nodiscard]] TransitionResult finish_prompt(std::uint64_t request_id);
        void retire_prompt();

        Lifecycle lifecycle_{Lifecycle::starting};
        View view_;
        Mode mode_;
        RootStage root_stage_{RootStage::initramfs};
        std::optional<std::string> status_;
        std::optional<std::string> message_;
        std::optional<std::uint8_t> progress_;
        std::optional<std::uint64_t> last_finished_prompt_id_;
    };

} // namespace bootart::splash
