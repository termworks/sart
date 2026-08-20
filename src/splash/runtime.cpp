#include "sart/splash/runtime.hpp"

#include <algorithm>
#include <cerrno>
#include <compare>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <limits.h>
#include <sys/file.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

namespace sart::splash {
    namespace {

        [[noreturn]] void io_error(std::string_view operation, const std::filesystem::path &path) {
            throw RuntimeError(RuntimeErrorCode::io,
                               std::format("failed to {} {}: {}", operation, path.string(), std::strerror(errno)));
        }

        RuntimeOwner::Identity identity(const struct stat &metadata) {
            return {static_cast<std::uint64_t>(metadata.st_dev), static_cast<std::uint64_t>(metadata.st_ino)};
        }

        bool path_has_identity(const std::filesystem::path &path, RuntimeOwner::Identity expected) noexcept {
            struct stat metadata{};
            return lstat(path.c_str(), &metadata) == 0 && identity(metadata) == expected;
        }

        void remove_if_same(const std::filesystem::path &path, RuntimeOwner::Identity expected) noexcept {
            if (path_has_identity(path, expected))
                unlink(path.c_str());
        }

        sockaddr_un unix_address(const std::filesystem::path &path, socklen_t &length) {
            const auto bytes = path.native();
            sockaddr_un address{};
            if (!path.is_absolute() || bytes.empty() || bytes.find('\0') != std::string::npos ||
                bytes.size() >= sizeof(address.sun_path)) {
                throw RuntimeError(RuntimeErrorCode::unsafe_path, "Unix socket path is invalid or too long");
            }
            address.sun_family = AF_UNIX;
            std::memcpy(address.sun_path, bytes.data(), bytes.size());
            address.sun_path[bytes.size()] = '\0';
            length = static_cast<socklen_t>(offsetof(sockaddr_un, sun_path) + bytes.size() + 1);
            return address;
        }

        void validate_runtime_path(const std::filesystem::path &path) {
            const auto native = path.native();
            if (!path.is_absolute() || path == "/" || native.find('\n') != std::string::npos ||
                native.find('\r') != std::string::npos || native.find('\0') != std::string::npos) {
                throw RuntimeError(RuntimeErrorCode::unsafe_path, "runtime path is unsafe");
            }
            for (const auto &component : path) {
                if (component == "." || component == "..") {
                    throw RuntimeError(RuntimeErrorCode::unsafe_path, "runtime path is not normalized");
                }
            }
            std::error_code error;
            const auto canonical_parent = std::filesystem::canonical(path.parent_path(), error);
            if (error || canonical_parent != path.parent_path()) {
                throw RuntimeError(RuntimeErrorCode::unsafe_path, "runtime parent is not canonical");
            }
        }

        void validate_directory(const std::filesystem::path &path, std::uint32_t required_uid) {
            struct stat metadata{};
            if (lstat(path.c_str(), &metadata) != 0)
                io_error("inspect runtime directory", path);
            if (!S_ISDIR(metadata.st_mode) || metadata.st_uid != required_uid || (metadata.st_mode & 0777) != 0700) {
                throw RuntimeError(RuntimeErrorCode::unsafe_directory, "runtime directory is unsafe");
            }
        }

    } // namespace

    FileDescriptor::FileDescriptor(int descriptor) noexcept : descriptor_(descriptor) {}
    FileDescriptor::FileDescriptor(FileDescriptor &&other) noexcept : descriptor_(other.release()) {}
    FileDescriptor &FileDescriptor::operator=(FileDescriptor &&other) noexcept {
        if (this != &other) {
            if (descriptor_ >= 0)
                close(descriptor_);
            descriptor_ = other.release();
        }
        return *this;
    }
    FileDescriptor::~FileDescriptor() {
        if (descriptor_ >= 0)
            close(descriptor_);
    }
    int FileDescriptor::get() const noexcept { return descriptor_; }
    int FileDescriptor::release() noexcept {
        const auto descriptor = descriptor_;
        descriptor_ = -1;
        return descriptor;
    }
    FileDescriptor::operator bool() const noexcept { return descriptor_ >= 0; }

    RuntimePaths::RuntimePaths(std::filesystem::path directory)
        : directory_(std::move(directory)), socket_(directory_ / control_socket_name),
          native_password_socket_(directory_ / native_password_socket_name), lock_(directory_ / daemon_lock_name) {}

