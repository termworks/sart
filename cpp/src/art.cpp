#include "bootart/art.hpp"

#include <algorithm>
#include <format>
#include <limits>

namespace bootart {
    namespace {

        bool unsafe_character(char32_t value) {
            return value < 0x20 || (value >= 0x7f && value <= 0x9f) || value == 0x061c || value == 0x200e ||
                   value == 0x200f || value == 0x2028 || value == 0x2029 || (value >= 0x202a && value <= 0x202e) ||
                   (value >= 0x2066 && value <= 0x2069);
        }

        bool trailing_space(char32_t value) {
            return value == U' ' || value == 0x00a0 || value == 0x1680 || (value >= 0x2000 && value <= 0x200a) ||
                   value == 0x202f || value == 0x205f || value == 0x3000;
        }

        ArtError invalid_utf8() { return {ArtErrorCode::invalid_utf8, "art is not valid UTF-8"}; }

    } // namespace

    ArtError::ArtError(ArtErrorCode code, std::string message, std::uint32_t codepoint)
        : std::runtime_error(std::move(message)), code_(code), codepoint_(codepoint) {}

    ArtErrorCode ArtError::code() const noexcept { return code_; }

    std::uint32_t ArtError::codepoint() const noexcept { return codepoint_; }

    std::u32string decode_utf8(std::string_view input) {
        std::u32string output;
        output.reserve(input.size());
        for (std::size_t offset = 0; offset < input.size();) {
            const auto first = static_cast<unsigned char>(input[offset]);
            char32_t value{};
            std::size_t length{};
            char32_t minimum{};
            if (first <= 0x7f) {
                value = first;
                length = 1;
                minimum = 0;
            } else if ((first & 0xe0) == 0xc0) {
                value = first & 0x1f;
                length = 2;
                minimum = 0x80;
            } else if ((first & 0xf0) == 0xe0) {
                value = first & 0x0f;
                length = 3;
                minimum = 0x800;
            } else if ((first & 0xf8) == 0xf0) {
                value = first & 0x07;
                length = 4;
                minimum = 0x10000;
            } else {
                throw invalid_utf8();
            }
            if (offset + length > input.size()) {
                throw invalid_utf8();
            }
            for (std::size_t index = 1; index < length; ++index) {
                const auto byte = static_cast<unsigned char>(input[offset + index]);
                if ((byte & 0xc0) != 0x80) {
                    throw invalid_utf8();
                }
                value = (value << 6) | (byte & 0x3f);
            }
            if (value < minimum || value > 0x10ffff || (value >= 0xd800 && value <= 0xdfff)) {
                throw invalid_utf8();
            }
            output.push_back(value);
            offset += length;
        }
        return output;
    }

