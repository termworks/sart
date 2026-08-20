#include "bootart/password_native.hpp"

#include "bootart/art.hpp"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <climits>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <poll.h>
#include <signal.h>
#include <stdexcept>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>
#include <utility>

namespace bootart::password {
    namespace {

        constexpr std::array<std::byte, 4> credential_magic{std::byte{'B'}, std::byte{'C'}, std::byte{'R'},
                                                            std::byte{'D'}};
        constexpr std::array<std::byte, 4> request_magic{std::byte{'B'}, std::byte{'N'}, std::byte{'A'},
                                                         std::byte{'P'}};
        constexpr std::size_t credential_header_bytes = 8;
        constexpr std::size_t request_header_bytes = 52;
        constexpr std::uint64_t maximum_request_lifetime_microseconds = 5ULL * 60 * 1'000'000;

        void verify_carrier(int descriptor, std::uint32_t uid) {
            ucred credentials{};
            socklen_t length = sizeof(credentials);
            if (getsockopt(descriptor, SOL_SOCKET, SO_PEERCRED, &credentials, &length) != 0 || credentials.uid != uid) {
                throw std::runtime_error("native credential peer UID mismatch");
            }
            int domain{};
            int type{};
            length = sizeof(domain);
            if (getsockopt(descriptor, SOL_SOCKET, SO_DOMAIN, &domain, &length) != 0 || domain != AF_UNIX) {
                throw std::runtime_error("native credential carrier is not AF_UNIX");
            }
            length = sizeof(type);
            if (getsockopt(descriptor, SOL_SOCKET, SO_TYPE, &type, &length) != 0 || type != SOCK_SEQPACKET) {
                throw std::runtime_error("native credential carrier is not SOCK_SEQPACKET");
            }
        }

        void wait_writable(int descriptor, std::chrono::steady_clock::time_point deadline) {
            while (true) {
                const auto remaining =
                    std::chrono::duration_cast<std::chrono::milliseconds>(deadline - std::chrono::steady_clock::now());
                if (remaining <= std::chrono::milliseconds(0))
                    throw std::runtime_error("credential transfer timed out");
                pollfd event{descriptor, POLLOUT, 0};
                const auto result =
                    poll(&event, 1, static_cast<int>(std::min<std::int64_t>(remaining.count(), INT_MAX)));
                if (result > 0)
                    return;
                if (result == 0)
                    throw std::runtime_error("credential transfer timed out");
                if (errno != EINTR)
                    throw std::system_error(errno, std::generic_category(), "poll credential carrier");
            }
        }

        template <typename Integer> void store_be(std::span<std::byte> output, Integer value) {
            for (std::size_t index = 0; index < sizeof(Integer); ++index) {
                output[sizeof(Integer) - 1 - index] = std::byte(static_cast<unsigned char>(value >> (index * 8)));
            }
        }

        template <typename Integer> Integer load_be(std::span<const std::byte> input) {
            Integer value{};
            for (const auto byte : input.first(sizeof(Integer))) {
                value = static_cast<Integer>((value << 8) | std::to_integer<unsigned>(byte));
            }
            return value;
        }

        bool safe_prompt(std::string_view prompt) {
            if (prompt.empty() || prompt.size() > 1024)
                return false;
            try {
                for (const auto character : decode_utf8(prompt)) {
                    if (character < 0x20 || (character >= 0x7f && character <= 0x9f) || character == 0x061c ||
                        character == 0x200e || character == 0x200f || (character >= 0x202a && character <= 0x202e) ||
                        (character >= 0x2066 && character <= 0x2069))
                        return false;
                }
                return true;
            } catch (...) {
                return false;
            }
        }

        std::uint64_t native_monotonic_microseconds() {
            timespec value{};
            if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) {
                throw std::system_error(errno, std::generic_category(), "read monotonic clock");
            }
            return static_cast<std::uint64_t>(value.tv_sec) * 1'000'000 +
                   static_cast<std::uint64_t>(value.tv_nsec / 1000);
        }

