#include "sart/display/backend.hpp"
#include "sart/display/buffer.hpp"

#include <doctest/doctest.h>

#include <array>
#include <chrono>
#include <cstdint>
#include <string_view>
#include <vector>

namespace {

    template <typename Function> void check_scene_error(sart::display::SceneErrorCode code, Function &&function) {
        try {
            function();
            FAIL("expected SceneError");
        } catch (const sart::display::SceneError &error) {
            CHECK(error.code() == code);
        }
    }

    template <typename Function> void check_display_error(sart::display::DisplayErrorCode code, Function &&function) {
        try {
            function();
            FAIL("expected DisplayError");
        } catch (const sart::display::DisplayError &error) {
            CHECK(error.code() == code);
        }
    }

} // namespace

TEST_SUITE("display") {

    TEST_CASE("dimensions expose cell count") {
        const sart::display::Dimensions dimensions(80, 25);
        CHECK(dimensions.columns() == 80);
        CHECK(dimensions.rows() == 25);
        CHECK(dimensions.cell_count() == 2'000);
    }

    TEST_CASE("dimensions reject empty axes") {
        check_scene_error(sart::display::SceneErrorCode::empty_dimensions,
                          [] { static_cast<void>(sart::display::Dimensions(0, 1)); });
        check_scene_error(sart::display::SceneErrorCode::empty_dimensions,
                          [] { static_cast<void>(sart::display::Dimensions(1, 0)); });
    }

    TEST_CASE("dimensions reject excessive cell count") {
        check_scene_error(sart::display::SceneErrorCode::too_large,
                          [] { static_cast<void>(sart::display::Dimensions(65'535, 65'535)); });
    }

    TEST_CASE("cell preserves style") {
        const sart::display::Style style{sart::display::Color::bright_green, sart::display::Color::blue, true};
        const sart::display::Cell cell(U'λ', style);
        CHECK(cell.glyph() == U'λ');
        CHECK(cell.style() == style);
    }

    TEST_CASE("cell rejects C0 and C1 controls") {
        check_scene_error(sart::display::SceneErrorCode::control_glyph,
                          [] { static_cast<void>(sart::display::Cell(U'\n')); });
        check_scene_error(sart::display::SceneErrorCode::control_glyph,
                          [] { static_cast<void>(sart::display::Cell(char32_t{0x85})); });
    }

    TEST_CASE("scene initializes every cell to a space") {
        const sart::display::Scene scene(sart::display::Dimensions(3, 2));
        REQUIRE(scene.cells().size() == 6);
        for (const auto &cell : scene.cells()) {
            CHECK(cell.glyph() == U' ');
        }
    }

    TEST_CASE("scene rejects wrong cell vector length") {
        check_scene_error(sart::display::SceneErrorCode::wrong_cell_count, [] {
            static_cast<void>(
                sart::display::Scene(sart::display::Dimensions(2, 2), std::vector<sart::display::Cell>(3)));
        });
    }

    TEST_CASE("scene rows decode UTF-8 and pad short rows") {
        constexpr std::array<std::string_view, 2> rows{"Aé", "中"};
        const auto scene = sart::display::Scene::from_rows(rows);
        CHECK(scene.dimensions() == sart::display::Dimensions(2, 2));
        CHECK(scene.get(0, 0)->glyph() == U'A');
        CHECK(scene.get(1, 0)->glyph() == U'é');
        CHECK(scene.get(0, 1)->glyph() == U'中');
        CHECK(scene.get(1, 1)->glyph() == U' ');
    }

    TEST_CASE("scene get and set enforce bounds") {
        sart::display::Scene scene(sart::display::Dimensions(2, 2));
        scene.set(1, 1, sart::display::Cell(U'X'));
        CHECK(scene.get(1, 1)->glyph() == U'X');
        CHECK_FALSE(scene.get(2, 1).has_value());
        check_scene_error(sart::display::SceneErrorCode::out_of_bounds,
                          [&] { scene.set(2, 1, sart::display::Cell(U'X')); });
    }

    TEST_CASE("resource ownership matches active display states") {
        using enum sart::display::DisplayState;
        CHECK_FALSE(sart::display::owns_resources(unacquired));
        CHECK(sart::display::owns_resources(acquiring));
        CHECK(sart::display::owns_resources(hidden));
        CHECK(sart::display::owns_resources(splash));
        CHECK(sart::display::owns_resources(details));
        CHECK_FALSE(sart::display::owns_resources(restored));
        CHECK_FALSE(sart::display::owns_resources(failed_open));
    }

    TEST_CASE("byte input event compares exact payload") {
        const std::array<std::uint8_t, 3> bytes{0, 127, 255};
        auto event = sart::display::InputEvent::bytes({bytes.begin(), bytes.end()});
        CHECK(event.kind() == sart::display::InputEvent::Kind::bytes);
        CHECK(event.equals_bytes(bytes));
        CHECK_FALSE(event.equals_bytes(std::span<const std::uint8_t>{bytes}.first(2)));
        CHECK_FALSE(event.resized_dimensions().has_value());
    }

    TEST_CASE("resize input event carries dimensions") {
        auto event = sart::display::InputEvent::resized(sart::display::Dimensions(100, 40));
        CHECK(event.kind() == sart::display::InputEvent::Kind::resized);
        REQUIRE(event.resized_dimensions().has_value());
        CHECK(*event.resized_dimensions() == sart::display::Dimensions(100, 40));
    }

    TEST_CASE("sensitive text counts ASCII and Unicode cells") {
        const sart::display::Dimensions dimensions(12, 2);
        CHECK(sart::display::validate_sensitive_text(dimensions, 0, 0, "abc") == 3);
        CHECK(sart::display::validate_sensitive_text(dimensions, 1, 2, "é中") == 4);
        CHECK(sart::display::validate_sensitive_text(dimensions, 1, 11, "") == 0);
    }

    TEST_CASE("sensitive text rejects invalid positions") {
        const sart::display::Dimensions dimensions(5, 2);
        check_display_error(sart::display::DisplayErrorCode::sensitive_text_out_of_bounds,
                            [&] { static_cast<void>(sart::display::validate_sensitive_text(dimensions, 2, 0, "x")); });
        check_display_error(sart::display::DisplayErrorCode::sensitive_text_out_of_bounds,
                            [&] { static_cast<void>(sart::display::validate_sensitive_text(dimensions, 0, 4, "xx")); });
    }

    TEST_CASE("sensitive text rejects controls and invalid UTF-8") {
        const sart::display::Dimensions dimensions(20, 2);
        check_display_error(sart::display::DisplayErrorCode::unsafe_sensitive_text, [&] {
            static_cast<void>(sart::display::validate_sensitive_text(dimensions, 0, 0, "a\nb"));
        });
        check_display_error(sart::display::DisplayErrorCode::unsafe_sensitive_text, [&] {
            static_cast<void>(sart::display::validate_sensitive_text(dimensions, 0, 0, "\xc0\x80"));
        });
    }

    TEST_CASE("buffer backend follows acquire show render restore lifecycle") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        CHECK(backend.state() == sart::display::DisplayState::unacquired);
        CHECK_FALSE(backend.dimensions().has_value());
        backend.acquire();
        backend.show();
        backend.render(sart::display::Scene(sart::display::Dimensions(4, 2)));
        backend.restore(sart::display::RestoreMode::retain_pixels);
        CHECK(backend.state() == sart::display::DisplayState::restored);
        CHECK(backend.frames().size() == 1);
        REQUIRE(backend.operations().size() == 4);
        CHECK(backend.operations().back().restore_mode == sart::display::RestoreMode::retain_pixels);
    }

    TEST_CASE("buffer backend idempotent transitions do not duplicate operations") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.acquire();
        backend.hide();
        backend.show();
        backend.show();
        backend.details(true);
        backend.details(true);
        backend.restore();
        backend.restore();
        CHECK(backend.operations().size() == 4);
    }

    TEST_CASE("buffer backend rejects render before show") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        check_display_error(sart::display::DisplayErrorCode::invalid_state,
                            [&] { backend.render(sart::display::Scene(sart::display::Dimensions(4, 2))); });
    }

    TEST_CASE("buffer backend rejects frames with other dimensions") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.show();
        check_display_error(sart::display::DisplayErrorCode::size_mismatch,
                            [&] { backend.render(sart::display::Scene(sart::display::Dimensions(3, 2))); });
    }

    TEST_CASE("buffer backend consumes resize and updates dimensions") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.show();
        backend.queue_input(sart::display::InputEvent::resized(sart::display::Dimensions(9, 3)));
        const auto event = backend.poll_input(std::chrono::milliseconds(7));
        REQUIRE(event.has_value());
        CHECK(event->kind() == sart::display::InputEvent::Kind::resized);
        CHECK(backend.dimensions() == sart::display::Dimensions(9, 3));
        CHECK(backend.operations().back().timeout == std::chrono::milliseconds(7));
    }

    TEST_CASE("details view consumes only return-to-splash input") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.show();
        backend.details(true);
        backend.queue_input(sart::display::InputEvent::bytes({27}));
        CHECK_FALSE(backend.poll_input(std::chrono::milliseconds(0)).has_value());
        backend.details(false);
        REQUIRE(backend.poll_input(std::chrono::milliseconds(0)).has_value());
        backend.details(true);
        backend.queue_input(sart::display::InputEvent::return_to_splash());
        const auto event = backend.poll_input(std::chrono::milliseconds(0));
        REQUIRE(event.has_value());
        CHECK(event->kind() == sart::display::InputEvent::Kind::return_to_splash);
    }

} // TEST_SUITE