    const std::filesystem::path &RuntimePaths::directory() const noexcept { return directory_; }
    const std::filesystem::path &RuntimePaths::socket() const noexcept { return socket_; }
    const std::filesystem::path &RuntimePaths::native_password_socket() const noexcept {
        return native_password_socket_;
    }
    const std::filesystem::path &RuntimePaths::lock() const noexcept { return lock_; }
    bool RuntimePaths::is_production() const noexcept { return directory_ == default_runtime_directory; }
    std::uint32_t RuntimePaths::required_daemon_uid() const noexcept { return is_production() ? 0U : effective_uid(); }

    RuntimeError::RuntimeError(RuntimeErrorCode code, std::string message)
        : std::runtime_error(std::move(message)), code_(code) {}
    RuntimeErrorCode RuntimeError::code() const noexcept { return code_; }

    RuntimeOwner::RuntimeOwner(RuntimePaths paths, FileDescriptor lock, Identity lock_identity, bool created_directory)
        : paths_(std::move(paths)), lock_(std::move(lock)), lock_identity_(lock_identity),
          created_directory_(created_directory) {}

    RuntimeOwner::RuntimeOwner(RuntimeOwner &&other) noexcept
        : paths_(std::move(other.paths_)), lock_(std::move(other.lock_)), lock_identity_(other.lock_identity_),
          created_directory_(other.created_directory_), socket_identity_(other.socket_identity_),
          native_password_socket_identity_(other.native_password_socket_identity_), active_(other.active_) {
        other.active_ = false;
    }

    RuntimeOwner RuntimeOwner::acquire(RuntimePaths paths) {
        validate_runtime_path(paths.directory());
        const auto required_uid = paths.required_daemon_uid();
        if (effective_uid() != required_uid) {
            throw RuntimeError(RuntimeErrorCode::wrong_daemon_uid, "daemon UID is not allowed");
        }
        bool created_directory{};
        if (mkdir(paths.directory().c_str(), 0700) == 0) {
            created_directory = true;
        } else if (errno != EEXIST) {
            io_error("create runtime directory", paths.directory());
        }
        try {
            validate_directory(paths.directory(), required_uid);
        } catch (...) {
            if (created_directory)
                rmdir(paths.directory().c_str());
            throw;
        }

        struct stat path_lock_metadata{};
        const auto lock_existed = lstat(paths.lock().c_str(), &path_lock_metadata) == 0;
        FileDescriptor lock(open(paths.lock().c_str(), O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0600));
        if (!lock) {
            if (created_directory)
                rmdir(paths.directory().c_str());
            io_error("open daemon lock", paths.lock());
        }
        struct stat lock_metadata{};
        if (fstat(lock.get(), &lock_metadata) != 0)
            io_error("inspect daemon lock", paths.lock());
        if (!S_ISREG(lock_metadata.st_mode) || lock_metadata.st_uid != required_uid || lock_metadata.st_nlink != 1 ||
            (lock_existed && (lock_metadata.st_mode & 0777) != 0600)) {
            throw RuntimeError(RuntimeErrorCode::unsafe_lock, "daemon lock is unsafe");
        }
        if (flock(lock.get(), LOCK_EX | LOCK_NB) != 0) {
            if (errno == EWOULDBLOCK || errno == EAGAIN) {
                throw RuntimeError(RuntimeErrorCode::already_running, "daemon is already running");
            }
            io_error("lock daemon lock", paths.lock());
        }
        const auto lock_identity = identity(lock_metadata);
        struct stat current_lock{};
        if (lstat(paths.lock().c_str(), &current_lock) != 0 || identity(current_lock) != lock_identity) {
            throw RuntimeError(RuntimeErrorCode::unsafe_lock, "daemon lock path changed");
        }
        if (fchmod(lock.get(), 0600) != 0 || ftruncate(lock.get(), 0) != 0) {
            io_error("prepare daemon lock", paths.lock());
        }
        const auto contents = std::format("{}\n", getpid());
        if (write(lock.get(), contents.data(), contents.size()) != static_cast<ssize_t>(contents.size())) {
            io_error("write daemon lock", paths.lock());
        }

        for (const auto *candidate : {&paths.socket(), &paths.native_password_socket()}) {
            struct stat metadata{};
            if (lstat(candidate->c_str(), &metadata) == 0) {
                if (!S_ISSOCK(metadata.st_mode) || metadata.st_uid != required_uid || metadata.st_nlink != 1) {
                    throw RuntimeError(RuntimeErrorCode::unsafe_socket, "runtime socket is unsafe");
                }
                if (unlink(candidate->c_str()) != 0)
                    io_error("remove stale socket", *candidate);
            } else if (errno != ENOENT) {
                io_error("inspect runtime socket", *candidate);
            }
        }
        return RuntimeOwner(std::move(paths), std::move(lock), lock_identity, created_directory);
    }

