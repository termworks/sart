#include "sart/display.hpp"

#include "sart/art.hpp"

#include <algorithm>
#include <atomic>
#include <format>
#include <limits>
#include <utility>

namespace sart {
    namespace {

        bool control_character(char32_t value) { return value < 0x20 || (value >= 0x7f && value <= 0x9f); }

        bool unsafe_sensitive_character(char32_t value) {
            return control_character(value) || value == 0x061c || value == 0x200e || value == 0x200f ||
                   (value >= 0x202a && value <= 0x202e) || (value >= 0x2066 && value <= 0x2069);
        }

    } // namespace

    SceneError::SceneError(SceneErrorCode code, std::string message)
        : std::runtime_error(std::move(message)), code_(code) {}
    SceneErrorCode SceneError::code() const noexcept { return code_; }

    Dimensions::Dimensions(std::uint16_t columns, std::uint16_t rows) : columns_(columns), rows_(rows) {
        if (columns == 0 || rows == 0) {
            throw SceneError(SceneErrorCode::empty_dimensions,
                             std::format("scene dimensions must be non-zero, got {}x{}", columns, rows));
        }
        const auto cells = static_cast<std::size_t>(columns) * rows;
        if (cells > maximum_scene_cells) {
            throw SceneError(SceneErrorCode::too_large, "scene exceeds the cell limit");
        }
    }

    std::uint16_t Dimensions::columns() const noexcept { return columns_; }
    std::uint16_t Dimensions::rows() const noexcept { return rows_; }
    std::size_t Dimensions::cell_count() const noexcept { return static_cast<std::size_t>(columns_) * rows_; }

    Cell::Cell(char32_t glyph, Style style) : glyph_(glyph), style_(style) {
        if (control_character(glyph)) {
            throw SceneError(SceneErrorCode::control_glyph, "scene glyph is a terminal control");
        }
    }
    char32_t Cell::glyph() const noexcept { return glyph_; }
    Style Cell::style() const noexcept { return style_; }

    Scene::Scene(Dimensions dimensions) : dimensions_(dimensions), cells_(dimensions.cell_count()) {}

    Scene::Scene(Dimensions dimensions, std::vector<Cell> cells) : dimensions_(dimensions), cells_(std::move(cells)) {
        if (cells_.size() != dimensions_.cell_count()) {
            throw SceneError(SceneErrorCode::wrong_cell_count, "scene has the wrong cell count");
        }
    }

    Scene Scene::from_rows(std::span<const std::string_view> rows) {
        if (rows.empty() || rows.size() > std::numeric_limits<std::uint16_t>::max()) {
            throw SceneError(SceneErrorCode::empty_dimensions, "scene must have rows");
        }
        std::vector<std::u32string> decoded;
        decoded.reserve(rows.size());
        std::size_t maximum_columns{};
        for (const auto row : rows) {
            decoded.push_back(decode_utf8(row));
            maximum_columns = std::max(maximum_columns, decoded.back().size());
        }
        if (maximum_columns == 0 || maximum_columns > std::numeric_limits<std::uint16_t>::max()) {
            throw SceneError(SceneErrorCode::empty_dimensions, "scene must have columns");
        }
        Scene scene(Dimensions(static_cast<std::uint16_t>(maximum_columns), static_cast<std::uint16_t>(rows.size())));
        for (std::size_t row = 0; row < decoded.size(); ++row) {
            for (std::size_t column = 0; column < decoded[row].size(); ++column) {
                scene.set(static_cast<std::uint16_t>(column), static_cast<std::uint16_t>(row),
                          Cell(decoded[row][column]));
            }
        }
        return scene;
    }

    Dimensions Scene::dimensions() const noexcept { return dimensions_; }
    const std::vector<Cell> &Scene::cells() const noexcept { return cells_; }

    std::optional<std::size_t> Scene::index(std::uint16_t column, std::uint16_t row) const noexcept {
        if (column >= dimensions_.columns() || row >= dimensions_.rows())
            return std::nullopt;
        return static_cast<std::size_t>(row) * dimensions_.columns() + column;
    }

    std::optional<Cell> Scene::get(std::uint16_t column, std::uint16_t row) const noexcept {
        const auto position = index(column, row);
        return position ? std::optional{cells_[*position]} : std::nullopt;
    }

