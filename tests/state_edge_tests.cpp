#include "sart/splash/state.hpp"

#include <doctest/doctest.h>

#include <optional>
#include <string>

namespace {

    void check_state_error(sart::splash::StateErrorCode code, const auto &action) {
        try {
            action();
            FAIL("expected StateError");
        } catch (const sart::splash::StateError &error) {
            CHECK(error.code() == code);
        }
    }

    sart::splash::SplashState running_state() {
        sart::splash::SplashState state;
        static_cast<void>(state.apply(sart::splash::MarkRunning{}));
        return state;
    }

} // namespace

TEST_SUITE("splash state edges") {

    TEST_CASE("constructor preserves requested mode") {
        const sart::splash::SplashState state(sart::splash::Mode::shutdown);
        CHECK(state.mode() == sart::splash::Mode::shutdown);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::starting);
    }

    TEST_CASE("starting lifecycle accepts presentation metadata") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::SetMode{sart::splash::Mode::update}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetStatus{std::string("Preparing")}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetMessage{std::string("Starting services")}) ==
              sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetProgress{42}) == sart::splash::TransitionResult::changed);
        CHECK(state.mode() == sart::splash::Mode::update);
        CHECK(state.status() == std::optional<std::string>{"Preparing"});
        CHECK(state.message() == std::optional<std::string>{"Starting services"});
        CHECK(state.progress() == std::optional<std::uint8_t>{42});
    }

    TEST_CASE("presentation metadata supports idempotence and clearing") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::SetStatus{std::string("Ready")}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetStatus{std::string("Ready")}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::SetStatus{std::nullopt}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::SetMessage{std::nullopt}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::SetProgress{std::nullopt}) == sart::splash::TransitionResult::unchanged);
        CHECK_FALSE(state.status().has_value());
    }

    TEST_CASE("deactivate and reactivate are idempotent") {
        auto state = running_state();
        CHECK(state.apply(sart::splash::Deactivate{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::deactivated);
        CHECK(state.apply(sart::splash::Deactivate{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::Reactivate{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::running);
        CHECK(state.apply(sart::splash::Reactivate{}) == sart::splash::TransitionResult::unchanged);
    }

    TEST_CASE("active prompt ignores ordinary view changes") {
        auto state = running_state();
        static_cast<void>(state.apply(sart::splash::ShowDetails{}));
        const auto prompt = sart::splash::PromptMetadata(5, "Unlock root");
        CHECK(state.apply(sart::splash::BeginPrompt{prompt}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::Show{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::Hide{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::ToggleDetails{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.view().prompt_metadata() != nullptr);
    }

    TEST_CASE("active prompt blocks lifecycle and root transitions") {
        auto state = running_state();
        static_cast<void>(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(6, "Unlock root")}));
        check_state_error(sart::splash::StateErrorCode::prompt_active,
                          [&] { static_cast<void>(state.apply(sart::splash::Deactivate{})); });
        check_state_error(sart::splash::StateErrorCode::prompt_active, [&] {
            static_cast<void>(state.apply(sart::splash::SetRootStage{sart::splash::RootStage::switching}));
        });
    }

    TEST_CASE("prompt completion validates request identity") {
        auto state = running_state();
        static_cast<void>(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(7, "Unlock root")}));
        check_state_error(sart::splash::StateErrorCode::prompt_id_mismatch, [&] {
            static_cast<void>(state.apply(sart::splash::FinishPrompt{8, sart::splash::PromptOutcome::answered}));
        });
        CHECK(state.view().prompt_metadata() != nullptr);
    }

    TEST_CASE("prompt completion rejects unrelated retired IDs") {
        auto state = running_state();
        check_state_error(sart::splash::StateErrorCode::no_active_prompt, [&] {
            static_cast<void>(state.apply(sart::splash::FinishPrompt{9, sart::splash::PromptOutcome::cancelled}));
        });
    }

    TEST_CASE("equivalent active prompt is idempotent") {
        auto state = running_state();
        const auto prompt = sart::splash::PromptMetadata(10, "Unlock root").with_source("cryptroot");
        CHECK(state.apply(sart::splash::BeginPrompt{prompt}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::BeginPrompt{prompt}) == sart::splash::TransitionResult::unchanged);
    }

    TEST_CASE("fail open retires prompts and can stop") {
        auto state = running_state();
        static_cast<void>(state.apply(sart::splash::BeginPrompt{sart::splash::PromptMetadata(11, "Unlock root")}));
        CHECK(state.apply(sart::splash::FailOpen{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::failed_open);
        CHECK(state.view().prompt_metadata() == nullptr);
        CHECK(state.apply(sart::splash::FailOpen{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::MarkStopped{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::stopped);
    }

    TEST_CASE("unsafe presentation text maps to a state error") {
        sart::splash::SplashState state;
        check_state_error(sart::splash::StateErrorCode::invalid_text, [&] {
            static_cast<void>(state.apply(sart::splash::SetMessage{std::string("unsafe\ntext")}));
        });
    }

    TEST_CASE("quit can stop directly from starting") {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::Quit{}) == sart::splash::TransitionResult::changed);
        CHECK(state.lifecycle() == sart::splash::Lifecycle::quitting);
        CHECK(state.apply(sart::splash::Quit{}) == sart::splash::TransitionResult::unchanged);
        CHECK(state.apply(sart::splash::MarkStopped{}) == sart::splash::TransitionResult::changed);
        CHECK(state.apply(sart::splash::MarkStopped{}) == sart::splash::TransitionResult::unchanged);
    }

    TEST_CASE("running lifecycle cannot stop without quitting") {
        auto state = running_state();
        check_state_error(sart::splash::StateErrorCode::invalid_lifecycle_transition,
                          [&] { static_cast<void>(state.apply(sart::splash::MarkStopped{})); });
    }

} // TEST_SUITE
