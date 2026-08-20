#pragma once

#include <compare>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>

namespace sart::splash {

    inline constexpr std::string_view default_runtime_directory = "/run/sart";
    inline constexpr std::string_view control_socket_name = "control.sock";
    inline constexpr std::string_view native_password_socket_name = "native-password.sock";
    inline constexpr std::string_view daemon_lock_name = "daemon.lock";

    class FileDescriptor {
      public:
        FileDescriptor() = default;
        explicit FileDescriptor(int descriptor) noexcept;
        FileDescriptor(const FileDescriptor &) = delete;
        FileDescriptor &operator=(const FileDescriptor &) = delete;
        FileDescriptor(FileDescriptor &&other) noexcept;
        FileDescriptor &operator=(FileDescriptor &&other) noexcept;
        ~FileDescriptor();

        [[nodiscard]] int get() const noexcept;
        [[nodiscard]] int release() noexcept;
        explicit operator bool() const noexcept;

      private:
        int descriptor_{-1};
    };

    class RuntimePaths {
      public:
        explicit RuntimePaths(std::filesystem::path directory = "/run/sart");
        [[nodiscard]] const std::filesystem::path &directory() const noexcept;
        [[nodiscard]] const std::filesystem::path &socket() const noexcept;
        [[nodiscard]] const std::filesystem::path &native_password_socket() const noexcept;
        [[nodiscard]] const std::filesystem::path &lock() const noexcept;
        [[nodiscard]] bool is_production() const noexcept;
        [[nodiscard]] std::uint32_t required_daemon_uid() const noexcept;
        auto operator<=>(const RuntimePaths &) const = default;

      private:
        std::filesystem::path directory_;
        std::filesystem::path socket_;
        std::filesystem::path native_password_socket_;
        std::filesystem::path lock_;
    };

    enum class RuntimeErrorCode {
        wrong_daemon_uid,
        already_running,
        unsafe_path,
        unsafe_directory,
        unsafe_lock,
        unsafe_socket,
        io,
    };

    class RuntimeError final : public std::runtime_error {
      public:
        RuntimeError(RuntimeErrorCode code, std::string message);
        [[nodiscard]] RuntimeErrorCode code() const noexcept;

      private:
        RuntimeErrorCode code_;
    };

    struct PeerCredentials {
        std::uint32_t pid{};
        std::uint32_t uid{};
        std::uint32_t gid{};
        auto operator<=>(const PeerCredentials &) const = default;
    };

    class RuntimeOwner {
      public:
        struct Identity {
            std::uint64_t device{};
            std::uint64_t inode{};
            auto operator<=>(const Identity &) const = default;
        };

        static RuntimeOwner acquire(RuntimePaths paths);
        RuntimeOwner(const RuntimeOwner &) = delete;
        RuntimeOwner &operator=(const RuntimeOwner &) = delete;
        RuntimeOwner(RuntimeOwner &&other) noexcept;
        RuntimeOwner &operator=(RuntimeOwner &&) = delete;
        ~RuntimeOwner();

        [[nodiscard]] FileDescriptor bind_listener();
        [[nodiscard]] FileDescriptor bind_native_password_listener();
        [[nodiscard]] const RuntimePaths &paths() const noexcept;
        [[nodiscard]] std::uint32_t required_client_uid() const noexcept;
        [[nodiscard]] bool owned_entries_reachable() const noexcept;

      private:
        RuntimeOwner(RuntimePaths paths, FileDescriptor lock, Identity lock_identity, bool created_directory);
        void cleanup() noexcept;

        RuntimePaths paths_;
        FileDescriptor lock_;
        Identity lock_identity_;
        bool created_directory_{};
        std::optional<Identity> socket_identity_;
        std::optional<Identity> native_password_socket_identity_;
        bool active_{true};
    };

    [[nodiscard]] std::uint32_t effective_uid() noexcept;
    [[nodiscard]] PeerCredentials peer_credentials(int descriptor);

} // namespace sart::splash
