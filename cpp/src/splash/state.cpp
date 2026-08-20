#include "bootart/splash/state.hpp"

#include "bootart/art.hpp"

#include <format>
#include <type_traits>
#include <utility>

namespace bootart::splash {
    namespace {

        bool unsafe_character(char32_t value) {
            return value < 0x20 || (value >= 0x7f && value <= 0x9f) || value == 0x061c || value == 0x200e ||
                   value == 0x200f || value == 0x2028 || value == 0x2029 || (value >= 0x202a && value <= 0x202e) ||
                   (value >= 0x2066 && value <= 0x2069);
        }

        template <typename Value> TransitionResult replace_if_different(Value &current, Value replacement) {
            if (current == replacement) {
                return TransitionResult::unchanged;
            }
            current = std::move(replacement);
            return TransitionResult::changed;
        }

        std::string_view action_name(const StateAction &action) {
            return std::visit(
                [](const auto &value) -> std::string_view {
                    using Action = std::decay_t<decltype(value)>;
                    if constexpr (std::is_same_v<Action, MarkRunning>)
                        return "mark-running";
                    if constexpr (std::is_same_v<Action, Show>)
                        return "show";
                    if constexpr (std::is_same_v<Action, Hide>)
                        return "hide";
                    if constexpr (std::is_same_v<Action, ShowDetails>)
                        return "show-details";
                    if constexpr (std::is_same_v<Action, HideDetails>)
                        return "hide-details";
                    if constexpr (std::is_same_v<Action, ToggleDetails>)
                        return "toggle-details";
                    if constexpr (std::is_same_v<Action, Deactivate>)
                        return "deactivate";
                    if constexpr (std::is_same_v<Action, Reactivate>)
                        return "reactivate";
                    if constexpr (std::is_same_v<Action, SetMode>)
                        return "set-mode";
                    if constexpr (std::is_same_v<Action, SetRootStage>)
                        return "set-root-stage";
                    if constexpr (std::is_same_v<Action, SetStatus>)
                        return "set-status";
                    if constexpr (std::is_same_v<Action, SetMessage>)
                        return "set-message";
                    if constexpr (std::is_same_v<Action, SetProgress>)
                        return "set-progress";
                    if constexpr (std::is_same_v<Action, BeginPrompt>)
                        return "begin-prompt";
                    if constexpr (std::is_same_v<Action, FinishPrompt>)
                        return "finish-prompt";
                    if constexpr (std::is_same_v<Action, Quit>)
                        return "quit";
                    if constexpr (std::is_same_v<Action, MarkStopped>)
                        return "mark-stopped";
                    return "fail-open";
                },
                action);
        }

    } // namespace

    TextError::TextError(TextErrorCode code, std::string message, std::size_t byte_index, std::uint32_t codepoint)
        : std::runtime_error(std::move(message)), code_(code), byte_index_(byte_index), codepoint_(codepoint) {}

    TextErrorCode TextError::code() const noexcept { return code_; }
    std::size_t TextError::byte_index() const noexcept { return byte_index_; }
    std::uint32_t TextError::codepoint() const noexcept { return codepoint_; }

    void validate_display_text(std::string_view value, std::size_t maximum_bytes) {
        if (value.size() > maximum_bytes) {
            throw TextError(TextErrorCode::too_long,
                            std::format("text is {} bytes; maximum is {}", value.size(), maximum_bytes));
        }
        std::u32string decoded;
        try {
            decoded = decode_utf8(value);
        } catch (...) {
            throw TextError(TextErrorCode::invalid_utf8, "text is not valid UTF-8");
        }
        std::size_t byte_index{};
        for (const auto character : decoded) {
            if (unsafe_character(character)) {
                throw TextError(TextErrorCode::unsafe_character,
                                std::format("text contains unsafe character U+{:04X} at byte {}",
                                            static_cast<std::uint32_t>(character), byte_index),
                                byte_index, static_cast<std::uint32_t>(character));
            }
            byte_index += encode_utf8(character).size();
        }
    }

