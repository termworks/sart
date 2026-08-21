#include "sart/core/sha256.hpp"

#include <algorithm>
#include <bit>
#include <cstdint>
#include <format>

namespace sart::core {
    namespace {

        constexpr std::array<std::uint32_t, 64> constants{
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
            0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
            0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
            0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
            0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
            0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
            0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

        std::uint32_t load32(const std::byte *bytes) {
            return std::to_integer<std::uint32_t>(bytes[0]) << 24 | std::to_integer<std::uint32_t>(bytes[1]) << 16 |
                   std::to_integer<std::uint32_t>(bytes[2]) << 8 | std::to_integer<std::uint32_t>(bytes[3]);
        }

    } // namespace

    Sha256::Sha256()
        : state_{0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19} {}

    void Sha256::update(std::span<const std::byte> bytes) {
        length_ += bytes.size();
        while (!bytes.empty()) {
            const auto count = std::min(bytes.size(), buffer_.size() - buffered_);
            std::copy_n(bytes.begin(), count, buffer_.begin() + static_cast<std::ptrdiff_t>(buffered_));
            buffered_ += count;
            bytes = bytes.subspan(count);
            if (buffered_ == buffer_.size()) {
                transform(buffer_.data());
                buffered_ = 0;
            }
        }
    }

    std::array<std::byte, 32> Sha256::finish() {
        const auto bit_length = length_ * 8;
        buffer_[buffered_++] = std::byte{0x80};
        if (buffered_ > 56) {
            std::fill(buffer_.begin() + static_cast<std::ptrdiff_t>(buffered_), buffer_.end(), std::byte{});
            transform(buffer_.data());
            buffered_ = 0;
        }
        std::fill(buffer_.begin() + static_cast<std::ptrdiff_t>(buffered_), buffer_.begin() + 56, std::byte{});
        for (std::size_t index = 0; index < 8; ++index) {
            buffer_[63 - index] = std::byte(static_cast<unsigned char>(bit_length >> (index * 8)));
        }
        transform(buffer_.data());
        std::array<std::byte, 32> output{};
        for (std::size_t word = 0; word < state_.size(); ++word) {
            for (std::size_t byte = 0; byte < 4; ++byte) {
                output[word * 4 + byte] = std::byte(static_cast<unsigned char>(state_[word] >> (24 - byte * 8)));
            }
        }
        return output;
    }

    void Sha256::transform(const std::byte *block) {
        std::array<std::uint32_t, 64> words{};
        for (std::size_t index = 0; index < 16; ++index)
            words[index] = load32(block + index * 4);
        for (std::size_t index = 16; index < words.size(); ++index) {
            const auto s0 =
                std::rotr(words[index - 15], 7) ^ std::rotr(words[index - 15], 18) ^ (words[index - 15] >> 3);
            const auto s1 =
                std::rotr(words[index - 2], 17) ^ std::rotr(words[index - 2], 19) ^ (words[index - 2] >> 10);
            words[index] = words[index - 16] + s0 + words[index - 7] + s1;
        }
        auto [a, b, c, d, e, f, g, h] = state_;
        for (std::size_t index = 0; index < words.size(); ++index) {
            const auto s1 = std::rotr(e, 6) ^ std::rotr(e, 11) ^ std::rotr(e, 25);
            const auto choice = (e & f) ^ (~e & g);
            const auto temporary1 = h + s1 + choice + constants[index] + words[index];
            const auto s0 = std::rotr(a, 2) ^ std::rotr(a, 13) ^ std::rotr(a, 22);
            const auto majority = (a & b) ^ (a & c) ^ (b & c);
            const auto temporary2 = s0 + majority;
            h = g;
            g = f;
            f = e;
            e = d + temporary1;
            d = c;
            c = b;
            b = a;
            a = temporary1 + temporary2;
        }
        state_[0] += a;
        state_[1] += b;
        state_[2] += c;
        state_[3] += d;
        state_[4] += e;
        state_[5] += f;
        state_[6] += g;
        state_[7] += h;
    }

    std::string sha256(std::span<const std::byte> bytes) {
        Sha256 digest;
        digest.update(bytes);
        const auto value = digest.finish();
        std::string output;
        output.reserve(64);
        for (const auto byte : value)
            output += std::format("{:02x}", std::to_integer<unsigned>(byte));
        return output;
    }

    std::string sha256(std::string_view bytes) { return sha256(std::as_bytes(std::span(bytes.data(), bytes.size()))); }

} // namespace sart::core
