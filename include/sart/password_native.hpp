#pragma once

#include "sart/password_secure.hpp"
#include "sart/splash/runtime.hpp"

#include <chrono>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <span>
#include <string>
#include <variant>

namespace sart::password {

    inline constexpr std::size_t maximum_responder_metadata_bytes = 2 * 1024;
    inline constexpr int native_askpass_output_fd = 8;
    inline constexpr int native_askpass_transport_exit_code = 75;
    inline constexpr int native_askpass_cancelled_exit_code = 76;

    enum class NativeAdapter : std::uint8_t {
        dracut_classic = 1,
        initramfs_tools_busybox = 2,
        mkinitfs_busybox = 3,
        mkinitfs_boot_deploy = 4,
        mkinitcpio_busybox = 5,
    };
    enum class PipeSecretFraming { exact, newline_terminated };
    [[nodiscard]] PipeSecretFraming secret_framing(NativeAdapter adapter) noexcept;
    [[nodiscard]] std::string_view prompt_source(NativeAdapter adapter) noexcept;

    class NativeCredentialClient {
      public:
        explicit NativeCredentialClient(splash::FileDescriptor descriptor);
        [[nodiscard]] int descriptor() const noexcept;
        [[nodiscard]] std::optional<SecureSecret> receive(std::size_t maximum_secret_bytes);

      private:
        splash::FileDescriptor descriptor_;
    };

    class NativeCredentialResponder {
      public:
        explicit NativeCredentialResponder(splash::FileDescriptor descriptor);
        [[nodiscard]] int descriptor() const noexcept;
        void reply_secret(SecureSecret &secret);
        void reply_cancel();

      private:
        void send_packet(std::span<const std::byte> header, std::span<const std::byte> payload = {});
        splash::FileDescriptor descriptor_;
    };

    struct NativeCredentialPair {
        NativeCredentialClient client;
        NativeCredentialResponder responder;
    };
    [[nodiscard]] NativeCredentialPair native_credential_pair();
    void send_responder_packet(int carrier, std::uint32_t expected_peer_uid, std::span<const std::byte> metadata,
                               NativeCredentialResponder responder,
                               std::chrono::milliseconds timeout = std::chrono::milliseconds(2000));
    struct ReceivedResponder {
        std::size_t metadata_size{};
        NativeCredentialResponder responder;
    };
    [[nodiscard]] ReceivedResponder receive_responder_packet(int carrier, std::uint32_t expected_peer_uid,
                                                             std::span<std::byte> metadata);

    struct NativeRequestIdentity {
        std::uint64_t request_id{};
        std::uint64_t generation{};
        std::uint32_t requester_pid{};
        std::uint64_t requester_start_ticks{};
        auto operator<=>(const NativeRequestIdentity &) const = default;
    };
    struct NativeRequestMetadata {
        NativeAdapter adapter{NativeAdapter::dracut_classic};
        NativeRequestIdentity identity;
        std::uint64_t deadline_microseconds{};
        std::uint16_t attempt{1};
        std::uint16_t attempts{1};
        std::size_t maximum_secret_bytes{default_secret_bytes};
        std::string prompt;
        bool echo{};
        bool silent{};
    };
    [[nodiscard]] std::vector<std::byte> encode_native_request(const NativeRequestMetadata &metadata);
    [[nodiscard]] NativeRequestMetadata decode_native_request(std::span<const std::byte> packet,
                                                              std::uint64_t now_microseconds);
    [[nodiscard]] std::uint64_t process_start_ticks(std::uint32_t pid);

    struct NativeAskpassMetadata {
        std::string prompt;
        std::uint16_t attempts{1};
        std::size_t maximum_secret_bytes{default_secret_bytes};
    };
    enum class NativeAskpassOutcome { delivered, user_cancelled, console_fallback };
    [[nodiscard]] splash::FileDescriptor claim_native_askpass_output();
    [[nodiscard]] NativeAskpassOutcome run_native_askpass_client(
        NativeAdapter adapter, const NativeAskpassMetadata &metadata, splash::FileDescriptor output,
        const std::filesystem::path &socket_path = "/run/sart/native-password.sock",
        std::uint32_t expected_daemon_uid = 0, std::chrono::milliseconds timeout = std::chrono::seconds(90)) noexcept;

} // namespace sart::password
