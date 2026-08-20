#pragma once

#include <cstddef>
#include <optional>
#include <span>
#include <string_view>
#include <utility>

namespace sart::password {

    class NativeCredentialClient;

    inline constexpr std::size_t maximum_secret_bytes = 4 * 1024;
    inline constexpr std::size_t default_secret_bytes = 1024;

    struct SecretProtection {
        bool locked{};
        bool excluded_from_core{};
        auto operator<=>(const SecretProtection &) const = default;
    };

    void protect_process_secrets();

    class SecureSecret {
      public:
        explicit SecureSecret(std::size_t capacity);
        SecureSecret(const SecureSecret &) = delete;
        SecureSecret &operator=(const SecureSecret &) = delete;
        SecureSecret(SecureSecret &&other) noexcept;
        SecureSecret &operator=(SecureSecret &&other) noexcept;
        ~SecureSecret();

        [[nodiscard]] std::size_t size() const noexcept;
        [[nodiscard]] bool empty() const noexcept;
        [[nodiscard]] std::size_t capacity() const noexcept;
        [[nodiscard]] SecretProtection protection() const noexcept;
        void push(char32_t character);
        void push(std::string_view text);
        [[nodiscard]] std::optional<char32_t> pop();
        void clear() noexcept;

        template <typename Function> decltype(auto) expose(Function &&action) const {
            return std::forward<Function>(action)(std::span<const std::byte>(data_, length_));
        }

      private:
        friend class NativeCredentialClient;
        [[nodiscard]] std::span<std::byte> receive_buffer() noexcept;
        void commit_received(std::size_t length);
        void release() noexcept;
        void zero_range(std::size_t begin, std::size_t end) noexcept;

        std::byte *data_{};
        std::size_t length_{};
        std::size_t capacity_{};
        std::size_t mapping_length_{};
        SecretProtection protection_{};
    };

} // namespace sart::password
