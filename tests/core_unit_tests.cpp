#include "sart/core/cmdline.hpp"
#include "sart/core/process.hpp"
#include "sart/core/sha256.hpp"

#include <doctest/doctest.h>

#include <filesystem>
#include <fstream>
#include <span>
#include <string>
#include <vector>

TEST_SUITE("core") {

    TEST_CASE("SHA-256 empty vector") {
        CHECK(sart::core::sha256("") == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    TEST_CASE("SHA-256 abc vector") {
        CHECK(sart::core::sha256("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    TEST_CASE("SHA-256 quick brown fox vector") {
        CHECK(sart::core::sha256("The quick brown fox jumps over the lazy dog") ==
              "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592");
    }

    TEST_CASE("SHA-256 distinguishes embedded NUL bytes") {
        const std::string bytes{"a\0b", 3};
        CHECK(sart::core::sha256(bytes) == "59b271ae1bbcb1d31d41929817f4b16fb439eb4f31520b5ad1d5ce98920a7138");
        CHECK(sart::core::sha256(bytes) != sart::core::sha256("a"));
    }

    TEST_CASE("SHA-256 incremental updates match one shot") {
        constexpr std::string_view input = "chunked digest input crossing no implementation boundary";
        sart::core::Sha256 digest;
        for (const auto character : input) {
            const auto byte = std::byte{static_cast<unsigned char>(character)};
            digest.update(std::span{&byte, 1});
        }
        const auto bytes = digest.finish();
        std::string hex;
        constexpr char digits[] = "0123456789abcdef";
        for (const auto byte : bytes) {
            const auto value = std::to_integer<unsigned>(byte);
            hex.push_back(digits[value >> 4]);
            hex.push_back(digits[value & 0xf]);
        }
        CHECK(hex == sart::core::sha256(input));
    }

    TEST_CASE("SHA-256 handles a complete block") {
        const std::string input(64, 'x');
        CHECK(sart::core::sha256(input) == "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c");
    }

    TEST_CASE("process guard refuses only PID 1") {
        CHECK_FALSE(sart::core::process_is_allowed(1));
        CHECK(sart::core::process_is_allowed(0));
        CHECK(sart::core::process_is_allowed(2));
        CHECK(sart::core::process_is_allowed(4'294'967'295U));
    }

    TEST_CASE("kernel command line accepts exact disable tokens") {
        CHECK(sart::core::cmdline::splash_disabled("sart=0"));
        CHECK(sart::core::cmdline::splash_disabled("quiet rd.sart=0 root=/dev/vda"));
        CHECK(sart::core::cmdline::splash_disabled("quiet\tsart=0\n"));
    }

    TEST_CASE("kernel command line rejects lookalike disable tokens") {
        CHECK_FALSE(sart::core::cmdline::splash_disabled("sart=1"));
        CHECK_FALSE(sart::core::cmdline::splash_disabled("xsart=0"));
        CHECK_FALSE(sart::core::cmdline::splash_disabled("sart=00"));
        CHECK_FALSE(sart::core::cmdline::splash_disabled("rd.sart=0x"));
    }

    TEST_CASE("kernel command line handles empty whitespace fields") {
        CHECK(sart::core::cmdline::splash_disabled("  \t\nrd.sart=0  "));
        CHECK_FALSE(sart::core::cmdline::splash_disabled(" \t\r\n "));
    }

    TEST_CASE("kernel command line file reader uses file contents") {
        const auto path = std::filesystem::path(SART_SOURCE_ROOT) / "target" / "cmdline-unit-test";
        std::filesystem::create_directories(path.parent_path());
        {
            std::ofstream output(path, std::ios::binary | std::ios::trunc);
            REQUIRE(output.good());
            output << "quiet rd.sart=0";
        }
        CHECK(sart::core::cmdline::splash_disabled_at(path));
        CHECK_FALSE(sart::core::cmdline::early_boot_enabled_at(path));
        std::filesystem::remove(path);
    }

    TEST_CASE("missing kernel command line fails closed") {
        const auto path = std::filesystem::path(SART_SOURCE_ROOT) / "target" / "missing-cmdline-unit-test";
        std::filesystem::remove(path);
        CHECK_THROWS(static_cast<void>(sart::core::cmdline::splash_disabled_at(path)));
        CHECK_FALSE(sart::core::cmdline::early_boot_enabled_at(path));
    }

} // TEST_SUITE