        NativeAdapter adapter_from_byte(std::uint8_t value) {
            if (value < 1 || value > 5)
                throw std::runtime_error("invalid native adapter");
            return static_cast<NativeAdapter>(value);
        }

        std::atomic<std::uint64_t> next_generation{1};
        std::atomic<bool> output_claimed{};

        splash::FileDescriptor connect_seqpacket(const std::filesystem::path &path,
                                                 std::chrono::steady_clock::time_point deadline) {
            sockaddr_un address{};
            const auto value = path.native();
            if (!path.is_absolute() || value.empty() || value.size() >= sizeof(address.sun_path)) {
                throw std::invalid_argument("invalid native password socket path");
            }
            address.sun_family = AF_UNIX;
            std::memcpy(address.sun_path, value.data(), value.size());
            splash::FileDescriptor descriptor(socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0));
            if (!descriptor)
                throw std::system_error(errno, std::generic_category(), "create native carrier");
            const auto length = static_cast<socklen_t>(offsetof(sockaddr_un, sun_path) + value.size() + 1);
            if (connect(descriptor.get(), reinterpret_cast<const sockaddr *>(&address), length) != 0 &&
                errno != EINPROGRESS && errno != EAGAIN) {
                throw std::system_error(errno, std::generic_category(), "connect native carrier");
            }
            pollfd event{descriptor.get(), POLLOUT, 0};
            const auto remaining =
                std::chrono::duration_cast<std::chrono::milliseconds>(deadline - std::chrono::steady_clock::now());
            if (remaining <= std::chrono::milliseconds(0) ||
                poll(&event, 1, static_cast<int>(std::min<std::int64_t>(remaining.count(), INT_MAX))) <= 0) {
                throw std::runtime_error("connect native carrier timed out");
            }
            int error{};
            socklen_t error_length = sizeof(error);
            if (getsockopt(descriptor.get(), SOL_SOCKET, SO_ERROR, &error, &error_length) != 0 || error != 0) {
                if (error != 0)
                    errno = error;
                throw std::system_error(errno, std::generic_category(), "connect native carrier");
            }
            return descriptor;
        }

        void validate_output_pipe(int descriptor, std::size_t maximum_write) {
            if (descriptor < 3)
                throw std::runtime_error("native secret output must not be a standard descriptor");
            struct stat metadata{};
            if (fstat(descriptor, &metadata) != 0 || !S_ISFIFO(metadata.st_mode)) {
                throw std::runtime_error("native secret output is not a pipe");
            }
            const auto flags = fcntl(descriptor, F_GETFL);
            if (flags < 0 || (flags & O_ACCMODE) != O_WRONLY)
                throw std::runtime_error("native secret pipe is not writable");
            if (fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) != 0 || fcntl(descriptor, F_SETFD, FD_CLOEXEC) != 0) {
                throw std::system_error(errno, std::generic_category(), "secure native secret pipe");
            }
            const auto atomic_limit = fpathconf(descriptor, _PC_PIPE_BUF);
            if (atomic_limit <= 0 || maximum_write > static_cast<std::size_t>(atomic_limit)) {
                throw std::runtime_error("native credential exceeds atomic pipe write limit");
            }
        }

        void write_secret_pipe(int descriptor, SecureSecret &secret, PipeSecretFraming framing) {
            class BlockedSigpipe {
              public:
                BlockedSigpipe() {
                    sigemptyset(&set_);
                    sigaddset(&set_, SIGPIPE);
                    const auto result = pthread_sigmask(SIG_BLOCK, &set_, &previous_);
                    if (result != 0) {
                        throw std::runtime_error("cannot block SIGPIPE for native secret delivery");
                    }
                    sigset_t pending{};
                    if (sigpending(&pending) != 0) {
                        static_cast<void>(pthread_sigmask(SIG_SETMASK, &previous_, nullptr));
                        throw std::runtime_error("cannot inspect pending SIGPIPE state");
                    }
                    was_pending_ = sigismember(&pending, SIGPIPE) == 1;
                }

                ~BlockedSigpipe() {
                    if (restore_)
                        static_cast<void>(pthread_sigmask(SIG_SETMASK, &previous_, nullptr));
                }

                void consume_generated() {
                    if (was_pending_)
                        return;
                    const timespec timeout{};
                    for (std::size_t attempt = 0; attempt < 16; ++attempt) {
                        const auto result = sigtimedwait(&set_, nullptr, &timeout);
                        if (result == SIGPIPE) {
                            if (!pending())
                                return;
                            continue;
                        }
                        if (result < 0 && errno == EINTR)
                            continue;
                        if (result < 0 && errno == EAGAIN && !pending())
                            return;
                        restore_ = false;
                        throw std::runtime_error("cannot consume generated SIGPIPE");
                    }
                    if (pending()) {
                        restore_ = false;
                        throw std::runtime_error("SIGPIPE remained pending after native secret delivery");
                    }
                }

              private:
                bool pending() {
                    sigset_t pending_set{};
                    if (sigpending(&pending_set) != 0) {
                        restore_ = false;
                        throw std::runtime_error("cannot inspect pending SIGPIPE state");
                    }
                    const auto member = sigismember(&pending_set, SIGPIPE);
                    if (member < 0) {
                        restore_ = false;
                        throw std::runtime_error("cannot inspect pending SIGPIPE membership");
                    }
                    return member == 1;
                }

                sigset_t set_{};
                sigset_t previous_{};
                bool was_pending_{};
                bool restore_{true};
            } signal_guard;
            try {
                secret.expose([&](std::span<const std::byte> bytes) {
                    const std::array<std::byte, 1> newline{std::byte{'\n'}};
                    std::array<iovec, 2> vectors{{{const_cast<std::byte *>(bytes.data()), bytes.size()},
                                                  {const_cast<std::byte *>(newline.data()), newline.size()}}};
                    const auto count = framing == PipeSecretFraming::newline_terminated ? 2 : 1;
                    const auto expected = bytes.size() + (count == 2 ? 1 : 0);
                    while (true) {
                        const auto written = writev(descriptor, vectors.data(), count);
                        if (written < 0 && errno == EINTR)
                            continue;
                        if (written < 0 && errno == EPIPE)
                            signal_guard.consume_generated();
                        if (written < 0 || static_cast<std::size_t>(written) != expected) {
                            throw std::runtime_error("native secret pipe write failed");
                        }
                        break;
                    }
                });
            } catch (...) {
                secret.clear();
                throw;
            }
            secret.clear();
        }

    } // namespace

    PipeSecretFraming secret_framing(NativeAdapter adapter) noexcept {
        return adapter == NativeAdapter::initramfs_tools_busybox || adapter == NativeAdapter::mkinitcpio_busybox
                   ? PipeSecretFraming::exact
                   : PipeSecretFraming::newline_terminated;
    }

    std::string_view prompt_source(NativeAdapter adapter) noexcept {
        switch (adapter) {
        case NativeAdapter::dracut_classic:
            return "dracut-classic-native";
        case NativeAdapter::initramfs_tools_busybox:
            return "initramfs-tools-busybox-native";
        case NativeAdapter::mkinitfs_busybox:
            return "mkinitfs-busybox-native";
        case NativeAdapter::mkinitfs_boot_deploy:
            return "mkinitfs-boot-deploy-native";
        case NativeAdapter::mkinitcpio_busybox:
            return "mkinitcpio-busybox-native";
        }
        return "native";
    }

    NativeCredentialClient::NativeCredentialClient(splash::FileDescriptor descriptor)
        : descriptor_(std::move(descriptor)) {}
    int NativeCredentialClient::descriptor() const noexcept { return descriptor_.get(); }

    std::optional<SecureSecret> NativeCredentialClient::receive(std::size_t maximum) {
        SecureSecret secret(maximum);
        std::array<std::byte, credential_header_bytes> header{};
        auto storage = secret.receive_buffer();
        std::array<iovec, 2> vectors{{{header.data(), header.size()}, {storage.data(), storage.size()}}};
        msghdr message{};
        message.msg_iov = vectors.data();
        message.msg_iovlen = vectors.size();
        const auto count = recvmsg(descriptor_.get(), &message, MSG_DONTWAIT);
        if (count < 0)
            throw std::system_error(errno, std::generic_category(), "receive private credential");
        if ((message.msg_flags & (MSG_TRUNC | MSG_CTRUNC)) != 0 || count < static_cast<ssize_t>(header.size()) ||
            !std::equal(credential_magic.begin(), credential_magic.end(), header.begin()) ||
            header[4] != std::byte{1}) {
            throw std::runtime_error("invalid private credential packet");
        }
        const auto payload = static_cast<std::size_t>(count) - header.size();
        if (load_be<std::uint16_t>(std::span(header).subspan(6, 2)) != payload) {
            throw std::runtime_error("private credential length mismatch");
        }
        if (header[5] == std::byte{2} && payload == 0)
            return std::nullopt;
        if (header[5] != std::byte{1})
            throw std::runtime_error("invalid private credential outcome");
        secret.commit_received(payload);
        return secret;
    }

    NativeCredentialResponder::NativeCredentialResponder(splash::FileDescriptor descriptor)
        : descriptor_(std::move(descriptor)) {}
    int NativeCredentialResponder::descriptor() const noexcept { return descriptor_.get(); }

    void NativeCredentialResponder::reply_secret(SecureSecret &secret) {
        if (secret.size() > UINT16_MAX) {
            secret.clear();
            throw std::runtime_error("credential too large");
        }
        std::array<std::byte, 8> header{credential_magic[0], credential_magic[1], credential_magic[2],
                                        credential_magic[3], std::byte{1},        std::byte{1}};
        store_be<std::uint16_t>(std::span(header).subspan(6), static_cast<std::uint16_t>(secret.size()));
        try {
            secret.expose([&](auto bytes) { send_packet(header, bytes); });
        } catch (...) {
            secret.clear();
            throw;
        }
        secret.clear();
    }

    void NativeCredentialResponder::reply_cancel() {
        const std::array<std::byte, 8> header{credential_magic[0], credential_magic[1], credential_magic[2],
                                              credential_magic[3], std::byte{1},        std::byte{2},
                                              std::byte{},         std::byte{}};
        send_packet(header);
    }

    void NativeCredentialResponder::send_packet(std::span<const std::byte> header, std::span<const std::byte> payload) {
        std::array<iovec, 2> vectors{{{const_cast<std::byte *>(header.data()), header.size()},
                                      {const_cast<std::byte *>(payload.data()), payload.size()}}};
        msghdr message{};
        message.msg_iov = vectors.data();
        message.msg_iovlen = payload.empty() ? 1 : 2;
        const auto count = sendmsg(descriptor_.get(), &message, MSG_DONTWAIT | MSG_NOSIGNAL);
        if (count < 0 || static_cast<std::size_t>(count) != header.size() + payload.size()) {
            throw std::runtime_error("send private credential failed");
        }
    }

    NativeCredentialPair native_credential_pair() {
        std::array<int, 2> descriptors{-1, -1};
        if (socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, descriptors.data()) != 0) {
            throw std::system_error(errno, std::generic_category(), "create credential socketpair");
        }
        return {NativeCredentialClient(splash::FileDescriptor(descriptors[0])),
                NativeCredentialResponder(splash::FileDescriptor(descriptors[1]))};
    }

    void send_responder_packet(int carrier, std::uint32_t uid, std::span<const std::byte> metadata,
                               NativeCredentialResponder responder, std::chrono::milliseconds timeout) {
        verify_carrier(carrier, uid);
        if (metadata.empty() || metadata.size() > maximum_responder_metadata_bytes) {
            throw std::invalid_argument("invalid responder metadata size");
        }
        std::array<std::byte, CMSG_SPACE(sizeof(int))> control{};
        iovec vector{const_cast<std::byte *>(metadata.data()), metadata.size()};
        msghdr message{};
        message.msg_iov = &vector;
        message.msg_iovlen = 1;
        message.msg_control = control.data();
        message.msg_controllen = control.size();
        auto *header = CMSG_FIRSTHDR(&message);
        header->cmsg_level = SOL_SOCKET;
        header->cmsg_type = SCM_RIGHTS;
        header->cmsg_len = CMSG_LEN(sizeof(int));
        const auto responder_descriptor = responder.descriptor();
        std::memcpy(CMSG_DATA(header), &responder_descriptor, sizeof(responder_descriptor));
        const auto deadline = std::chrono::steady_clock::now() + timeout;
        while (true) {
            const auto count = sendmsg(carrier, &message, MSG_DONTWAIT | MSG_NOSIGNAL);
            if (count >= 0) {
                if (static_cast<std::size_t>(count) != metadata.size())
                    throw std::runtime_error("short responder transfer");
                return;
            }
            if (errno == EINTR)
                continue;
            if (errno != EAGAIN && errno != EWOULDBLOCK)
                throw std::system_error(errno, std::generic_category());
            wait_writable(carrier, deadline);
        }
    }

    ReceivedResponder receive_responder_packet(int carrier, std::uint32_t uid, std::span<std::byte> metadata) {
        verify_carrier(carrier, uid);
        if (metadata.empty() || metadata.size() > maximum_responder_metadata_bytes) {
            throw std::invalid_argument("invalid responder metadata buffer");
        }
        std::array<std::byte, CMSG_SPACE(sizeof(int))> control{};
        iovec vector{metadata.data(), metadata.size()};
        msghdr message{};
        message.msg_iov = &vector;
        message.msg_iovlen = 1;
        message.msg_control = control.data();
        message.msg_controllen = control.size();
        const auto count = recvmsg(carrier, &message, MSG_DONTWAIT | MSG_CMSG_CLOEXEC);
        if (count < 0)
            throw std::system_error(errno, std::generic_category(), "receive responder transfer");
        if (count == 0 || (message.msg_flags & (MSG_TRUNC | MSG_CTRUNC)) != 0)
            throw std::runtime_error("invalid responder transfer");
        int received = -1;
        std::size_t descriptors{};
        for (auto *header = CMSG_FIRSTHDR(&message); header; header = CMSG_NXTHDR(&message, header)) {
            if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS ||
                header->cmsg_len != CMSG_LEN(sizeof(int)))
                throw std::runtime_error("unexpected responder ancillary data");
            std::memcpy(&received, CMSG_DATA(header), sizeof(received));
            ++descriptors;
        }
        if (descriptors != 1 || received < 0)
            throw std::runtime_error("responder transfer requires exactly one descriptor");
        splash::FileDescriptor descriptor(received);
        verify_carrier(descriptor.get(), uid);
        return {static_cast<std::size_t>(count), NativeCredentialResponder(std::move(descriptor))};
    }

    std::vector<std::byte> encode_native_request(const NativeRequestMetadata &metadata) {
        if (!safe_prompt(metadata.prompt) || metadata.attempts == 0 || metadata.attempts > 64 ||
            metadata.attempt == 0 || metadata.attempt > metadata.attempts || metadata.maximum_secret_bytes == 0 ||
            metadata.maximum_secret_bytes > maximum_secret_bytes || metadata.identity.request_id == 0 ||
            metadata.identity.generation == 0 || metadata.identity.requester_pid == 0 ||
            metadata.identity.requester_start_ticks == 0 || (metadata.echo && metadata.silent))
            throw std::invalid_argument("invalid native request metadata");
        std::vector<std::byte> packet(request_header_bytes + metadata.prompt.size());
        std::copy(request_magic.begin(), request_magic.end(), packet.begin());
        packet[4] = std::byte{1};
        packet[5] = std::byte{1};
        packet[6] = std::byte(static_cast<std::uint8_t>(metadata.adapter));
        packet[7] = std::byte((metadata.echo ? 1 : 0) | (metadata.silent ? 2 : 0));
        store_be(std::span(packet).subspan(8, 8), metadata.identity.request_id);
        store_be(std::span(packet).subspan(16, 8), metadata.identity.generation);
        store_be(std::span(packet).subspan(24, 4), metadata.identity.requester_pid);
        store_be(std::span(packet).subspan(28, 8), metadata.identity.requester_start_ticks);
        store_be(std::span(packet).subspan(36, 8), metadata.deadline_microseconds);
        store_be(std::span(packet).subspan(44, 2), metadata.attempt);
        store_be(std::span(packet).subspan(46, 2), metadata.attempts);
        store_be<std::uint16_t>(std::span(packet).subspan(48, 2),
                                static_cast<std::uint16_t>(metadata.maximum_secret_bytes));
        store_be<std::uint16_t>(std::span(packet).subspan(50, 2), static_cast<std::uint16_t>(metadata.prompt.size()));
        std::memcpy(packet.data() + request_header_bytes, metadata.prompt.data(), metadata.prompt.size());
        return packet;
    }

    NativeRequestMetadata decode_native_request(std::span<const std::byte> packet, std::uint64_t now) {
        if (packet.size() < request_header_bytes ||
            !std::equal(request_magic.begin(), request_magic.end(), packet.begin()) || packet[4] != std::byte{1} ||
            packet[5] != std::byte{1} || (std::to_integer<unsigned>(packet[7]) & ~3U) != 0 ||
            packet[7] == std::byte{3}) {
            throw std::runtime_error("invalid native request packet");
        }
        const auto prompt_length = load_be<std::uint16_t>(packet.subspan(50, 2));
        if (prompt_length == 0 || prompt_length > 1024 || packet.size() != request_header_bytes + prompt_length)
            throw std::runtime_error("invalid native prompt length");
        NativeRequestMetadata metadata;
        metadata.adapter = adapter_from_byte(std::to_integer<std::uint8_t>(packet[6]));
        metadata.identity = {
            load_be<std::uint64_t>(packet.subspan(8, 8)), load_be<std::uint64_t>(packet.subspan(16, 8)),
            load_be<std::uint32_t>(packet.subspan(24, 4)), load_be<std::uint64_t>(packet.subspan(28, 8))};
        metadata.deadline_microseconds = load_be<std::uint64_t>(packet.subspan(36, 8));
        metadata.attempt = load_be<std::uint16_t>(packet.subspan(44, 2));
        metadata.attempts = load_be<std::uint16_t>(packet.subspan(46, 2));
        metadata.maximum_secret_bytes = load_be<std::uint16_t>(packet.subspan(48, 2));
        metadata.prompt =
            std::string(reinterpret_cast<const char *>(packet.data() + request_header_bytes), prompt_length);
        metadata.echo = (std::to_integer<unsigned>(packet[7]) & 1) != 0;
        metadata.silent = (std::to_integer<unsigned>(packet[7]) & 2) != 0;
        if (metadata.deadline_microseconds <= now ||
            metadata.deadline_microseconds - now > maximum_request_lifetime_microseconds)
            throw std::runtime_error("invalid native request deadline");
        static_cast<void>(encode_native_request(metadata));
        return metadata;
    }

    std::uint64_t process_start_ticks(std::uint32_t pid) {
        std::ifstream input("/proc/" + std::to_string(pid) + "/stat");
        std::string record;
        std::getline(input, record);
        if (!input && record.empty())
            throw std::runtime_error("cannot read requester process identity");
        const auto close = record.rfind(')');
        if (close == std::string::npos)
            throw std::runtime_error("invalid process stat");
        auto remainder = std::string_view(record).substr(close + 1);
        for (std::size_t field = 0; field <= 19; ++field) {
            const auto begin = remainder.find_first_not_of(' ');
            if (begin == std::string_view::npos)
                throw std::runtime_error("short process stat");
            remainder.remove_prefix(begin);
            const auto end = remainder.find(' ');
            const auto token = remainder.substr(0, end);
            if (field == 19)
                return std::stoull(std::string(token));
            remainder = end == std::string_view::npos ? std::string_view{} : remainder.substr(end + 1);
        }
        throw std::runtime_error("short process stat");
    }

    splash::FileDescriptor claim_native_askpass_output() {
        if (fcntl(native_askpass_output_fd, F_GETFD) < 0) {
            throw std::system_error(errno, std::generic_category(), "claim native askpass output");
        }
        bool expected{};
        if (!output_claimed.compare_exchange_strong(expected, true)) {
            throw std::runtime_error("native askpass output was already claimed");
        }
        return splash::FileDescriptor(native_askpass_output_fd);
    }

    NativeAskpassOutcome run_native_askpass_client(NativeAdapter adapter, const NativeAskpassMetadata &metadata,
                                                   splash::FileDescriptor output,
                                                   const std::filesystem::path &socket_path,
                                                   std::uint32_t expected_daemon_uid,
                                                   std::chrono::milliseconds timeout) noexcept {
        try {
            if (!safe_prompt(metadata.prompt) || metadata.attempts == 0 || metadata.attempts > 64 ||
                metadata.maximum_secret_bytes == 0 || metadata.maximum_secret_bytes > maximum_secret_bytes ||
                timeout <= std::chrono::milliseconds(0) || timeout > std::chrono::minutes(5)) {
                throw std::invalid_argument("invalid native askpass metadata");
            }
            protect_process_secrets();
            validate_output_pipe(output.get(),
                                 metadata.maximum_secret_bytes +
                                     (secret_framing(adapter) == PipeSecretFraming::newline_terminated ? 1 : 0));
            const auto now = native_monotonic_microseconds();
            const auto deadline = std::chrono::steady_clock::now() + timeout;
            const auto pid = static_cast<std::uint32_t>(getpid());
            const auto generation = std::max<std::uint64_t>(1, next_generation.fetch_add(1));
            NativeRequestMetadata request{
                adapter,
                {std::max<std::uint64_t>(1, now ^ std::rotl<std::uint64_t>(pid, 17) ^ std::rotl(generation, 37)),
                 generation, pid, process_start_ticks(pid)},
                now + static_cast<std::uint64_t>(timeout.count()) * 1000,
                1,
                metadata.attempts,
                metadata.maximum_secret_bytes,
                metadata.prompt,
                false,
                false};
            auto carrier = connect_seqpacket(socket_path, deadline);
            const auto peer = splash::peer_credentials(carrier.get());
            if (peer.uid != expected_daemon_uid)
                throw std::runtime_error("native daemon UID mismatch");
            auto pair = native_credential_pair();
            const auto packet = encode_native_request(request);
            send_responder_packet(
                carrier.get(), expected_daemon_uid, packet, std::move(pair.responder),
                std::chrono::duration_cast<std::chrono::milliseconds>(deadline - std::chrono::steady_clock::now()));
            carrier = splash::FileDescriptor();
            while (true) {
                const auto remaining =
                    std::chrono::duration_cast<std::chrono::milliseconds>(deadline - std::chrono::steady_clock::now());
                if (remaining <= std::chrono::milliseconds(0))
                    throw std::runtime_error("native askpass timed out");
                std::array<pollfd, 2> events{{{pair.client.descriptor(), POLLIN, 0}, {output.get(), 0, 0}}};
                const auto ready = poll(events.data(), events.size(),
                                        static_cast<int>(std::min<std::int64_t>(remaining.count(), INT_MAX)));
                if (ready < 0 && errno == EINTR)
                    continue;
                if (ready <= 0)
                    throw std::runtime_error("native askpass timed out");
                if ((events[1].revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
                    throw std::runtime_error("native askpass pipe consumer disappeared");
                }
                if ((events[0].revents & POLLIN) != 0) {
                    auto secret = pair.client.receive(metadata.maximum_secret_bytes);
                    if (!secret)
                        return NativeAskpassOutcome::user_cancelled;
                    write_secret_pipe(output.get(), *secret, secret_framing(adapter));
                    return NativeAskpassOutcome::delivered;
                }
                if ((events[0].revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
                    throw std::runtime_error("native credential channel closed");
                }
            }
        } catch (...) {
            return NativeAskpassOutcome::console_fallback;
        }
    }

} // namespace bootart::password