    PromptMetadata::PromptMetadata(std::uint64_t request_id, std::string text)
        : request_id_(request_id), text_(std::move(text)) {
        if (text_.empty()) {
            throw TextError(TextErrorCode::empty, "text must not be empty");
        }
        validate_display_text(text_, max_prompt_text_bytes);
    }

    PromptMetadata &PromptMetadata::with_source(std::string source) {
        if (source.empty()) {
            throw TextError(TextErrorCode::empty, "text must not be empty");
        }
        validate_display_text(source, max_prompt_source_bytes);
        source_ = std::move(source);
        return *this;
    }

    PromptMetadata &PromptMetadata::with_requester_pid(std::uint32_t requester_pid) noexcept {
        requester_pid_ = requester_pid;
        return *this;
    }

    PromptMetadata &PromptMetadata::with_echo(bool echo) noexcept {
        echo_ = echo;
        return *this;
    }

    PromptMetadata &PromptMetadata::with_silent(bool silent) noexcept {
        silent_ = silent;
        return *this;
    }

    PromptMetadata &PromptMetadata::with_expiry(std::uint64_t expires_at_milliseconds) noexcept {
        expires_at_milliseconds_ = expires_at_milliseconds;
        return *this;
    }

    std::uint64_t PromptMetadata::request_id() const noexcept { return request_id_; }
    const std::string &PromptMetadata::text() const noexcept { return text_; }
    const std::optional<std::string> &PromptMetadata::source() const noexcept { return source_; }
    std::optional<std::uint32_t> PromptMetadata::requester_pid() const noexcept { return requester_pid_; }
    bool PromptMetadata::echo() const noexcept { return echo_; }
    bool PromptMetadata::silent() const noexcept { return silent_; }
    std::optional<std::uint64_t> PromptMetadata::expires_at_milliseconds() const noexcept {
        return expires_at_milliseconds_;
    }

    View::View(BaseView base) : base_(base), previous_(base) {}

    View View::prompt(BaseView previous_view, PromptMetadata metadata) {
        View result(previous_view);
        result.base_.reset();
        result.prompt_ = std::move(metadata);
        result.previous_ = previous_view;
        return result;
    }

    std::optional<BaseView> View::base_view() const noexcept { return base_; }
    const PromptMetadata *View::prompt_metadata() const noexcept { return prompt_ ? &*prompt_ : nullptr; }

    BaseView View::previous_view() const {
        if (!prompt_) {
            throw std::logic_error("view has no active prompt");
        }
        return previous_;
    }

    StateError::StateError(StateErrorCode code, std::string message)
        : std::runtime_error(std::move(message)), code_(code) {}

    StateErrorCode StateError::code() const noexcept { return code_; }

    SplashState::SplashState(Mode mode) : mode_(mode) {}
    Lifecycle SplashState::lifecycle() const noexcept { return lifecycle_; }
    const View &SplashState::view() const noexcept { return view_; }
    Mode SplashState::mode() const noexcept { return mode_; }
    RootStage SplashState::root_stage() const noexcept { return root_stage_; }
    const std::optional<std::string> &SplashState::status() const noexcept { return status_; }
    const std::optional<std::string> &SplashState::message() const noexcept { return message_; }
    std::optional<std::uint8_t> SplashState::progress() const noexcept { return progress_; }

    void SplashState::require_running(std::string_view operation) const {
        if (lifecycle_ != Lifecycle::running) {
            throw StateError(StateErrorCode::invalid_lifecycle_transition,
                             std::format("operation {} is invalid for the current lifecycle", operation));
        }
    }