    FileDescriptor RuntimeOwner::bind_listener() {
        socklen_t length{};
        const auto address = unix_address(paths_.socket(), length);
        FileDescriptor listener(socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0));
        if (!listener)
            io_error("create control socket", paths_.socket());
        if (bind(listener.get(), reinterpret_cast<const sockaddr *>(&address), length) != 0 ||
            listen(listener.get(), 16) != 0) {
            io_error("bind control socket", paths_.socket());
        }
        struct stat metadata{};
        if (lstat(paths_.socket().c_str(), &metadata) != 0 || !S_ISSOCK(metadata.st_mode) ||
            metadata.st_uid != required_client_uid() || metadata.st_nlink != 1) {
            throw RuntimeError(RuntimeErrorCode::unsafe_socket, "bound control socket is unsafe");
        }
        if (chmod(paths_.socket().c_str(), 0600) != 0)
            io_error("set control socket mode", paths_.socket());
        socket_identity_ = identity(metadata);
        return listener;
    }

    FileDescriptor RuntimeOwner::bind_native_password_listener() {
        socklen_t length{};
        const auto address = unix_address(paths_.native_password_socket(), length);
        FileDescriptor listener(socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0));
        if (!listener)
            io_error("create native password socket", paths_.native_password_socket());
        if (bind(listener.get(), reinterpret_cast<const sockaddr *>(&address), length) != 0 ||
            listen(listener.get(), 16) != 0) {
            io_error("bind native password socket", paths_.native_password_socket());
        }
        struct stat metadata{};
        if (lstat(paths_.native_password_socket().c_str(), &metadata) != 0 || !S_ISSOCK(metadata.st_mode) ||
            metadata.st_uid != required_client_uid() || metadata.st_nlink != 1) {
            throw RuntimeError(RuntimeErrorCode::unsafe_socket, "bound native password socket is unsafe");
        }
        if (chmod(paths_.native_password_socket().c_str(), 0600) != 0) {
            io_error("set native password socket mode", paths_.native_password_socket());
        }
        native_password_socket_identity_ = identity(metadata);
        return listener;
    }

    const RuntimePaths &RuntimeOwner::paths() const noexcept { return paths_; }
    std::uint32_t RuntimeOwner::required_client_uid() const noexcept { return paths_.required_daemon_uid(); }

    bool RuntimeOwner::owned_entries_reachable() const noexcept {
        return path_has_identity(paths_.lock(), lock_identity_) &&
               (!socket_identity_ || path_has_identity(paths_.socket(), *socket_identity_)) &&
               (!native_password_socket_identity_ ||
                path_has_identity(paths_.native_password_socket(), *native_password_socket_identity_));
    }

    void RuntimeOwner::cleanup() noexcept {
        if (!active_)
            return;
        if (native_password_socket_identity_) {
            remove_if_same(paths_.native_password_socket(), *native_password_socket_identity_);
        }
        if (socket_identity_)
            remove_if_same(paths_.socket(), *socket_identity_);
        remove_if_same(paths_.lock(), lock_identity_);
        if (created_directory_)
            rmdir(paths_.directory().c_str());
        active_ = false;
    }

    RuntimeOwner::~RuntimeOwner() { cleanup(); }

    std::uint32_t effective_uid() noexcept { return geteuid(); }

    PeerCredentials peer_credentials(int descriptor) {
        struct ucred credentials{};
        socklen_t length = sizeof(credentials);
        if (getsockopt(descriptor, SOL_SOCKET, SO_PEERCRED, &credentials, &length) != 0 ||
            length != sizeof(credentials)) {
            throw RuntimeError(RuntimeErrorCode::io, "cannot read socket peer credentials");
        }
        return {static_cast<std::uint32_t>(std::max(credentials.pid, 0)), credentials.uid, credentials.gid};
    }

} // namespace sart::splash
