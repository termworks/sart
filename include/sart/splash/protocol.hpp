#pragma once

#include "sart/splash/state.hpp"

#include <array>
#include <compare>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace sart::splash {

    inline constexpr std::array<std::uint8_t, 4> protocol_magic{'B', 'A', 'R', 'T'};
    inline constexpr std::uint16_t protocol_version = 1;
    inline constexpr std::size_t protocol_header_length = 24;
    inline constexpr std::size_t maximum_payload_length = 8 * 1024;
    inline constexpr std::size_t maximum_frame_length = protocol_header_length + maximum_payload_length;
    inline constexpr std::size_t maximum_status_length = 2 * 1024;
    inline constexpr std::size_t maximum_message_length = 4 * 1024;
    inline constexpr std::size_t maximum_path_length = 4 * 1024;
    inline constexpr std::size_t maximum_error_length = 2 * 1024;
    inline constexpr std::uint32_t retain_splash_flag = 1U;

    enum class Opcode : std::uint16_t {
        ping = 0x0001,
        show = 0x0002,
        hide = 0x0003,
        status = 0x0004,
        progress = 0x0005,
        message = 0x0006,
        hide_message = 0x0007,
        details_show = 0x0008,
        details_hide = 0x0009,
        details_toggle = 0x000a,
        deactivate = 0x000b,
        reactivate = 0x000c,
        set_mode = 0x000d,
        update_root_fs = 0x000e,
        state = 0x000f,
        quit = 0x0010,
        native_ready = 0x0011,
        ack = 0x8000,
        error = 0x8001,
        pong = 0x8002,
        state_result = 0x8003,
    };

    enum class ProtocolErrorCode {
        truncated,
        trailing_bytes,
        invalid_magic,
        unsupported_version,
        unknown_opcode,
        unknown_flags,
        flags_not_allowed,
        payload_too_large,
        invalid_payload_length,
        text_too_long,
        empty_text,
        invalid_utf8,
        invalid_text,
        invalid_progress,
        invalid_mode,
        invalid_root_path,
        io,
    };

    class ProtocolError final : public std::runtime_error {
      public:
        ProtocolError(ProtocolErrorCode code, std::string message, std::size_t expected = 0, std::size_t actual = 0);
        [[nodiscard]] ProtocolErrorCode code() const noexcept;
        [[nodiscard]] std::size_t expected() const noexcept;
        [[nodiscard]] std::size_t actual() const noexcept;

      private:
        ProtocolErrorCode code_;
        std::size_t expected_;
        std::size_t actual_;
    };

    class Frame {
      public:
        Frame(Opcode opcode, std::uint32_t flags, std::uint64_t request_id, std::vector<std::uint8_t> payload);
        static Frame empty(Opcode opcode, std::uint64_t request_id);
        static Frame text(Opcode opcode, std::uint64_t request_id, std::string_view text);
        static Frame progress(std::uint64_t request_id, std::uint8_t percent);
        static Frame mode(std::uint64_t request_id, Mode mode);
        static Frame quit(std::uint64_t request_id, bool retain_splash);
        static Frame ack(std::uint64_t request_id);
        static Frame error(std::uint64_t request_id, std::string_view message);
        static Frame pong(std::uint64_t request_id);
        static Frame state_result(std::uint64_t request_id, std::string_view json);
        static Frame decode_exact(std::span<const std::uint8_t> encoded);
        static Frame read_from_fd(int descriptor);
        static Frame read_exact_message(int descriptor);

        [[nodiscard]] std::uint16_t version() const noexcept;
        [[nodiscard]] Opcode opcode() const noexcept;
        [[nodiscard]] std::uint32_t flags() const noexcept;
        [[nodiscard]] std::uint64_t request_id() const noexcept;
        [[nodiscard]] const std::vector<std::uint8_t> &payload() const noexcept;
        [[nodiscard]] std::string_view payload_text() const;
        [[nodiscard]] std::optional<std::uint8_t> progress_value() const noexcept;
        [[nodiscard]] std::optional<Mode> mode_value() const;
        [[nodiscard]] bool retains_splash() const noexcept;
        [[nodiscard]] std::size_t encoded_length() const noexcept;
        [[nodiscard]] std::vector<std::uint8_t> encode() const;
        void write_to_fd(int descriptor) const;
        auto operator<=>(const Frame &) const = default;

      private:
        std::uint16_t version_{protocol_version};
        Opcode opcode_;
        std::uint32_t flags_;
        std::uint64_t request_id_;
        std::vector<std::uint8_t> payload_;
    };

} // namespace sart::splash