    std::string encode_utf8(char32_t value) {
        std::string output;
        if (value <= 0x7f) {
            output.push_back(static_cast<char>(value));
        } else if (value <= 0x7ff) {
            output.push_back(static_cast<char>(0xc0 | (value >> 6)));
            output.push_back(static_cast<char>(0x80 | (value & 0x3f)));
        } else if (value <= 0xffff) {
            output.push_back(static_cast<char>(0xe0 | (value >> 12)));
            output.push_back(static_cast<char>(0x80 | ((value >> 6) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | (value & 0x3f)));
        } else {
            output.push_back(static_cast<char>(0xf0 | (value >> 18)));
            output.push_back(static_cast<char>(0x80 | ((value >> 12) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | ((value >> 6) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | (value & 0x3f)));
        }
        return output;
    }

    Art Art::parse(std::string_view input) { return parse_with_limits(input, max_art_width, max_art_height); }

    Art Art::parse_with_limits(std::string_view input, std::size_t maximum_width, std::size_t maximum_height) {
        if (input.empty()) {
            throw ArtError(ArtErrorCode::empty, "art file is empty");
        }
        if (input.size() > max_art_bytes) {
            throw ArtError(ArtErrorCode::exceeds_max_bytes,
                           std::format("art is {} bytes; maximum allowed is {} bytes", input.size(), max_art_bytes));
        }

        std::string normalized;
        normalized.reserve(input.size());
        for (std::size_t index = 0; index < input.size(); ++index) {
            if (input[index] == '\r' && index + 1 < input.size() && input[index + 1] == '\n') {
                normalized.push_back('\n');
                ++index;
            } else {
                normalized.push_back(input[index]);
            }
        }
        const auto characters = decode_utf8(normalized);
        for (const auto value : characters) {
            if (value == U'\r') {
                throw ArtError(ArtErrorCode::contains_standalone_carriage_return,
                               "art contains un-normalized carriage return '\\r'");
            }
            if (value == U'\0') {
                throw ArtError(ArtErrorCode::contains_nul, "art contains NUL bytes");
            }
            if (value == U'\t') {
                throw ArtError(ArtErrorCode::contains_tab, "art contains tab characters");
            }
            if (value != U'\n' && unsafe_character(value)) {
                throw ArtError(
                    ArtErrorCode::contains_control,
                    std::format("art contains unsafe control character U+{:04X}", static_cast<std::uint32_t>(value)),
                    static_cast<std::uint32_t>(value));
            }
        }

        std::vector<std::u32string> raw_lines(1);
        for (const auto value : characters) {
            if (value == U'\n') {
                raw_lines.emplace_back();
            } else {
                raw_lines.back().push_back(value);
            }
        }
        for (auto &line : raw_lines) {
            while (!line.empty() && trailing_space(line.back())) {
                line.pop_back();
            }
        }

        auto first = std::find_if(raw_lines.begin(), raw_lines.end(), [](const auto &line) { return !line.empty(); });
        if (first == raw_lines.end()) {
            throw ArtError(ArtErrorCode::no_visible_characters, "art contains no non-space visible characters");
        }
        auto last =
            std::find_if(raw_lines.rbegin(), raw_lines.rend(), [](const auto &line) { return !line.empty(); }).base();

        Art art;
        art.lines_.assign(first, last);
        for (const auto &line : art.lines_) {
            art.width_ = std::max(art.width_, line.size());
        }
        if (art.lines_.size() > maximum_height) {
            throw ArtError(ArtErrorCode::exceeds_max_height,
                           std::format("art height ({} rows) exceeds maximum allowed ({} rows)", art.lines_.size(),
                                       maximum_height));
        }
        if (art.width_ > maximum_width) {
            throw ArtError(
                ArtErrorCode::exceeds_max_width,
                std::format("art width ({} cols) exceeds maximum allowed ({} cols)", art.width_, maximum_width));
        }
        return art;
    }

    std::size_t Art::width() const noexcept { return width_; }

    std::size_t Art::height() const noexcept { return lines_.size(); }

    Size Art::size() const noexcept { return {width(), height()}; }

    char32_t Art::cell(std::size_t x, std::size_t y) const noexcept {
        if (y >= lines_.size() || x >= lines_[y].size()) {
            return U' ';
        }
        return lines_[y][x];
    }

    Layout layout(Size art_size, Size terminal_size) noexcept {
        if (terminal_size.width == 0 || terminal_size.height == 0) {
            return {};
        }
        Layout result;
        if (art_size.width <= terminal_size.width) {
            result.visible_width = art_size.width;
            result.destination_x = (terminal_size.width - art_size.width) / 2;
        } else {
            result.visible_width = terminal_size.width;
            result.source_x = (art_size.width - terminal_size.width) / 2;
        }
        if (art_size.height <= terminal_size.height) {
            result.visible_height = art_size.height;
            result.destination_y = (terminal_size.height - art_size.height) / 2;
        } else {
            result.visible_height = terminal_size.height;
            result.source_y = (art_size.height - terminal_size.height) / 2;
        }
        return result;
    }

} // namespace bootart
