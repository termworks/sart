#include "sart/visual/frame_engine.hpp"
#include "sart/visual/renderer.hpp"

#include <doctest/doctest.h>

#include <cstdint>
#include <limits>
#include <string>

namespace {

    void check_scene_error(sart::display::SceneErrorCode code, const auto &action) {
        try {
            action();
            FAIL("expected SceneError");
        } catch (const sart::display::SceneError &error) {
            CHECK(error.code() == code);
        }
    }

} // namespace

TEST_SUITE("renderer") {

    TEST_CASE("art selector keeps the primary art when it fits") {
        const auto primary = sart::visual::Art::parse("AB\nCD");
        const auto small = sart::visual::Art::parse("x");
        CHECK(&sart::visual::select_art(primary, &small, {2, 2}) == &primary);
    }

    TEST_CASE("art selector uses the fallback when only it fits") {
        const auto primary = sart::visual::Art::parse("ABCDE\nFGHIJ");
        const auto small = sart::visual::Art::parse("xy");
        CHECK(&sart::visual::select_art(primary, &small, {2, 1}) == &small);
    }

    TEST_CASE("art selector keeps primary when neither art fits") {
        const auto primary = sart::visual::Art::parse("ABCDE");
        const auto small = sart::visual::Art::parse("xyz");
        CHECK(&sart::visual::select_art(primary, &small, {2, 1}) == &primary);
        CHECK(&sart::visual::select_art(primary, nullptr, {2, 1}) == &primary);
    }

    TEST_CASE("first frame hides the cursor and clears the terminal") {
        const auto art = sart::visual::Art::parse("X");
        const sart::visual::AnimationMetadata metadata(art, 7);
        const auto bytes = sart::visual::generate_frame_bytes(art, metadata, sart::visual::layout({1, 1}, {3, 3}),
                                                              {0.5F, true, true, true, 0});
        CHECK(bytes.starts_with("\x1b[?25l\x1b[0m\x1b[2J"));
        CHECK(bytes.contains("\x1b[2;2H"));
        CHECK(bytes.contains('X'));
    }

    TEST_CASE("first frame can preserve existing terminal contents") {
        const auto art = sart::visual::Art::parse("X");
        const sart::visual::AnimationMetadata metadata(art, 7);
        const auto bytes = sart::visual::generate_frame_bytes(art, metadata, sart::visual::layout({1, 1}, {1, 1}),
                                                              {0.5F, true, true, false, 0});
        CHECK(bytes.starts_with("\x1b[?25l\x1b[0m"));
        CHECK_FALSE(bytes.contains("\x1b[2J"));
    }

    TEST_CASE("later frames omit cursor and clear setup") {
        const auto art = sart::visual::Art::parse("X");
        const sart::visual::AnimationMetadata metadata(art, 7);
        const auto bytes = sart::visual::generate_frame_bytes(art, metadata, sart::visual::layout({1, 1}, {1, 1}),
                                                              {0.5F, true, false, true, 0});
        CHECK_FALSE(bytes.contains("\x1b[?25l"));
        CHECK_FALSE(bytes.contains("\x1b[2J"));
        CHECK(bytes.contains('X'));
    }

    TEST_CASE("exit bytes restore the cursor below rendered art") {
        const sart::visual::Layout placement{0, 0, 4, 2, 3, 5};
        const auto bytes = sart::visual::build_exit_bytes(placement, {10, 10});
        CHECK(bytes == "\x1b[0m\x1b[?25h\x1b[8;1H");
    }

    TEST_CASE("exit bytes clamp the final cursor row") {
        const sart::visual::Layout placement{0, 0, 4, 3, 0, 8};
        CHECK(sart::visual::build_exit_bytes(placement, {10, 10}).ends_with("\x1b[10;1H"));
        CHECK(sart::visual::build_exit_bytes(placement, {0, 0}).ends_with("\x1b[1;1H"));
    }

    TEST_CASE("frame engine reports source art size") {
        const auto art = sart::visual::Art::parse("ABC\nDEF");
        const sart::visual::FrameEngine engine(art, 42);
        CHECK(engine.art_size() == sart::visual::Size{3, 2});
    }

    TEST_CASE("frame engine centers no-color glyphs") {
        const auto art = sart::visual::Art::parse("X");
        const sart::visual::FrameEngine engine(art, 42);
        const auto scene = engine.render({3, 3}, 0.5F, true, 0);
        CHECK(scene.dimensions() == sart::display::Dimensions(3, 3));
        REQUIRE(scene.get(1, 1).has_value());
        CHECK(scene.get(1, 1)->glyph() == U'X');
        CHECK(scene.get(1, 1)->style().foreground == sart::display::Color::white);
    }

    TEST_CASE("frame engine crops oversized art") {
        const auto art = sart::visual::Art::parse("ABCDE");
        const sart::visual::FrameEngine engine(art, 42);
        const auto scene = engine.render({3, 1}, 0.5F, true, 0);
        REQUIRE(scene.get(0, 0).has_value());
        REQUIRE(scene.get(2, 0).has_value());
        CHECK(scene.get(0, 0)->glyph() == U'B');
        CHECK(scene.get(2, 0)->glyph() == U'D');
    }

    TEST_CASE("frame engine rejects dimensions beyond scene storage") {
        const auto art = sart::visual::Art::parse("X");
        const sart::visual::FrameEngine engine(art, 42);
        check_scene_error(sart::display::SceneErrorCode::too_large, [&] {
            static_cast<void>(engine.render(
                {static_cast<std::size_t>(std::numeric_limits<std::uint16_t>::max()) + 1, 1}, 0.5F, true, 0));
        });
    }

    TEST_CASE("final renderer emits a complete terminal transaction") {
        const auto art = sart::visual::Art::parse("X");
        sart::visual::BufferTerminal terminal({1, 1});
        sart::visual::render_final(terminal, art, nullptr, true);
        CHECK(terminal.contents().starts_with("\x1b[?25l\x1b[0m\x1b[2J"));
        CHECK(terminal.contents().contains('X'));
        CHECK(terminal.contents().ends_with("\x1b[0m\x1b[?25h\x1b[1;1H"));
    }

} // TEST_SUITE
