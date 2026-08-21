#include "sart/splash/protocol.hpp"

#include <doctest/doctest.h>

#include <array>
#include <cstdint>
#include <string>
#include <unistd.h>
#include <vector>

namespace {

    void check_protocol_error(sart::splash::ProtocolErrorCode code, const auto &action) {
        try {
            action();
            FAIL("expected ProtocolError");
        } catch (const sart::splash::ProtocolError &error) {
            CHECK(error.code() == code);
        }
    }

} // namespace

TEST_SUITE("protocol edges") {

    TEST_CASE("header preserves version flags request and length") {
        const auto source = sart::splash::Frame::quit(0x0102030405060708ULL, true);
        const auto decoded = sart::splash::Frame::decode_exact(source.encode());
        CHECK(decoded.version() == sart::splash::protocol_version);
        CHECK(decoded.flags() == sart::splash::retain_splash_flag);
        CHECK(decoded.request_id() == 0x0102030405060708ULL);
        CHECK(decoded.encoded_length() == sart::splash::protocol_header_length);
    }

    TEST_CASE("decoder rejects invalid protocol magic") {
        auto encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 1).encode();
        encoded[0] ^= 0xff;
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_magic,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
    }

    TEST_CASE("decoder rejects unsupported protocol version") {
        auto encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 1).encode();
        encoded[4] = 0;
        encoded[5] = 2;
        check_protocol_error(sart::splash::ProtocolErrorCode::unsupported_version,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
    }

    TEST_CASE("decoder rejects unknown flag bits") {
        auto encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 1).encode();
        encoded[11] = 2;
        check_protocol_error(sart::splash::ProtocolErrorCode::unknown_flags,
                             [&] { static_cast<void>(sart::splash::Frame::decode_exact(encoded)); });
    }

    TEST_CASE("empty opcodes reject payload bytes") {
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_payload_length,
                             [] { static_cast<void>(sart::splash::Frame(sart::splash::Opcode::show, 0, 1, {1})); });
    }

    TEST_CASE("payload length errors expose expected and actual sizes") {
        try {
            static_cast<void>(sart::splash::Frame(sart::splash::Opcode::progress, 0, 1, {}));
            FAIL("expected ProtocolError");
        } catch (const sart::splash::ProtocolError &error) {
            CHECK(error.code() == sart::splash::ProtocolErrorCode::invalid_payload_length);
            CHECK(error.expected() == 1);
            CHECK(error.actual() == 0);
        }
    }

    TEST_CASE("status may be empty while message may not") {
        CHECK_NOTHROW(sart::splash::Frame::text(sart::splash::Opcode::status, 1, ""));
        CHECK_NOTHROW(sart::splash::Frame::text(sart::splash::Opcode::hide_message, 1, ""));
        check_protocol_error(sart::splash::ProtocolErrorCode::empty_text, [] {
            static_cast<void>(sart::splash::Frame::text(sart::splash::Opcode::message, 1, ""));
        });
    }

    TEST_CASE("text payload rejects invalid UTF-8") {
        const std::string invalid{"\xc0\x80", 2};
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_utf8, [&] {
            static_cast<void>(sart::splash::Frame::text(sart::splash::Opcode::message, 1, invalid));
        });
    }

    TEST_CASE("text payload rejects terminal controls") {
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_text, [] {
            static_cast<void>(sart::splash::Frame::text(sart::splash::Opcode::message, 1, "unsafe\ntext"));
        });
    }

    TEST_CASE("progress rejects values over one hundred") {
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_progress,
                             [] { static_cast<void>(sart::splash::Frame::progress(1, 101)); });
    }

    TEST_CASE("mode rejects unknown encoded values") {
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_mode, [] {
            static_cast<void>(sart::splash::Frame(sart::splash::Opcode::set_mode, 0, 1, {255}));
        });
    }

    TEST_CASE("root update requires an absolute path") {
        check_protocol_error(sart::splash::ProtocolErrorCode::invalid_root_path, [] {
            static_cast<void>(sart::splash::Frame::text(sart::splash::Opcode::update_root_fs, 1, "relative/root"));
        });
        CHECK_NOTHROW(sart::splash::Frame::text(sart::splash::Opcode::update_root_fs, 1, "/sysroot"));
    }

    TEST_CASE("payloads cannot exceed the protocol maximum") {
        const std::vector<std::uint8_t> oversized(sart::splash::maximum_payload_length + 1, 'x');
        check_protocol_error(sart::splash::ProtocolErrorCode::payload_too_large, [&] {
            static_cast<void>(sart::splash::Frame(sart::splash::Opcode::state_result, 0, 1, oversized));
        });
    }

    TEST_CASE("mode factory round trips every presentation mode") {
        for (const auto mode : {sart::splash::Mode::boot, sart::splash::Mode::shutdown, sart::splash::Mode::reboot,
                                sart::splash::Mode::update, sart::splash::Mode::upgrade}) {
            const auto frame = sart::splash::Frame::decode_exact(sart::splash::Frame::mode(7, mode).encode());
            CHECK(frame.mode_value() == std::optional{mode});
        }
    }

    TEST_CASE("response factories preserve opcode and payload") {
        CHECK(sart::splash::Frame::ack(1).opcode() == sart::splash::Opcode::ack);
        CHECK(sart::splash::Frame::pong(2).opcode() == sart::splash::Opcode::pong);
        CHECK(sart::splash::Frame::error(3, "failed").payload_text() == "failed");
        CHECK(sart::splash::Frame::state_result(4, "{}").payload_text() == "{}");
    }

    TEST_CASE("file descriptor round trip reads one complete frame") {
        std::array<int, 2> descriptors{-1, -1};
        REQUIRE(pipe(descriptors.data()) == 0);
        const auto source = sart::splash::Frame::text(sart::splash::Opcode::message, 77, "Booting");
        source.write_to_fd(descriptors[1]);
        REQUIRE(close(descriptors[1]) == 0);
        descriptors[1] = -1;
        const auto decoded = sart::splash::Frame::read_exact_message(descriptors[0]);
        CHECK(decoded == source);
        CHECK(close(descriptors[0]) == 0);
    }

    TEST_CASE("exact descriptor reader detects trailing bytes") {
        std::array<int, 2> descriptors{-1, -1};
        REQUIRE(pipe(descriptors.data()) == 0);
        const auto encoded = sart::splash::Frame::empty(sart::splash::Opcode::ping, 88).encode();
        REQUIRE(write(descriptors[1], encoded.data(), encoded.size()) == static_cast<ssize_t>(encoded.size()));
        const std::uint8_t trailing = 0xff;
        REQUIRE(write(descriptors[1], &trailing, 1) == 1);
        REQUIRE(close(descriptors[1]) == 0);
        descriptors[1] = -1;
        check_protocol_error(sart::splash::ProtocolErrorCode::trailing_bytes,
                             [&] { static_cast<void>(sart::splash::Frame::read_exact_message(descriptors[0])); });
        CHECK(close(descriptors[0]) == 0);
    }

} // TEST_SUITE
