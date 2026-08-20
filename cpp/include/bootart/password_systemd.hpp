#pragma once

#include "bootart/password_secure.hpp"

#include <compare>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace bootart::password {

    inline constexpr std::string_view ask_password_directory = "/run/systemd/ask-password";
    inline constexpr std::size_t maximum_ask_request_bytes = 8 * 1024;
    inline constexpr std::size_t maximum_ask_message_bytes = 1024;
    inline constexpr std::size_t maximum_ask_request_files = 256;

    struct AskRequestId {
        std::string name;
        std::uint64_t device{};
        std::uint64_t inode{};
        auto operator<=>(const AskRequestId &) const = default;
    };

    class AskRequest {
      public:
        static AskRequest parse(AskRequestId id, std::string_view contents);
        [[nodiscard]] const AskRequestId &id() const noexcept;
        [[nodiscard]] const std::string &message() const noexcept;
        [[nodiscard]] std::uint32_t requester_pid() const noexcept;
        [[nodiscard]] const std::filesystem::path &socket() const noexcept;
        [[nodiscard]] bool echo() const noexcept;
        [[nodiscard]] bool silent() const noexcept;
        [[nodiscard]] bool accept_cached_requested() const noexcept;
        [[nodiscard]] std::uint64_t not_after_microseconds() const noexcept;
        [[nodiscard]] bool expired(std::uint64_t now_microseconds) const noexcept;

      private:
        AskRequestId id_;
        std::string message_;
        std::uint32_t requester_pid_{};
        std::filesystem::path socket_;
        bool echo_{};
        bool silent_{};
        bool accept_cached_requested_{};
        std::uint64_t not_after_microseconds_{};
    };

    struct RejectedAskRequest {
        std::string name;
        std::string reason;
    };
    struct AskScanResult {
        std::vector<AskRequest> requests;
        std::vector<RejectedAskRequest> rejected;
    };

    [[nodiscard]] AskScanResult scan_ask_requests(const std::filesystem::path &directory = ask_password_directory,
                                                  std::uint32_t expected_uid = 0);
    [[nodiscard]] std::uint64_t monotonic_microseconds();
    [[nodiscard]] bool requester_alive(std::uint32_t pid);

    class SystemdReplySocket {
      public:
        SystemdReplySocket();
        SystemdReplySocket(const SystemdReplySocket &) = delete;
        SystemdReplySocket &operator=(const SystemdReplySocket &) = delete;
        SystemdReplySocket(SystemdReplySocket &&other) noexcept;
        SystemdReplySocket &operator=(SystemdReplySocket &&other) noexcept;
        ~SystemdReplySocket();

        void send_success(const AskRequest &request, SecureSecret &secret, std::uint32_t expected_uid = 0);
        void send_cancel(const AskRequest &request, std::uint32_t expected_uid = 0);

      private:
        void send(const AskRequest &request, std::span<const std::span<const std::byte>> parts,
                  std::uint32_t expected_uid);
        int descriptor_{-1};
    };

} // namespace bootart::password
