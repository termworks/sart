#pragma once

#include <compare>
#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace sart {

    inline constexpr std::size_t max_art_width = 512;
    inline constexpr std::size_t max_art_height = 256;
    inline constexpr std::size_t max_art_bytes = 1024 * 1024;

    struct Size {
        std::size_t width{};
        std::size_t height{};
        auto operator<=>(const Size &) const = default;
    };

    enum class ArtErrorCode {
        empty,
        invalid_utf8,
        no_visible_characters,
        contains_tab,
        contains_nul,
        contains_standalone_carriage_return,
        contains_control,
        exceeds_max_bytes,
        exceeds_max_width,
        exceeds_max_height,
    };

    class ArtError final : public std::runtime_error {
      public:
        ArtError(ArtErrorCode code, std::string message, std::uint32_t codepoint = 0);
        [[nodiscard]] ArtErrorCode code() const noexcept;
        [[nodiscard]] std::uint32_t codepoint() const noexcept;

      private:
        ArtErrorCode code_;
        std::uint32_t codepoint_;
    };

    class Art {
      public:
        static Art parse(std::string_view input);
        static Art parse_with_limits(std::string_view input, std::size_t maximum_width, std::size_t maximum_height);

        [[nodiscard]] std::size_t width() const noexcept;
        [[nodiscard]] std::size_t height() const noexcept;
        [[nodiscard]] Size size() const noexcept;
        [[nodiscard]] char32_t cell(std::size_t x, std::size_t y) const noexcept;

      private:
        std::size_t width_{};
        std::vector<std::u32string> lines_;
    };

    struct Layout {
        std::size_t source_x{};
        std::size_t source_y{};
        std::size_t visible_width{};
        std::size_t visible_height{};
        std::size_t destination_x{};
        std::size_t destination_y{};
        auto operator<=>(const Layout &) const = default;
    };

    [[nodiscard]] Layout layout(Size art_size, Size terminal_size) noexcept;
    [[nodiscard]] std::u32string decode_utf8(std::string_view input);
    [[nodiscard]] std::string encode_utf8(char32_t value);

} // namespace sart
