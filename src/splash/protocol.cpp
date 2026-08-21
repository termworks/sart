#include "sart/splash/protocol.hpp"

#include "sart/visual/art.hpp"

#include <algorithm>
#include <cerrno>
#include <cstring>
#include <format>
#include <optional>
#include <sys/socket.h>
#include <unistd.h>

namespace sart::splash {
    namespace {

        struct DecodedHeader {
            Opcode opcode;
            std::uint32_t flags;
            std::uint64_t request_id;
            std::size_t payload_length;
        };

        void append_u16(std::vector<std::uint8_t> &output, std::uint16_t value) {
            output.push_back(static_cast<std::uint8_t>(value >> 8));
            output.push_back(static_cast<std::uint8_t>(value));
        }

        void append_u32(std::vector<std::uint8_t> &output, std::uint32_t value) {
            for (int shift = 24; shift >= 0; shift -= 8) {
                output.push_back(static_cast<std::uint8_t>(value >> shift));
            }
        }

        void append_u64(std::vector<std::uint8_t> &output, std::uint64_t value) {
            for (int shift = 56; shift >= 0; shift -= 8) {
                output.push_back(static_cast<std::uint8_t>(value >> shift));
            }
        }

        std::uint16_t read_u16(std::span<const std::uint8_t> bytes) {
            return static_cast<std::uint16_t>((bytes[0] << 8) | bytes[1]);
        }

        std::uint32_t read_u32(std::span<const std::uint8_t> bytes) {
            std::uint32_t value{};
            for (const auto byte : bytes.first<4>())
                value = (value << 8) | byte;
            return value;
        }

        std::uint64_t read_u64(std::span<const std::uint8_t> bytes) {
            std::uint64_t value{};
            for (const auto byte : bytes.first<8>())
                value = (value << 8) | byte;
            return value;
        }

        Opcode decode_opcode(std::uint16_t value) {
            switch (value) {
            case 0x0001:
                return Opcode::ping;
            case 0x0002:
                return Opcode::show;
            case 0x0003:
                return Opcode::hide;
            case 0x0004:
                return Opcode::status;
            case 0x0005:
                return Opcode::progress;
            case 0x0006:
                return Opcode::message;
            case 0x0007:
                return Opcode::hide_message;
            case 0x0008:
                return Opcode::details_show;
            case 0x0009:
                return Opcode::details_hide;
            case 0x000a:
                return Opcode::details_toggle;
            case 0x000b:
                return Opcode::deactivate;
            case 0x000c:
                return Opcode::reactivate;
            case 0x000d:
                return Opcode::set_mode;
            case 0x000e:
                return Opcode::update_root_fs;
            case 0x000f:
                return Opcode::state;
            case 0x0010:
                return Opcode::quit;
            case 0x0011:
                return Opcode::native_ready;
            case 0x8000:
                return Opcode::ack;
            case 0x8001:
                return Opcode::error;
            case 0x8002:
                return Opcode::pong;
            case 0x8003:
                return Opcode::state_result;
            default:
                throw ProtocolError(ProtocolErrorCode::unknown_opcode, std::format("unknown opcode {:#06x}", value));
            }
        }

        std::uint8_t encode_mode(Mode mode) { return static_cast<std::uint8_t>(mode); }

        Mode decode_mode(std::uint8_t value) {
            if (value > static_cast<std::uint8_t>(Mode::upgrade)) {
                throw ProtocolError(ProtocolErrorCode::invalid_mode, "unknown presentation mode");
            }
            return static_cast<Mode>(value);
        }

        std::string_view validate_text_payload(Opcode opcode, std::span<const std::uint8_t> payload,
                                               std::size_t maximum, bool allow_empty) {
            if (payload.size() > maximum) {
                throw ProtocolError(ProtocolErrorCode::text_too_long, "protocol text is too long", maximum,
                                    payload.size());
            }
            if (payload.empty() && !allow_empty) {
                throw ProtocolError(ProtocolErrorCode::empty_text, "protocol text must not be empty");
            }
            const std::string_view text(reinterpret_cast<const char *>(payload.data()), payload.size());
            try {
                validate_display_text(text, maximum);
            } catch (const TextError &error) {
                throw ProtocolError(error.code() == TextErrorCode::invalid_utf8 ? ProtocolErrorCode::invalid_utf8
                                                                                : ProtocolErrorCode::invalid_text,
                                    std::format("invalid text for opcode {:#06x}: {}",
                                                static_cast<std::uint16_t>(opcode), error.what()));
            }
            return text;
        }

        void require_length(Opcode, std::span<const std::uint8_t> payload, std::size_t expected) {
            if (payload.size() != expected) {
                throw ProtocolError(ProtocolErrorCode::invalid_payload_length, "invalid protocol payload length",
                                    expected, payload.size());
            }
        }

