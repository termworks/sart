#include "sart/splash/client.hpp"

#include <atomic>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <poll.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <unistd.h>
#include <utility>

namespace sart::splash {
    namespace {

        std::atomic<std::uint64_t> next_id{1};

        FileDescriptor connect_with_timeout(const std::filesystem::path &path, std::chrono::milliseconds timeout) {
            const auto bytes = path.native();
            sockaddr_un address{};
            if (!path.is_absolute() || bytes.empty() || bytes.find('\0') != std::string::npos ||
                bytes.size() >= sizeof(address.sun_path)) {
                throw std::runtime_error("invalid Unix socket path");
            }
            address.sun_family = AF_UNIX;
            std::memcpy(address.sun_path, bytes.data(), bytes.size());
            const auto length = static_cast<socklen_t>(offsetof(sockaddr_un, sun_path) + bytes.size() + 1);
            FileDescriptor descriptor(socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0));
            if (!descriptor)
                throw std::runtime_error(std::string("create socket: ") + std::strerror(errno));
            if (connect(descriptor.get(), reinterpret_cast<const sockaddr *>(&address), length) != 0) {
                if (errno != EINPROGRESS && errno != EAGAIN) {
                    throw std::runtime_error(std::string("connect daemon: ") + std::strerror(errno));
                }
                pollfd event{descriptor.get(), POLLOUT, 0};
                const auto result = poll(&event, 1, static_cast<int>(timeout.count()));
                if (result == 0)
                    throw std::runtime_error("daemon connection timed out");
                if (result < 0)
                    throw std::runtime_error(std::string("poll daemon: ") + std::strerror(errno));
                int socket_error{};
                socklen_t error_length = sizeof(socket_error);
                if (getsockopt(descriptor.get(), SOL_SOCKET, SO_ERROR, &socket_error, &error_length) != 0 ||
                    socket_error != 0) {
                    throw std::runtime_error(std::string("connect daemon: ") + std::strerror(socket_error));
                }
            }
            const auto flags = fcntl(descriptor.get(), F_GETFL);
            if (flags < 0 || fcntl(descriptor.get(), F_SETFL, flags & ~O_NONBLOCK) != 0) {
                throw std::runtime_error("cannot configure daemon socket");
            }
            const timeval value{static_cast<time_t>(timeout.count() / 1000),
                                static_cast<suseconds_t>((timeout.count() % 1000) * 1000)};
            if (setsockopt(descriptor.get(), SOL_SOCKET, SO_RCVTIMEO, &value, sizeof(value)) != 0 ||
                setsockopt(descriptor.get(), SOL_SOCKET, SO_SNDTIMEO, &value, sizeof(value)) != 0) {
                throw std::runtime_error("cannot set daemon socket timeout");
            }
            return descriptor;
        }

    } // namespace

    ClientConfig::ClientConfig(RuntimePaths runtime_paths)
        : runtime(std::move(runtime_paths)), expected_server_uid(runtime.required_daemon_uid()) {}

    std::uint64_t next_request_id() noexcept {
        return (static_cast<std::uint64_t>(getpid()) << 32) | next_id.fetch_add(1, std::memory_order_relaxed);
    }

    Frame send_request(const ClientConfig &config, const Frame &request) {
        auto connection = connect_with_timeout(config.runtime.socket(), config.timeout);
        const auto credentials = peer_credentials(connection.get());
        if (credentials.uid != config.expected_server_uid) {
            throw std::runtime_error("daemon UID does not match expected UID");
        }
        request.write_to_fd(connection.get());
        if (shutdown(connection.get(), SHUT_WR) != 0) {
            throw std::runtime_error(std::string("finish daemon request: ") + std::strerror(errno));
        }
        auto response = Frame::read_exact_message(connection.get());
        if (response.request_id() != request.request_id()) {
            throw std::runtime_error("response request ID mismatch");
        }
        return response;
    }

} // namespace sart::splash
