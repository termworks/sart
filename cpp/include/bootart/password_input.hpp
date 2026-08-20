#pragma once

#include "bootart/password_secure.hpp"

#include <array>
#include <cstdint>
#include <optional>

namespace bootart::password {

    enum class EchoMode { obscured, visible, silent };

    struct InputFeedback {
        std::size_t character_count{};
        EchoMode echo_mode{EchoMode::obscured};
        auto operator<=>(const InputFeedback &) const = default;
    };

    enum class InputRejection { invalid_utf8, control_character, maximum_length };
    enum class InputOutcomeKind { pending, changed, submit, cancelled, rejected };

    struct InputOutcome {
        InputOutcomeKind kind{InputOutcomeKind::pending};
        std::optional<InputFeedback> feedback;
        std::optional<InputRejection> rejection;
        auto operator<=>(const InputOutcome &) const = default;
    };

    enum class PromptKeyKind { character, enter, backspace, clear, cancel };
    struct PromptKey {
        PromptKeyKind kind;
        char32_t character{};
    };

    class PromptInput {
      public:
        PromptInput(std::size_t capacity, bool echo, bool silent);
        PromptInput(const PromptInput &) = delete;
        PromptInput &operator=(const PromptInput &) = delete;
        ~PromptInput();

        [[nodiscard]] InputFeedback feedback() const noexcept;
        [[nodiscard]] SecretProtection protection() const noexcept;
        [[nodiscard]] bool empty() const noexcept;
        [[nodiscard]] InputOutcome handle(PromptKey key);
        [[nodiscard]] InputOutcome feed(std::uint8_t byte);
        void clear() noexcept;

        template <typename Function> decltype(auto) with_visible_text(Function &&render) const {
            if (echo_mode_ != EchoMode::visible) {
                return std::forward<Function>(render)(std::optional<std::string_view>{});
            }
            return secret_.expose([&](std::span<const std::byte> bytes) -> decltype(auto) {
                return std::forward<Function>(render)(std::optional<std::string_view>(
                    std::string_view(reinterpret_cast<const char *>(bytes.data()), bytes.size())));
            });
        }

        template <typename Function> decltype(auto) finish_with(Function &&deliver) {
            struct Guard {
                PromptInput &input;
                ~Guard() { input.clear(); }
            } guard{*this};
            return std::forward<Function>(deliver)(secret_);
        }

      private:
        [[nodiscard]] InputOutcome push(char32_t character);
        [[nodiscard]] InputOutcome backspace();
        void reset_pending() noexcept;

        SecureSecret secret_;
        EchoMode echo_mode_;
        std::size_t character_count_{};
        std::array<std::uint8_t, 4> pending_{};
        std::size_t pending_length_{};
        std::size_t pending_expected_{};
    };

} // namespace bootart::password