        void validate_frame_fields(Opcode opcode, std::uint32_t flags, std::span<const std::uint8_t> payload) {
            if (payload.size() > maximum_payload_length) {
                throw ProtocolError(ProtocolErrorCode::payload_too_large, "protocol payload is too large",
                                    maximum_payload_length, payload.size());
            }
            const auto permitted_flags = opcode == Opcode::quit ? retain_splash_flag : 0U;
            if ((flags & ~retain_splash_flag) != 0) {
                throw ProtocolError(ProtocolErrorCode::unknown_flags, "unknown protocol flags");
            }
            if ((flags & ~permitted_flags) != 0) {
                throw ProtocolError(ProtocolErrorCode::flags_not_allowed, "flags are not allowed for opcode");
            }
            switch (opcode) {
            case Opcode::ping:
            case Opcode::show:
            case Opcode::hide:
            case Opcode::details_show:
            case Opcode::details_hide:
            case Opcode::details_toggle:
            case Opcode::deactivate:
            case Opcode::reactivate:
            case Opcode::state:
            case Opcode::quit:
            case Opcode::native_ready:
            case Opcode::ack:
            case Opcode::pong:
                require_length(opcode, payload, 0);
                break;
            case Opcode::status:
                validate_text_payload(opcode, payload, maximum_status_length, true);
                break;
            case Opcode::progress:
                require_length(opcode, payload, 1);
                if (payload[0] > 100) {
                    throw ProtocolError(ProtocolErrorCode::invalid_progress, "progress is outside 0..=100");
                }
                break;
            case Opcode::message:
                validate_text_payload(opcode, payload, maximum_message_length, false);
                break;
            case Opcode::hide_message:
                validate_text_payload(opcode, payload, maximum_message_length, true);
                break;
            case Opcode::set_mode:
                require_length(opcode, payload, 1);
                static_cast<void>(decode_mode(payload[0]));
                break;
            case Opcode::update_root_fs: {
                const auto path = validate_text_payload(opcode, payload, maximum_path_length, false);
                if (!path.starts_with('/')) {
                    throw ProtocolError(ProtocolErrorCode::invalid_root_path, "root path must be absolute");
                }
                break;
            }
            case Opcode::error:
                validate_text_payload(opcode, payload, maximum_error_length, false);
                break;
            case Opcode::state_result:
                validate_text_payload(opcode, payload, maximum_payload_length, false);
                break;
            }
        }

        DecodedHeader parse_header(std::span<const std::uint8_t> encoded) {
            if (!std::equal(protocol_magic.begin(), protocol_magic.end(), encoded.begin())) {
                throw ProtocolError(ProtocolErrorCode::invalid_magic, "invalid protocol magic");
            }
            const auto version = read_u16(encoded.subspan(4, 2));
            if (version != protocol_version) {
                throw ProtocolError(ProtocolErrorCode::unsupported_version, "unsupported protocol version");
            }
            const auto payload_length = static_cast<std::size_t>(read_u32(encoded.subspan(20, 4)));
            if (payload_length > maximum_payload_length) {
                throw ProtocolError(ProtocolErrorCode::payload_too_large, "protocol payload is too large",
                                    maximum_payload_length, payload_length);
            }
            return {
                decode_opcode(read_u16(encoded.subspan(6, 2))),
                read_u32(encoded.subspan(8, 4)),
                read_u64(encoded.subspan(12, 8)),
                payload_length,
            };
        }

        std::size_t read_fully(int descriptor, std::span<std::uint8_t> output) {
            std::size_t offset{};
            while (offset < output.size()) {
                const auto result = read(descriptor, output.data() + offset, output.size() - offset);
                if (result > 0) {
                    offset += static_cast<std::size_t>(result);
                } else if (result == 0) {
                    break;
                } else if (errno != EINTR) {
                    throw ProtocolError(ProtocolErrorCode::io, std::string("protocol read: ") + std::strerror(errno));
                }
            }
            return offset;
        }

    } // namespace

    ProtocolError::ProtocolError(ProtocolErrorCode code, std::string message, std::size_t expected, std::size_t actual)
        : std::runtime_error(std::move(message)), code_(code), expected_(expected), actual_(actual) {}

    ProtocolErrorCode ProtocolError::code() const noexcept { return code_; }
    std::size_t ProtocolError::expected() const noexcept { return expected_; }
    std::size_t ProtocolError::actual() const noexcept { return actual_; }

    Frame::Frame(Opcode opcode, std::uint32_t flags, std::uint64_t request_id, std::vector<std::uint8_t> payload)
        : opcode_(opcode), flags_(flags), request_id_(request_id), payload_(std::move(payload)) {
        validate_frame_fields(opcode_, flags_, payload_);
    }

    Frame Frame::empty(Opcode opcode, std::uint64_t request_id) { return {opcode, 0, request_id, {}}; }

    Frame Frame::text(Opcode opcode, std::uint64_t request_id, std::string_view text) {
        return {opcode, 0, request_id, std::vector<std::uint8_t>(text.begin(), text.end())};
    }

    Frame Frame::progress(std::uint64_t request_id, std::uint8_t percent) {
        return {Opcode::progress, 0, request_id, {percent}};
    }

    Frame Frame::mode(std::uint64_t request_id, Mode mode) {
        return {Opcode::set_mode, 0, request_id, {encode_mode(mode)}};
    }

