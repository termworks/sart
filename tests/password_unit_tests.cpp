#include "sart/password/input.hpp"
#include "sart/password/secure.hpp"
#include "sart/visual/art.hpp"

#include <doctest/doctest.h>

#include <cstddef>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

namespace {

    std::string exposed_text(const sart::password::SecureSecret &secret) {
        return secret.expose([](std::span<const std::byte> bytes) {
            return std::string(reinterpret_cast<const char *>(bytes.data()), bytes.size());
        });
    }

} // namespace

TEST_SUITE("password") {

    TEST_CASE("secure secret enforces capacity range") {
        CHECK_THROWS_AS(sart::password::SecureSecret(0), std::invalid_argument);
        CHECK_THROWS_AS(sart::password::SecureSecret(sart::password::maximum_secret_bytes + 1), std::invalid_argument);
        CHECK_NOTHROW(sart::password::SecureSecret(1));
        CHECK_NOTHROW(sart::password::SecureSecret(sart::password::maximum_secret_bytes));
    }

    TEST_CASE("secure secret stores ASCII bytes") {
        sart::password::SecureSecret secret(16);
        CHECK(secret.empty());
        secret.push("sart");
        CHECK(secret.size() == 4);
        CHECK(secret.capacity() == 16);
        CHECK(exposed_text(secret) == "sart");
    }

    TEST_CASE("secure secret stores and pops Unicode scalars") {
        sart::password::SecureSecret secret(16);
        secret.push(U'é');
        secret.push(U'🙂');
        CHECK(secret.size() == 6);
        CHECK(secret.pop() == std::optional<char32_t>{U'🙂'});
        CHECK(secret.pop() == std::optional<char32_t>{U'é'});
        CHECK_FALSE(secret.pop().has_value());
    }

    TEST_CASE("secure secret rejects invalid UTF-8") {
        sart::password::SecureSecret secret(16);
        CHECK_THROWS_AS(secret.push("\xc0\x80"), sart::visual::ArtError);
        CHECK(secret.empty());
    }

    TEST_CASE("secure secret overflow keeps prior contents") {
        sart::password::SecureSecret secret(4);
        secret.push("abc");
        CHECK_THROWS_AS(secret.push("de"), std::length_error);
        CHECK(exposed_text(secret) == "abc");
    }

    TEST_CASE("secure secret clear resets logical contents") {
        sart::password::SecureSecret secret(16);
        secret.push("erase me");
        secret.clear();
        CHECK(secret.empty());
        CHECK(exposed_text(secret).empty());
    }

    TEST_CASE("secure secret move transfers contents") {
        sart::password::SecureSecret source(16);
        source.push("move");
        sart::password::SecureSecret destination(std::move(source));
        CHECK(destination.capacity() == 16);
        CHECK(exposed_text(destination) == "move");
        CHECK(source.capacity() == 0);
        CHECK(source.empty());
    }

    TEST_CASE("prompt input chooses obscured mode by default") {
        sart::password::PromptInput input(16, false, false);
        CHECK(input.feedback() == sart::password::InputFeedback{0, sart::password::EchoMode::obscured});
        const auto outcome = input.handle({sart::password::PromptKeyKind::character, U'x'});
        CHECK(outcome.kind == sart::password::InputOutcomeKind::changed);
        CHECK(input.feedback().character_count == 1);
    }

    TEST_CASE("prompt input visible mode exposes text") {
        sart::password::PromptInput input(16, true, false);
        static_cast<void>(input.feed('a'));
        static_cast<void>(input.feed('b'));
        const auto text = input.with_visible_text(
            [](std::optional<std::string_view> value) { return value ? std::string(*value) : std::string{}; });
        CHECK(text == "ab");
    }

    TEST_CASE("prompt input obscured mode hides text") {
        sart::password::PromptInput input(16, false, false);
        static_cast<void>(input.feed('a'));
        CHECK_FALSE(input.with_visible_text([](std::optional<std::string_view> value) { return value.has_value(); }));
    }

    TEST_CASE("prompt input silent mode suppresses count") {
        sart::password::PromptInput input(16, true, true);
        static_cast<void>(input.feed('a'));
        CHECK(input.feedback() == sart::password::InputFeedback{0, sart::password::EchoMode::silent});
    }

    TEST_CASE("prompt byte stream accepts complete UTF-8 scalar") {
        sart::password::PromptInput input(16, false, false);
        CHECK(input.feed(0xf0).kind == sart::password::InputOutcomeKind::pending);
        CHECK(input.feed(0x9f).kind == sart::password::InputOutcomeKind::pending);
        CHECK(input.feed(0x99).kind == sart::password::InputOutcomeKind::pending);
        CHECK(input.feed(0x82).kind == sart::password::InputOutcomeKind::changed);
        CHECK(input.feedback().character_count == 1);
    }

    TEST_CASE("prompt byte stream rejects invalid UTF-8 lead") {
        sart::password::PromptInput input(16, false, false);
        const auto outcome = input.feed(0x80);
        CHECK(outcome.kind == sart::password::InputOutcomeKind::rejected);
        CHECK(outcome.rejection == std::optional{sart::password::InputRejection::invalid_utf8});
    }

    TEST_CASE("prompt byte stream resets after invalid continuation") {
        sart::password::PromptInput input(16, false, false);
        CHECK(input.feed(0xe2).kind == sart::password::InputOutcomeKind::pending);
        CHECK(input.feed('x').rejection == std::optional{sart::password::InputRejection::invalid_utf8});
        CHECK(input.feed('y').kind == sart::password::InputOutcomeKind::changed);
        CHECK(input.feedback().character_count == 1);
    }

    TEST_CASE("prompt byte stream maps submit controls") {
        for (const auto byte : {std::uint8_t{0}, std::uint8_t{'\r'}, std::uint8_t{'\n'}}) {
            sart::password::PromptInput input(16, false, false);
            CHECK(input.feed(byte).kind == sart::password::InputOutcomeKind::submit);
        }
    }

    TEST_CASE("prompt byte stream maps cancellation controls") {
        for (const auto byte : {std::uint8_t{3}, std::uint8_t{4}, std::uint8_t{27}}) {
            sart::password::PromptInput input(16, false, false);
            static_cast<void>(input.feed('x'));
            CHECK(input.feed(byte).kind == sart::password::InputOutcomeKind::cancelled);
            CHECK(input.empty());
        }
    }

    TEST_CASE("prompt backspace removes one Unicode scalar") {
        sart::password::PromptInput input(16, false, false);
        for (const auto byte : std::string("é")) {
            static_cast<void>(input.feed(static_cast<std::uint8_t>(byte)));
        }
        static_cast<void>(input.feed('x'));
        CHECK(input.feedback().character_count == 2);
        CHECK(input.feed(127).kind == sart::password::InputOutcomeKind::changed);
        CHECK(input.feedback().character_count == 1);
    }

    TEST_CASE("prompt clear control removes all input") {
        sart::password::PromptInput input(16, false, false);
        static_cast<void>(input.feed('a'));
        static_cast<void>(input.feed('b'));
        CHECK(input.feed(21).kind == sart::password::InputOutcomeKind::changed);
        CHECK(input.empty());
        CHECK(input.feedback().character_count == 0);
    }

    TEST_CASE("prompt rejects characters beyond byte capacity") {
        sart::password::PromptInput input(2, false, false);
        static_cast<void>(input.feed('a'));
        static_cast<void>(input.feed('b'));
        const auto outcome = input.feed('c');
        CHECK(outcome.kind == sart::password::InputOutcomeKind::rejected);
        CHECK(outcome.rejection == std::optional{sart::password::InputRejection::maximum_length});
    }

    TEST_CASE("prompt finish delivers then clears secret") {
        sart::password::PromptInput input(16, false, false);
        static_cast<void>(input.feed('o'));
        static_cast<void>(input.feed('k'));
        const auto delivered =
            input.finish_with([](const sart::password::SecureSecret &secret) { return exposed_text(secret); });
        CHECK(delivered == "ok");
        CHECK(input.empty());
    }

} // TEST_SUITE
