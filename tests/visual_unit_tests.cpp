#include "sart/visual/animation.hpp"
#include "sart/visual/art.hpp"
#include "sart/visual/terminal.hpp"

#include <doctest/doctest.h>

#include <algorithm>
#include <array>
#include <string>

namespace {

    void check_art_error(sart::visual::ArtErrorCode code, std::string_view input) {
        try {
            static_cast<void>(sart::visual::Art::parse(input));
            FAIL("expected ArtError");
        } catch (const sart::visual::ArtError &error) {
            CHECK(error.code() == code);
        }
    }

} // namespace

TEST_SUITE("visual") {

    TEST_CASE("UTF-8 round trips scalar boundaries") {
        constexpr std::array values{char32_t{0},     char32_t{0x7f},   char32_t{0x80},    char32_t{0x7ff},
                                    char32_t{0x800}, char32_t{0xffff}, char32_t{0x10000}, char32_t{0x10ffff}};
        for (const auto value : values) {
            const auto encoded = sart::visual::encode_utf8(value);
            const auto decoded = sart::visual::decode_utf8(encoded);
            REQUIRE(decoded.size() == 1);
            CHECK(decoded.front() == value);
        }
    }

    TEST_CASE("UTF-8 decoder accepts mixed scripts") {
        const auto decoded = sart::visual::decode_utf8("Aé中🙂");
        REQUIRE(decoded.size() == 4);
        CHECK(decoded[0] == U'A');
        CHECK(decoded[1] == U'é');
        CHECK(decoded[2] == U'中');
        CHECK(decoded[3] == U'🙂');
    }

    TEST_CASE("UTF-8 decoder rejects continuation lead") {
        CHECK_THROWS_AS(static_cast<void>(sart::visual::decode_utf8("\x80")), sart::visual::ArtError);
    }

    TEST_CASE("UTF-8 decoder rejects truncated sequence") {
        CHECK_THROWS_AS(static_cast<void>(sart::visual::decode_utf8("\xe2\x82")), sart::visual::ArtError);
    }

    TEST_CASE("UTF-8 decoder rejects overlong sequence") {
        CHECK_THROWS_AS(static_cast<void>(sart::visual::decode_utf8("\xc0\x80")), sart::visual::ArtError);
    }

    TEST_CASE("UTF-8 decoder rejects surrogate sequence") {
        CHECK_THROWS_AS(static_cast<void>(sart::visual::decode_utf8("\xed\xa0\x80")), sart::visual::ArtError);
    }

    TEST_CASE("UTF-8 decoder rejects values above Unicode") {
        CHECK_THROWS_AS(static_cast<void>(sart::visual::decode_utf8("\xf4\x90\x80\x80")), sart::visual::ArtError);
    }

    TEST_CASE("art normalizes CRLF and trims outer rows") {
        const auto art = sart::visual::Art::parse("\r\nAB  \r\nC\r\n\r\n");
        CHECK(art.size() == sart::visual::Size{2, 2});
        CHECK(art.cell(0, 0) == U'A');
        CHECK(art.cell(1, 1) == U' ');
    }

    TEST_CASE("art preserves internal blank rows") {
        const auto art = sart::visual::Art::parse("A\n\nB");
        CHECK(art.size() == sart::visual::Size{1, 3});
        CHECK(art.cell(0, 1) == U' ');
    }

    TEST_CASE("art measures Unicode codepoints") {
        const auto art = sart::visual::Art::parse("é中\n🙂");
        CHECK(art.width() == 2);
        CHECK(art.height() == 2);
        CHECK(art.cell(1, 0) == U'中');
        CHECK(art.cell(0, 1) == U'🙂');
    }

    TEST_CASE("art rejects empty and whitespace-only input") {
        check_art_error(sart::visual::ArtErrorCode::empty, "");
        check_art_error(sart::visual::ArtErrorCode::no_visible_characters, "  \n \n");
    }

    TEST_CASE("art rejects terminal controls") {
        check_art_error(sart::visual::ArtErrorCode::contains_tab, "A\tB");
        check_art_error(sart::visual::ArtErrorCode::contains_nul, std::string_view{"A\0B", 3});
        check_art_error(sart::visual::ArtErrorCode::contains_standalone_carriage_return, "A\rB");
        check_art_error(sart::visual::ArtErrorCode::contains_control, "A\x1b"
                                                                      "B");
    }

    TEST_CASE("art enforces caller width and height limits") {
        CHECK_THROWS_AS(sart::visual::Art::parse_with_limits("ABC", 2, 2), sart::visual::ArtError);
        CHECK_THROWS_AS(sart::visual::Art::parse_with_limits("A\nB\nC", 2, 2), sart::visual::ArtError);
    }

    TEST_CASE("layout centers smaller art") {
        const auto value = sart::visual::layout({4, 2}, {10, 8});
        CHECK(value == sart::visual::Layout{0, 0, 4, 2, 3, 3});
    }

    TEST_CASE("layout crops larger art from center") {
        const auto value = sart::visual::layout({11, 9}, {4, 3});
        CHECK(value == sart::visual::Layout{3, 3, 4, 3, 0, 0});
    }

    TEST_CASE("layout rejects zero terminal extent without underflow") {
        CHECK(sart::visual::layout({10, 10}, {0, 5}) == sart::visual::Layout{});
        CHECK(sart::visual::layout({10, 10}, {5, 0}) == sart::visual::Layout{});
    }

    TEST_CASE("smoothstep clamps and interpolates") {
        CHECK(sart::visual::smoothstep(-1.0F) == doctest::Approx(0.0F));
        CHECK(sart::visual::smoothstep(0.5F) == doctest::Approx(0.5F));
        CHECK(sart::visual::smoothstep(2.0F) == doctest::Approx(1.0F));
    }

    TEST_CASE("cell hash is deterministic and position-sensitive") {
        CHECK(sart::visual::cell_hash(42, 3, 5) == sart::visual::cell_hash(42, 3, 5));
        CHECK(sart::visual::cell_hash(42, 3, 5) != sart::visual::cell_hash(42, 4, 5));
        CHECK(sart::visual::cell_hash(42, 3, 5) != sart::visual::cell_hash(43, 3, 5));
    }

    TEST_CASE("normalized hash stays within unit interval") {
        for (std::size_t index = 0; index < 100; ++index) {
            const auto value = sart::visual::normalized_hash(9, index, index * 3);
            CHECK(value >= 0.0F);
            CHECK(value <= 1.0F);
        }
    }

    TEST_CASE("animation metadata indexes visible cells only") {
        const auto art = sart::visual::Art::parse("A B\n C ");
        const sart::visual::AnimationMetadata metadata(art, 99);
        CHECK(metadata.seed() == 99);
        CHECK(metadata.width() == 3);
        CHECK(metadata.height() == 2);
        CHECK(metadata.cells().size() == 3);
        CHECK(metadata.cell_at(0, 0)->glyph == U'A');
        CHECK(metadata.cell_at(1, 0) == nullptr);
        CHECK(metadata.cell_at(3, 0) == nullptr);
    }

    TEST_CASE("no-color animation always emits light gray glyph") {
        const sart::visual::AnimatedCell cell{2, 4, U'X', 0.2F, 3};
        const auto colored = sart::visual::cell_color_at(cell, 0.0F, 10, 10, true, 7);
        REQUIRE(colored.has_value());
        CHECK(colored->glyph == U'X');
        CHECK(colored->color == sart::visual::AnsiColor::light_gray);
    }

    TEST_CASE("animation completes by removing the glyph") {
        const sart::visual::AnimatedCell cell{0, 0, U'X', 0.0F, 0};
        CHECK_FALSE(sart::visual::cell_color_at(cell, 1.0F, 1, 1, false, 0).has_value());
    }

    TEST_CASE("ANSI palette produces one escape sequence per color") {
        std::string output;
        for (std::size_t index = 0; index < 16; ++index) {
            sart::visual::append_ansi_color(output, static_cast<sart::visual::AnsiColor>(index));
        }
        CHECK(output.starts_with("\x1b[0m"));
        CHECK(output.ends_with("\x1b[97m"));
        CHECK(std::count(output.begin(), output.end(), '\x1b') == 16);
    }

    TEST_CASE("buffer terminal records frames and dimensions") {
        sart::visual::BufferTerminal terminal({17, 9});
        CHECK(terminal.dimensions() == sart::visual::TerminalSize{17, 9});
        terminal.write_frame("one");
        terminal.write_frame("two");
        terminal.flush();
        CHECK(terminal.contents() == "onetwo");
    }

} // TEST_SUITE