    void SplashState::require_presentable(std::string_view operation) const {
        if (lifecycle_ != Lifecycle::starting && lifecycle_ != Lifecycle::running &&
            lifecycle_ != Lifecycle::deactivated) {
            throw StateError(StateErrorCode::invalid_lifecycle_transition,
                             std::format("operation {} is invalid for the current lifecycle", operation));
        }
    }

    TransitionResult SplashState::set_view(BaseView target, std::string_view operation) {
        require_running(operation);
        if (view_.prompt_metadata() != nullptr) {
            return TransitionResult::unchanged;
        }
        return replace_if_different(view_, View(target));
    }

    TransitionResult SplashState::set_root_stage(RootStage target, std::string_view operation) {
        require_presentable(operation);
        if (view_.prompt_metadata() != nullptr) {
            throw StateError(StateErrorCode::prompt_active, "a prompt has priority over this operation");
        }
        if (root_stage_ == target) {
            return TransitionResult::unchanged;
        }
        const auto valid = (root_stage_ == RootStage::initramfs && target == RootStage::switching) ||
                           (root_stage_ == RootStage::switching && target == RootStage::real_root);
        if (!valid) {
            throw StateError(StateErrorCode::invalid_root_transition, "invalid root stage transition");
        }
        root_stage_ = target;
        return TransitionResult::changed;
    }

    TransitionResult SplashState::finish_prompt(std::uint64_t request_id) {
        require_running("finish-prompt");
        if (const auto *metadata = view_.prompt_metadata()) {
            if (metadata->request_id() != request_id) {
                throw StateError(StateErrorCode::prompt_id_mismatch, "prompt completion ID does not match");
            }
            const auto previous = view_.previous_view();
            view_ = View(previous);
            last_finished_prompt_id_ = request_id;
            return TransitionResult::changed;
        }
        if (last_finished_prompt_id_ == request_id) {
            return TransitionResult::unchanged;
        }
        throw StateError(StateErrorCode::no_active_prompt, "there is no active prompt");
    }

    void SplashState::retire_prompt() {
        if (const auto *metadata = view_.prompt_metadata()) {
            const auto request_id = metadata->request_id();
            const auto previous = view_.previous_view();
            view_ = View(previous);
            last_finished_prompt_id_ = request_id;
        }
    }