    void Scene::set(std::uint16_t column, std::uint16_t row, Cell cell) {
        const auto position = index(column, row);
        if (!position) {
            throw SceneError(SceneErrorCode::out_of_bounds, "cell is outside scene bounds");
        }
        cells_[*position] = cell;
    }

    bool owns_resources(DisplayState state) noexcept {
        return state == DisplayState::acquiring || state == DisplayState::hidden || state == DisplayState::splash ||
               state == DisplayState::details;
    }

    InputEvent::InputEvent(Kind kind) : kind_(kind) {}
    InputEvent InputEvent::bytes(std::vector<std::uint8_t> bytes) {
        InputEvent result(Kind::bytes);
        result.bytes_ = std::move(bytes);
        return result;
    }
    InputEvent InputEvent::resized(Dimensions dimensions) {
        InputEvent result(Kind::resized);
        result.dimensions_ = dimensions;
        return result;
    }
    InputEvent InputEvent::return_to_splash() { return InputEvent(Kind::return_to_splash); }
    InputEvent::InputEvent(InputEvent &&other) noexcept
        : kind_(other.kind_), bytes_(std::move(other.bytes_)), dimensions_(other.dimensions_) {}
    InputEvent &InputEvent::operator=(InputEvent &&other) noexcept {
        if (this != &other) {
            erase_bytes();
            kind_ = other.kind_;
            bytes_ = std::move(other.bytes_);
            dimensions_ = other.dimensions_;
        }
        return *this;
    }
    InputEvent::~InputEvent() { erase_bytes(); }
    void InputEvent::erase_bytes() noexcept {
        std::atomic_signal_fence(std::memory_order_seq_cst);
        for (auto &byte : bytes_) {
            *reinterpret_cast<volatile std::uint8_t *>(&byte) = 0;
        }
        std::atomic_signal_fence(std::memory_order_seq_cst);
    }
    InputEvent::Kind InputEvent::kind() const noexcept { return kind_; }
    const std::vector<std::uint8_t> &InputEvent::byte_data() const noexcept { return bytes_; }
    std::optional<Dimensions> InputEvent::resized_dimensions() const noexcept { return dimensions_; }
    bool InputEvent::equals_bytes(std::span<const std::uint8_t> expected) const noexcept {
        return kind_ == Kind::bytes && bytes_.size() == expected.size() &&
               std::equal(bytes_.begin(), bytes_.end(), expected.begin());
    }

    DisplayError::DisplayError(DisplayErrorCode code, std::string message, bool restoration_failed)
        : std::runtime_error(std::move(message)), code_(code), restoration_failed_(restoration_failed) {}
    DisplayErrorCode DisplayError::code() const noexcept { return code_; }
    bool DisplayError::restoration_failed() const noexcept { return restoration_failed_; }

    void DisplayBackend::render_sensitive_text(std::uint16_t, std::uint16_t, std::string_view, Style) {
        throw DisplayError(DisplayErrorCode::sensitive_text_unsupported,
                           "display backend does not support direct sensitive text");
    }

    void DisplayBackend::restore(RestoreMode) { restore(); }

    std::size_t validate_sensitive_text(Dimensions dimensions, std::uint16_t row, std::uint16_t column,
                                        std::string_view text) {
        std::u32string decoded;
        try {
            decoded = decode_utf8(text);
        } catch (...) {
            throw DisplayError(DisplayErrorCode::unsafe_sensitive_text, "sensitive text is not UTF-8");
        }
        std::size_t cells{};
        for (const auto character : decoded) {
            if (unsafe_sensitive_character(character)) {
                throw DisplayError(DisplayErrorCode::unsafe_sensitive_text,
                                   "sensitive text contains unsafe terminal characters");
            }
            cells += character <= 0x7f ? 1 : 2;
        }
        if (row >= dimensions.rows() || column >= dimensions.columns() ||
            cells > static_cast<std::size_t>(dimensions.columns() - column)) {
            throw DisplayError(DisplayErrorCode::sensitive_text_out_of_bounds,
                               "sensitive text is outside display bounds");
        }
        return cells;
    }

} // namespace sart
