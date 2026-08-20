#include "sart/password_input.hpp"

#include "sart/art.hpp"

#include <algorithm>
#include <stdexcept>

namespace sart::password {
    namespace {

        bool unsafe_for_terminal(char32_t character) noexcept {
            return character < 0x20 || (character >= 0x7f && character <= 0x9f) || character == 0x061c ||
                   character == 0x200e || character == 0x200f || (character >= 0x202a && character <= 0x202e) ||
                   (character >= 0x2066 && character <= 0x2069);
        }

        InputOutcome changed(InputFeedback feedback) { return {InputOutcomeKind::changed, feedback, std::nullopt}; }

        InputOutcome rejected(InputRejection rejection) {
            return {InputOutcomeKind::rejected, std::nullopt, rejection};
        }

    } // namespace

    PromptInput::PromptInput(std::size_t capacity, bool echo, bool silent)
        : secret_(capacity), echo_mode_(silent ? EchoMode::silent
                                        : echo ? EchoMode::visible
                                               : EchoMode::obscured) {}

    PromptInput::~PromptInput() { clear(); }

    InputFeedback PromptInput::feedback() const noexcept {
        return {echo_mode_ == EchoMode::silent ? 0 : character_count_, echo_mode_};
    }

    SecretProtection PromptInput::protection() const noexcept { return secret_.protection(); }
    bool PromptInput::empty() const noexcept { return secret_.empty(); }

    InputOutcome PromptInput::handle(PromptKey key) {
        reset_pending();
        switch (key.kind) {
        case PromptKeyKind::character:
            return push(key.character);
        case PromptKeyKind::enter:
            return {InputOutcomeKind::submit, {}, {}};
        case PromptKeyKind::backspace:
            return backspace();
        case PromptKeyKind::clear:
            secret_.clear();
            character_count_ = 0;
            return changed(feedback());
        case PromptKeyKind::cancel:
            clear();
            return {InputOutcomeKind::cancelled, {}, {}};
        }
        return {};
    }

    InputOutcome PromptInput::feed(std::uint8_t byte) {
        if (pending_length_ == 0) {
            if (byte == 0 || byte == '\r' || byte == '\n')
                return {InputOutcomeKind::submit, {}, {}};
            if (byte == 8 || byte == 127)
                return backspace();
            if (byte == 21)
                return handle({PromptKeyKind::clear});
            if (byte == 3 || byte == 4 || byte == 27)
                return handle({PromptKeyKind::cancel});
            if (byte >= 0x20 && byte <= 0x7e)
                return push(byte);
            if (byte >= 0xc2 && byte <= 0xdf)
                pending_expected_ = 2;
            else if (byte >= 0xe0 && byte <= 0xef)
                pending_expected_ = 3;
            else if (byte >= 0xf0 && byte <= 0xf4)
                pending_expected_ = 4;
            else if (byte >= 0x80 || byte == 0xc0 || byte == 0xc1) {
                return rejected(InputRejection::invalid_utf8);
            } else {
                return rejected(InputRejection::control_character);
            }
            pending_[0] = byte;
            pending_length_ = 1;
            return {};
        }
        if (byte < 0x80 || byte > 0xbf || pending_length_ >= pending_expected_) {
            reset_pending();
            return rejected(InputRejection::invalid_utf8);
        }
        pending_[pending_length_++] = byte;
        if (pending_length_ != pending_expected_)
            return {};
        try {
            const auto text = std::string_view(reinterpret_cast<const char *>(pending_.data()), pending_length_);
            const auto characters = decode_utf8(text);
            reset_pending();
            if (characters.size() != 1)
                return rejected(InputRejection::invalid_utf8);
            return push(characters.front());
        } catch (...) {
            reset_pending();
            return rejected(InputRejection::invalid_utf8);
        }
    }

    void PromptInput::clear() noexcept {
        secret_.clear();
        character_count_ = 0;
        reset_pending();
    }

    InputOutcome PromptInput::push(char32_t character) {
        if (unsafe_for_terminal(character))
            return rejected(InputRejection::control_character);
        try {
            secret_.push(character);
            ++character_count_;
            return changed(feedback());
        } catch (const std::length_error &) {
            return rejected(InputRejection::maximum_length);
        } catch (...) {
            return rejected(InputRejection::invalid_utf8);
        }
    }

    InputOutcome PromptInput::backspace() {
        try {
            if (secret_.pop())
                character_count_ = character_count_ == 0 ? 0 : character_count_ - 1;
            return changed(feedback());
        } catch (...) {
            clear();
            return rejected(InputRejection::invalid_utf8);
        }
    }

    void PromptInput::reset_pending() noexcept {
        std::fill(pending_.begin(), pending_.end(), 0);
        pending_length_ = 0;
        pending_expected_ = 0;
    }

} // namespace sart::password