    TransitionResult SplashState::apply(StateAction action) {
        const auto operation = action_name(action);
        try {
            return std::visit(
                [&](auto &&value) -> TransitionResult {
                    using Action = std::decay_t<decltype(value)>;
                    if constexpr (std::is_same_v<Action, MarkRunning>) {
                        if (lifecycle_ == Lifecycle::starting) {
                            lifecycle_ = Lifecycle::running;
                            return TransitionResult::changed;
                        }
                        if (lifecycle_ == Lifecycle::running)
                            return TransitionResult::unchanged;
                        require_running(operation);
                    } else if constexpr (std::is_same_v<Action, Show>) {
                        return set_view(BaseView::splash, operation);
                    } else if constexpr (std::is_same_v<Action, Hide>) {
                        return set_view(BaseView::hidden, operation);
                    } else if constexpr (std::is_same_v<Action, ShowDetails>) {
                        return set_view(BaseView::details, operation);
                    } else if constexpr (std::is_same_v<Action, HideDetails>) {
                        return set_view(BaseView::splash, operation);
                    } else if constexpr (std::is_same_v<Action, ToggleDetails>) {
                        require_running(operation);
                        const auto base = view_.base_view();
                        if (!base)
                            return TransitionResult::unchanged;
                        return replace_if_different(
                            view_, View(*base == BaseView::details ? BaseView::splash : BaseView::details));
                    } else if constexpr (std::is_same_v<Action, Deactivate>) {
                        if (view_.prompt_metadata()) {
                            throw StateError(StateErrorCode::prompt_active,
                                             "a prompt has priority over this operation");
                        }
                        if (lifecycle_ == Lifecycle::running) {
                            lifecycle_ = Lifecycle::deactivated;
                            return TransitionResult::changed;
                        }
                        if (lifecycle_ == Lifecycle::deactivated)
                            return TransitionResult::unchanged;
                        throw StateError(StateErrorCode::invalid_lifecycle_transition, "cannot deactivate");
                    } else if constexpr (std::is_same_v<Action, Reactivate>) {
                        if (lifecycle_ == Lifecycle::deactivated) {
                            lifecycle_ = Lifecycle::running;
                            return TransitionResult::changed;
                        }
                        if (lifecycle_ == Lifecycle::running)
                            return TransitionResult::unchanged;
                        throw StateError(StateErrorCode::invalid_lifecycle_transition, "cannot reactivate");
                    } else if constexpr (std::is_same_v<Action, SetMode>) {
                        require_presentable(operation);
                        return replace_if_different(mode_, value.value);
                    } else if constexpr (std::is_same_v<Action, SetRootStage>) {
                        return set_root_stage(value.value, operation);
                    } else if constexpr (std::is_same_v<Action, SetStatus>) {
                        require_presentable(operation);
                        if (value.value)
                            validate_display_text(*value.value, max_display_text_bytes);
                        return replace_if_different(status_, std::move(value.value));
                    } else if constexpr (std::is_same_v<Action, SetMessage>) {
                        require_presentable(operation);
                        if (value.value)
                            validate_display_text(*value.value, max_display_text_bytes);
                        return replace_if_different(message_, std::move(value.value));
                    } else if constexpr (std::is_same_v<Action, SetProgress>) {
                        require_presentable(operation);
                        if (value.value && *value.value > 100) {
                            throw StateError(StateErrorCode::invalid_progress, "progress is outside 0..=100");
                        }
                        return replace_if_different(progress_, value.value);
                    } else if constexpr (std::is_same_v<Action, BeginPrompt>) {
                        require_running(operation);
                        if (const auto *active = view_.prompt_metadata()) {
                            if (*active == value.metadata)
                                return TransitionResult::unchanged;
                            throw StateError(StateErrorCode::prompt_conflict, "another prompt is active");
                        }
                        const auto previous = *view_.base_view();
                        view_ = View::prompt(previous, std::move(value.metadata));
                        return TransitionResult::changed;
                    } else if constexpr (std::is_same_v<Action, FinishPrompt>) {
                        return finish_prompt(value.request_id);
                    } else if constexpr (std::is_same_v<Action, Quit>) {
                        if (lifecycle_ == Lifecycle::quitting || lifecycle_ == Lifecycle::stopped) {
                            return TransitionResult::unchanged;
                        }
                        retire_prompt();
                        lifecycle_ = Lifecycle::quitting;
                        return TransitionResult::changed;
                    } else if constexpr (std::is_same_v<Action, MarkStopped>) {
                        if (lifecycle_ == Lifecycle::quitting || lifecycle_ == Lifecycle::failed_open) {
                            lifecycle_ = Lifecycle::stopped;
                            return TransitionResult::changed;
                        }
                        if (lifecycle_ == Lifecycle::stopped)
                            return TransitionResult::unchanged;
                        throw StateError(StateErrorCode::invalid_lifecycle_transition, "cannot stop");
                    } else {
                        if (lifecycle_ == Lifecycle::starting || lifecycle_ == Lifecycle::running ||
                            lifecycle_ == Lifecycle::deactivated) {
                            retire_prompt();
                            lifecycle_ = Lifecycle::failed_open;
                            return TransitionResult::changed;
                        }
                        if (lifecycle_ == Lifecycle::failed_open)
                            return TransitionResult::unchanged;
                        throw StateError(StateErrorCode::invalid_lifecycle_transition, "cannot fail open");
                    }
                    return TransitionResult::unchanged;
                },
                std::move(action));
        } catch (const TextError &error) {
            throw StateError(StateErrorCode::invalid_text, error.what());
        }
    }

} // namespace bootart::splash
