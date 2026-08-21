#include "sart/splash/command.hpp"
#include "sart/splash/protocol.hpp"
#include "sart/splash/state.hpp"

#include <doctest/doctest.h>

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace {

    template <typename Function> void check_text_error(sart::splash::TextErrorCode code, Function &&function) {
        try {
            function();
            FAIL("expected TextError");
        } catch (const sart::splash::TextError &error) {
            CHECK(error.code() == code);
        }
    }

    template <typename Function> void check_state_error(sart::splash::StateErrorCode code, Function &&function) {
        try {
            function();
            FAIL("expected StateError");
        } catch (const sart::splash::StateError &error) {
            CHECK(error.code() == code);
        }
    }

    template <typename Function> void check_protocol_error(sart::splash::ProtocolErrorCode code, Function &&function) {
        try {
            function();
            FAIL("expected ProtocolError");
        } catch (const sart::splash::ProtocolError &error) {
            CHECK(error.code() == code);
        }
    }

} // namespace

TEST_SUITE("splash") {

    TEST_CASE("display text accepts empty and Unicode text") {
        CHECK_NOTHROW(sart::splash::validate_display_text("", 0));
        CHECK_NOTHROW(sart::splash::validate_display_text("Starting 中🙂", 64));
    }

    TEST_CASE("display text reports byte limits") {
        check_text_error(sart::splash::TextErrorCode::too_long, [] { sart::splash::validate_display_text("abcd", 3); });
    }

    TEST_CASE("display text reports invalid UTF-8") {
        check_text_error(sart::splash::TextErrorCode::invalid_utf8,
                         [] { sart::splash::validate_display_text("\xc0\x80", 8); });
    }

    TEST_CASE("display text reports unsafe codepoint and byte offset") {
        try {
            sart::splash::validate_display_text("é\n", 8);
            FAIL("expected TextError");
        } catch (const sart::splash::TextError &error) {
            CHECK(error.code() == sart::splash::TextErrorCode::unsafe_character);
            CHECK(error.byte_index() == 2);
            CHECK(error.codepoint() == U'\n');
        }
    }

    TEST_CASE("prompt metadata preserves builder fields") {
        auto metadata = sart::splash::PromptMetadata(44, "Password")
                            .with_source("cryptroot")
                            .with_requester_pid(123)
                            .with_echo(true)
                            .with_silent(true)
                            .with_expiry(9'000);
        CHECK(metadata.request_id() == 44);
        CHECK(metadata.text() == "Password");
        CHECK(metadata.source() == std::optional<std::string>{"cryptroot"});
        CHECK(metadata.requester_pid() == std::optional<std::uint32_t>{123});
        CHECK(metadata.echo());
        CHECK(metadata.silent());
        CHECK(metadata.expires_at_milliseconds() == std::optional<std::uint64_t>{9'000});
    }

    TEST_CASE("prompt metadata rejects empty fields") {
        check_text_error(sart::splash::TextErrorCode::empty,
                         [] { static_cast<void>(sart::splash::PromptMetadata(1, "")); });
        check_text_error(sart::splash::TextErrorCode::empty, [] {
            auto metadata = sart::splash::PromptMetadata(1, "Password");
            static_cast<void>(metadata.with_source(""));
        });
    }

    TEST_CASE("base view has no prompt history") {
        const sart::splash::View view(sart::splash::BaseView::details);
        CHECK(view.base_view() == std::optional{sart::splash::BaseView::details});
        CHECK(view.prompt_metadata() == nullptr);
        CHECK_THROWS_AS(static_cast<void>(view.previous_view()), std::logic_error);
    }

    TEST_CASE("prompt view preserves previous view") {
        const auto view =
            sart::splash::View::prompt(sart::splash::BaseView::hidden, sart::splash::PromptMetadata(7, "Unlock"));
        CHECK_FALSE(view.base_view().has_value());
        REQUIRE(view.prompt_metadata() != nullptr);
        CHECK(view.prompt_metadata()->request_id() == 7);
        CHECK(view.previous_view() == sart::splash::BaseView::hidden);
    }

    TEST_CASE("splash state starts with boot defaults") {
        const sart::splash::SplashState state;
        CHECK(state.lifecycle() == sart::splash::Lifecycle::starting);
        CHECK(state.view().base_view() == std::optional{sart::splash::BaseView::splash});
        CHECK(state.mode() == sart::splash::Mode::boot);
        CHECK(state.root_stage() == sart::splash::RootStage::initramfs);
        CHECK_FALSE(state.progress().has_value());
    }

    TEST_CASE("mark running is idempotent") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::MarkRunning{}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::MarkRunning{}) == sart::splash::TransitionResult::unchanged);
    }

    TEST_CASE("view changes require running lifecycle") {
        sart::splash::SplashState state;
        check_state_error(sart::splash::StateErrorCode::invalid_lifecycle_transition,
                          [&] { static_cast<void>(state.apply(sart::splash::Hide{})); });
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        CHECK(state.apply(sart::splash::Hide{}) == sart::splash::TransitionResult::changed);
        CHECK(state.view().base_view() == std::optional{sart::splash::BaseView::hidden});
    }

    TEST_CASE("details toggle alternates views") {
        sart::splash::SplashState state;
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        static_cast<void>(state.apply(sart::splash::ToggleDetails{}));
        CHECK(state.view().base_view() == std::optional{sart::splash::BaseView::details});
        static_cast<void>(state.apply(sart::splash::ToggleDetails{}));
        CHECK(state.view().base_view() == std::optional{sart::splash::BaseView::splash});
    }

    TEST_CASE("root stage accepts only forward transitions") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::SetRootStage{sart::splash::RootStage::switching}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetRootStage{sart::splash::RootStage::real_root}) ==
              sart::splash::TransitionResult::changed);
        check_state_error(sart::splash::StateErrorCode::invalid_root_transition, [&] {
            static_cast<void>(state.apply(sart::splash::SetRootStage{sart::splash::RootStage::initramfs}));
        });
    }

    TEST_CASE("progress accepts endpoints and rejects overflow") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::SetProgress{0}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetProgress{100}) == sart::splash::TransitionResult::changed);
        check_state_error(sart::splash::StateErrorCode::invalid_progress,
                          [&] { static_cast<void>(state.apply(sart::splash::SetProgress{101})); });
    }

    TEST_CASE("prompt finish restores view and is idempotent") {
        sart::splash::SplashState state;
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        static_cast<void>(state.apply(sart::splash::ShowDetails{}));
        CHECK(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(12, "Password")}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.view().prompt_metadata()->request_id() == 12);
        CHECK(state.apply(sart::splash::FinishPrompt{12, sart::splash::PromptOutcome::answered}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.view().base_view() == std::optional{sart::splash::BaseView::details});
        CHECK(state.apply(sart::splash::FinishPrompt{12, sart::splash::PromptOutcome::answered}) ==
              sart::splash::TransitionResult::unchanged);
    }

    TEST_CASE("prompt refuses conflicting request") {
        sart::splash::SplashState state;
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        static_cast<void>(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(1, "One")}));
        check_state_error(sart::splash::StateErrorCode::prompt_conflict, [&] {
            static_cast<void>(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(2, "Two")}));
        });
    }

    TEST_CASE("quit and stop form terminal lifecycle") {
        sart::splash::SplashState state;
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        CHECK(state.apply(sart::splash::Quit{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::quitting);
        CHECK(state.apply(sart::splash::MarkStopped{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::stopped);
    }

    TEST_CASE("protocol empty frame round trips") {
        const auto frame = sart::splash::Frame::empty(sart::splash::Opcode::ping, 0x0102030405060708ULL);
        const auto encoded = frame.encode();
        CHECK(encoded.size() == sart::splash::protocol_header_length);
        CHECK(sart::splash::Frame::decode_exact(encoded) == frame);
    }

    TEST_CASE("protocol text frame round trips Unicode") {
        const auto frame = sart::splash::Frame::text(sart::splash::Opcode::message, 9, "Booting 中");
        const auto decoded = sart::splash::Frame::decode_exact(frame.encode());
        CHECK(decoded.opcode() == sart::splash::Opcode::message);
        CHECK(decoded.payload_text() == "Booting 中");
    }

    TEST_CASE("protocol progress and mode expose typed values") {
        const auto progress = sart::splash::Frame::progress(1, 75);
        CHECK(progress.progress_value() == std::optional<std::uint8_t>{75});
        CHECK_FALSE(progress.mode_value().has_value());
        const auto mode = sart::splash::Frame::mode(2, sart::splash::Mode::upgrade);
        CHECK(mode.mode_value() == std::optional{sart::splash::Mode::upgrade});
        CHECK_FALSE(mode.progress_value().has_value());
    }

    TEST_CASE("protocol quit owns the only allowed flag") {
        CHECK_FALSE(sart::splash::Frame::quit(1, false).retains_splash());
        CHECK(sart::splash::Frame::quit(1, true).retains_splash());
        check_protocol_error(sart::splash::ProtocolErrorCode::flags_not_allowed, [] {
            static_cast<void>(sart::splash::Frame(sart::splash::Opcode::ping, sart::splash::retain_splash_flag, 1, {}));
        });
    }

    TEST_CASE("protocol rejects malformed lengths") {
        auto encoded = sart::splash::Frame::text(sart::splash::Opcode::message, 3, "x").encode();
        encoded.pop_back();
        check_protocol_error(sart::splash::ProtocolErrorCode::truncated,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
        encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 3).encode();
        encoded.push_back(0);
        check_protocol_error(sart::splash::ProtocolErrorCode::trailing_bytes,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
    }

    TEST_CASE("protocol rejects unknown opcode") {
        auto encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 3).encode();
        encoded[6] = 0x7f;
        encoded[7] = 0xff;
        check_protocol_error(sart::splash::ProtocolErrorCode::unknown_opcode,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
    }

    TEST_CASE("command metadata classifies mutation") {
        CHECK_FALSE(sart::splash::is_mutating(sart::splash::Opcode::ping));
        CHECK_FALSE(sart::splash::is_mutating(sart::splash::Opcode::state));
        CHECK(sart::splash::is_mutating(sart::splash::Opcode::show));
        CHECK(sart::splash::is_mutating(sart::splash::Opcode::progress));
    }

    TEST_CASE("mode names cover every presentation mode") {
        CHECK(sart::splash::mode_name(sart::splash::Mode::boot) == "boot");
        CHECK(sart::splash::mode_name(sart::splash::Mode::shutdown) == "shutdown");
        CHECK(sart::splash::mode_name(sart::splash::Mode::reboot) == "reboot");
        CHECK(sart::splash::mode_name(sart::splash::Mode::update) == "update");
        CHECK(sart::splash::mode_name(sart::splash::Mode::upgrade) == "upgrade");
    }

} // TEST_SUITE