    Frame Frame::quit(std::uint64_t request_id, bool retain_splash) {
        return {Opcode::quit, retain_splash ? retain_splash_flag : 0U, request_id, {}};
    }

    Frame Frame::ack(std::uint64_t request_id) { return empty(Opcode::ack, request_id); }
    Frame Frame::error(std::uint64_t request_id, std::string_view message) {
        return text(Opcode::error, request_id, message);
    }
    Frame Frame::pong(std::uint64_t request_id) { return empty(Opcode::pong, request_id); }
    Frame Frame::state_result(std::uint64_t request_id, std::string_view json) {
        return text(Opcode::state_result, request_id, json);
    }

    Frame Frame::decode_exact(std::span<const std::uint8_t> encoded) {
        if (encoded.size() < protocol_header_length) {
            throw ProtocolError(ProtocolErrorCode::truncated, "truncated protocol header", protocol_header_length,
                                encoded.size());
        }
        const auto header = parse_header(encoded.first(protocol_header_length));
        const auto expected = protocol_header_length + header.payload_length;
        if (encoded.size() < expected) {
            throw ProtocolError(ProtocolErrorCode::truncated, "truncated protocol payload", expected, encoded.size());
        }
        if (encoded.size() > expected) {
            throw ProtocolError(ProtocolErrorCode::trailing_bytes, "trailing protocol bytes", expected, encoded.size());
        }
        return {header.opcode, header.flags, header.request_id,
                std::vector<std::uint8_t>(encoded.begin() + protocol_header_length, encoded.end())};
    }

    Frame Frame::read_from_fd(int descriptor) {
        std::array<std::uint8_t, protocol_header_length> header_bytes{};
        const auto header_read = read_fully(descriptor, header_bytes);
        if (header_read != protocol_header_length) {
            throw ProtocolError(ProtocolErrorCode::truncated, "truncated protocol header", protocol_header_length,
                                header_read);
        }
        const auto header = parse_header(header_bytes);
        std::vector<std::uint8_t> payload(header.payload_length);
        const auto payload_read = read_fully(descriptor, payload);
        if (payload_read != payload.size()) {
            throw ProtocolError(ProtocolErrorCode::truncated, "truncated protocol payload",
                                protocol_header_length + payload.size(), protocol_header_length + payload_read);
        }
        return {header.opcode, header.flags, header.request_id, std::move(payload)};
    }

    Frame Frame::read_exact_message(int descriptor) {
        auto frame = read_from_fd(descriptor);
        std::uint8_t trailing{};
        const auto result = read(descriptor, &trailing, 1);
        if (result > 0) {
            throw ProtocolError(ProtocolErrorCode::trailing_bytes, "trailing protocol bytes", frame.encoded_length(),
                                frame.encoded_length() + 1);
        }
        if (result < 0 && errno != EINTR) {
            throw ProtocolError(ProtocolErrorCode::io, std::string("protocol read: ") + std::strerror(errno));
        }
        return frame;
    }

    std::uint16_t Frame::version() const noexcept { return version_; }
    Opcode Frame::opcode() const noexcept { return opcode_; }
    std::uint32_t Frame::flags() const noexcept { return flags_; }
    std::uint64_t Frame::request_id() const noexcept { return request_id_; }
    const std::vector<std::uint8_t> &Frame::payload() const noexcept { return payload_; }

    std::string_view Frame::payload_text() const {
        return {reinterpret_cast<const char *>(payload_.data()), payload_.size()};
    }

    std::optional<std::uint8_t> Frame::progress_value() const noexcept {
        return opcode_ == Opcode::progress ? std::optional{payload_[0]} : std::nullopt;
    }

    std::optional<Mode> Frame::mode_value() const {
        return opcode_ == Opcode::set_mode ? std::optional{decode_mode(payload_[0])} : std::nullopt;
    }

    bool Frame::retains_splash() const noexcept {
        return opcode_ == Opcode::quit && (flags_ & retain_splash_flag) != 0;
    }

    std::size_t Frame::encoded_length() const noexcept { return protocol_header_length + payload_.size(); }

    std::vector<std::uint8_t> Frame::encode() const {
        std::vector<std::uint8_t> output;
        output.reserve(encoded_length());
        output.insert(output.end(), protocol_magic.begin(), protocol_magic.end());
        append_u16(output, version_);
        append_u16(output, static_cast<std::uint16_t>(opcode_));
        append_u32(output, flags_);
        append_u64(output, request_id_);
        append_u32(output, static_cast<std::uint32_t>(payload_.size()));
        output.insert(output.end(), payload_.begin(), payload_.end());
        return output;
    }

    void Frame::write_to_fd(int descriptor) const {
        const auto encoded = encode();
        std::size_t offset{};
        while (offset < encoded.size()) {
            const auto result = write(descriptor, encoded.data() + offset, encoded.size() - offset);
            if (result > 0) {
                offset += static_cast<std::size_t>(result);
            } else if (result < 0 && errno == EINTR) {
                continue;
            } else {
                throw ProtocolError(ProtocolErrorCode::io, std::string("protocol write: ") + std::strerror(errno));
            }
        }
    }

} // namespace sart::splash
