#pragma once

#include <chrono>
#include <compare>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace bootart {

    inline constexpr std::size_t maximum_scene_cells = 1'048'576;

    enum class SceneErrorCode {
        empty_dimensions,
        too_large,
        wrong_cell_count,
        control_glyph,
        out_of_bounds,
    };

    class SceneError final : public std::runtime_error {
      public:
        SceneError(SceneErrorCode code, std::string message);
        [[nodiscard]] SceneErrorCode code() const noexcept;

      private:
        SceneErrorCode code_;
    };

    class Dimensions {
      public:
        Dimensions(std::uint16_t columns, std::uint16_t rows);
        [[nodiscard]] std::uint16_t columns() const noexcept;
        [[nodiscard]] std::uint16_t rows() const noexcept;
        [[nodiscard]] std::size_t cell_count() const noexcept;
        auto operator<=>(const Dimensions &) const = default;

      private:
        std::uint16_t columns_;
        std::uint16_t rows_;
    };

    enum class Color {
        default_color,
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    };

    struct Style {
        Color foreground{Color::default_color};
        Color background{Color::default_color};
        bool bold{};
        auto operator<=>(const Style &) const = default;
    };

    class Cell {
      public:
        explicit Cell(char32_t glyph = U' ', Style style = {});
        [[nodiscard]] char32_t glyph() const noexcept;
        [[nodiscard]] Style style() const noexcept;
        auto operator<=>(const Cell &) const = default;

      private:
        char32_t glyph_;
        Style style_;
    };

    class Scene {
      public:
        explicit Scene(Dimensions dimensions);
        Scene(Dimensions dimensions, std::vector<Cell> cells);
        static Scene from_rows(std::span<const std::string_view> rows);

        [[nodiscard]] Dimensions dimensions() const noexcept;
        [[nodiscard]] const std::vector<Cell> &cells() const noexcept;
        [[nodiscard]] std::optional<Cell> get(std::uint16_t column, std::uint16_t row) const noexcept;
        void set(std::uint16_t column, std::uint16_t row, Cell cell);
        auto operator<=>(const Scene &) const = default;

      private:
        [[nodiscard]] std::optional<std::size_t> index(std::uint16_t column, std::uint16_t row) const noexcept;

        Dimensions dimensions_;
        std::vector<Cell> cells_;
    };

    enum class DisplayState {
        unacquired,
        acquiring,
        hidden,
        splash,
        details,
        restored,
        failed_open,
    };

    [[nodiscard]] bool owns_resources(DisplayState state) noexcept;

    enum class RestoreMode { clear, retain_pixels };

    class InputEvent {
      public:
        enum class Kind { bytes, resized, return_to_splash };

        static InputEvent bytes(std::vector<std::uint8_t> bytes);
        static InputEvent resized(Dimensions dimensions);
        static InputEvent return_to_splash();
        InputEvent(const InputEvent &) = delete;
        InputEvent &operator=(const InputEvent &) = delete;
        InputEvent(InputEvent &&other) noexcept;
        InputEvent &operator=(InputEvent &&other) noexcept;
        ~InputEvent();

        [[nodiscard]] Kind kind() const noexcept;
        [[nodiscard]] const std::vector<std::uint8_t> &byte_data() const noexcept;
        [[nodiscard]] std::optional<Dimensions> resized_dimensions() const noexcept;
        [[nodiscard]] bool equals_bytes(std::span<const std::uint8_t> expected) const noexcept;

      private:
        explicit InputEvent(Kind kind);
        void erase_bytes() noexcept;

        Kind kind_;
        std::vector<std::uint8_t> bytes_;
        std::optional<Dimensions> dimensions_;
    };

    enum class DisplayErrorCode {
        invalid_state,
        size_mismatch,
        sensitive_text_unsupported,
        sensitive_text_out_of_bounds,
        unsafe_sensitive_text,
        scene,
        backend,
        operation_and_restore,
    };

    class DisplayError final : public std::runtime_error {
      public:
        DisplayError(DisplayErrorCode code, std::string message, bool restoration_failed = false);
        [[nodiscard]] DisplayErrorCode code() const noexcept;
        [[nodiscard]] bool restoration_failed() const noexcept;

      private:
        DisplayErrorCode code_;
        bool restoration_failed_;
    };

    class DisplayBackend {
      public:
        virtual ~DisplayBackend() = default;
        [[nodiscard]] virtual DisplayState state() const noexcept = 0;
        [[nodiscard]] virtual std::optional<Dimensions> dimensions() const noexcept = 0;
        virtual void acquire() = 0;
        virtual void show() = 0;
        virtual void hide() = 0;
        virtual void render(const Scene &scene) = 0;
        virtual void render_sensitive_text(std::uint16_t row, std::uint16_t column, std::string_view text, Style style);
        [[nodiscard]] virtual std::optional<InputEvent> poll_input(std::chrono::milliseconds timeout) = 0;
        virtual void details(bool visible) = 0;
        virtual void restore() = 0;
        virtual void restore(RestoreMode mode);
    };

    [[nodiscard]] std::size_t validate_sensitive_text(Dimensions dimensions, std::uint16_t row, std::uint16_t column,
                                                      std::string_view text);

} // namespace bootart
