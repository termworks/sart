#include "sart/display/backend.hpp"
#include "sart/display/buffer.hpp"
#include "sart/visual/art.hpp"

#include <doctest/doctest.h>

#include <array>
#include <chrono>
#include <cstdint>
#include <optional>
#include <string_view>
#include <utility>

namespace {

    void check_scene_error(sart::display::SceneErrorCode code, const auto &action) {
        try {
            action();
            FAIL("expected SceneError");
        } catch (const sart::display::SceneError &error) {
            CHECK(error.code() == code);
        }
    }

    void check_display_error(sart::display::DisplayErrorCode code, const auto &action) {
        try {
            action();
            FAIL("expected DisplayError");
        } catch (const sart::display::DisplayError &error) {
            CHECK(error.code() == code);
        }
    }

    class MinimalBackend final : public sart::display::DisplayBackend {
      public:
        [[nodiscard]] sart::display::DisplayState state() const noexcept override {
            return sart::display::DisplayState::splash;
        }
        [[nodiscard]] std::optional<sart::display::Dimensions> dimensions() const noexcept override {
            return sart::display::Dimensions(4, 2);
        }
        void acquire() override {}
        void show() override {}
        void hide() override {}
        void render(const sart::display::Scene &) override {}
        [[nodiscard]] std::optional<sart::display::InputEvent> poll_input(std::chrono::milliseconds) override {
            return std::nullopt;
        }
        void details(bool) override {}
        void restore() override { ++restore_count; }

        std::size_t restore_count{};
    };

} // namespace

TEST_SUITE("display edges") {

    TEST_CASE("scene row constructor rejects no rows") {
        constexpr std::array<std::string_view, 0> rows{};
        check_scene_error(sart::display::SceneErrorCode::empty_dimensions,
                          [&] { static_cast<void>(sart::display::Scene::from_rows(rows)); });
    }

    TEST_CASE("scene row constructor rejects rows without columns") {
        constexpr std::array<std::string_view, 2> rows{"", ""};
        check_scene_error(sart::display::SceneErrorCode::empty_dimensions,
                          [&] { static_cast<void>(sart::display::Scene::from_rows(rows)); });
    }

    TEST_CASE("scene row constructor rejects invalid UTF-8") {
        constexpr std::array<std::string_view, 1> rows{"\xc0\x80"};
        CHECK_THROWS_AS(static_cast<void>(sart::display::Scene::from_rows(rows)), sart::visual::ArtError);
    }

    TEST_CASE("scene cell storage is row major") {
        sart::display::Scene scene(sart::display::Dimensions(3, 2));
        scene.set(0, 0, sart::display::Cell(U'A'));
        scene.set(2, 0, sart::display::Cell(U'B'));
        scene.set(0, 1, sart::display::Cell(U'C'));
        REQUIRE(scene.cells().size() == 6);
        CHECK(scene.cells()[0].glyph() == U'A');
        CHECK(scene.cells()[2].glyph() == U'B');
        CHECK(scene.cells()[3].glyph() == U'C');
    }

    TEST_CASE("input move construction preserves byte payload") {
        auto source = sart::display::InputEvent::bytes({1, 2, 3, 255});
        const sart::display::InputEvent destination(std::move(source));
        CHECK(destination.kind() == sart::display::InputEvent::Kind::bytes);
        CHECK(destination.equals_bytes(std::array<std::uint8_t, 4>{1, 2, 3, 255}));
    }

    TEST_CASE("input move assignment replaces event metadata") {
        auto destination = sart::display::InputEvent::bytes({9, 8, 7});
        auto source = sart::display::InputEvent::resized(sart::display::Dimensions(12, 4));
        destination = std::move(source);
        CHECK(destination.kind() == sart::display::InputEvent::Kind::resized);
        CHECK(destination.resized_dimensions() == std::optional{sart::display::Dimensions(12, 4)});
        CHECK(destination.byte_data().empty());
    }

    TEST_CASE("display error exposes restoration state") {
        const sart::display::DisplayError ordinary(sart::display::DisplayErrorCode::invalid_state, "ordinary");
        const sart::display::DisplayError restoration(sart::display::DisplayErrorCode::operation_and_restore, "restore",
                                                      true);
        CHECK_FALSE(ordinary.restoration_failed());
        CHECK(restoration.restoration_failed());
        CHECK(restoration.code() == sart::display::DisplayErrorCode::operation_and_restore);
    }

    TEST_CASE("display base class rejects sensitive output by default") {
        MinimalBackend backend;
        check_display_error(sart::display::DisplayErrorCode::sensitive_text_unsupported,
                            [&] { backend.render_sensitive_text(0, 0, "secret", {}); });
    }

    TEST_CASE("display base restore mode delegates to ordinary restore") {
        MinimalBackend backend;
        static_cast<sart::display::DisplayBackend &>(backend).restore(sart::display::RestoreMode::retain_pixels);
        CHECK(backend.restore_count == 1);
    }

    TEST_CASE("sensitive text rejects bidirectional formatting controls") {
        const sart::display::Dimensions dimensions(20, 2);
        for (const auto text : {"\xe2\x80\xae", "\xe2\x81\xa6", "\xd8\x9c"}) {
            check_display_error(sart::display::DisplayErrorCode::unsafe_sensitive_text, [&] {
                static_cast<void>(sart::display::validate_sensitive_text(dimensions, 0, 0, text));
            });
        }
    }

    TEST_CASE("buffer records sensitive text placement and cell width") {
        sart::display::BufferBackend backend(sart::display::Dimensions(10, 2));
        backend.acquire();
        backend.show();
        backend.render_sensitive_text(1, 3, "éx", {});
        REQUIRE(backend.operations().size() == 3);
        const auto &operation = backend.operations().back();
        CHECK(operation.kind == sart::display::BufferOperation::Kind::render_sensitive_text);
        CHECK(operation.row == 1);
        CHECK(operation.column == 3);
        CHECK(operation.cells == 3);
    }

    TEST_CASE("empty buffer poll records its timeout") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.show();
        CHECK_FALSE(backend.poll_input(std::chrono::milliseconds(19)).has_value());
        REQUIRE(backend.operations().size() == 3);
        CHECK(backend.operations().back().kind == sart::display::BufferOperation::Kind::poll_input);
        CHECK(backend.operations().back().timeout == std::chrono::milliseconds(19));
    }

    TEST_CASE("restored buffer refuses reacquisition") {
        sart::display::BufferBackend backend(sart::display::Dimensions(4, 2));
        backend.acquire();
        backend.restore();
        check_display_error(sart::display::DisplayErrorCode::invalid_state, [&] { backend.acquire(); });
        check_display_error(sart::display::DisplayErrorCode::invalid_state, [&] { backend.show(); });
    }

} // TEST_SUITE
