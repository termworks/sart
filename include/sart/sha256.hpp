#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>

namespace sart {

    class Sha256 {
      public:
        Sha256();
        void update(std::span<const std::byte> bytes);
        [[nodiscard]] std::array<std::byte, 32> finish();

      private:
        void transform(const std::byte *block);
        std::array<std::uint32_t, 8> state_;
        std::array<std::byte, 64> buffer_{};
        std::uint64_t length_{};
        std::size_t buffered_{};
    };

    [[nodiscard]] std::string sha256(std::span<const std::byte> bytes);
    [[nodiscard]] std::string sha256(std::string_view bytes);

} // namespace sart
