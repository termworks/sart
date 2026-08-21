#include "sart/password/systemd.hpp"

#include "sart/visual/art.hpp"

#include <algorithm>
#include <cerrno>
#include <charconv>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <fstream>
#include <limits>
#include <map>
#include <signal.h>
#include <stdexcept>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include <utility>

namespace sart::password {
    namespace {

        std::string_view trim(std::string_view value) {
            const auto begin = value.find_first_not_of(" \t\r\n");
            if (begin == std::string_view::npos)
                return {};
            const auto end = value.find_last_not_of(" \t\r\n");
            return value.substr(begin, end - begin + 1);
        }

        bool valid_request_name(std::string_view name) {
            if (!name.starts_with("ask.") || name.size() <= 4 || name.size() > 255)
                return false;
            return std::all_of(name.begin() + 4, name.end(), [](unsigned char byte) {
                return std::isalnum(byte) || byte == '.' || byte == '_' || byte == '-';
            });
        }

        template <typename Integer>
        Integer parse_integer(std::string_view value, std::string_view field, std::size_t line) {
            Integer result{};
            const auto [end, error] = std::from_chars(value.data(), value.data() + value.size(), result);
            if (error != std::errc{} || end != value.data() + value.size()) {
                throw std::invalid_argument(std::format("invalid [Ask] field {} on line {}", field, line));
            }
            return result;
        }

        bool parse_boolean(std::string_view value, std::size_t line) {
            std::string lower(value);
            std::ranges::transform(lower, lower.begin(),
                                   [](unsigned char byte) { return static_cast<char>(std::tolower(byte)); });
            if (lower == "1" || lower == "yes" || lower == "true" || lower == "on")
                return true;
            if (lower == "0" || lower == "no" || lower == "false" || lower == "off")
                return false;
            throw std::invalid_argument(std::format("invalid [Ask] boolean on line {}", line));
        }

        void validate_message(std::string_view message) {
            if (message.empty() || message.size() > maximum_ask_message_bytes) {
                throw std::invalid_argument("unsafe ask-password message");
            }
            for (const auto character : decode_utf8(message)) {
                if (character < 0x20 || (character >= 0x7f && character <= 0x9f) || character == 0x061c ||
                    character == 0x200e || character == 0x200f || (character >= 0x202a && character <= 0x202e) ||
                    (character >= 0x2066 && character <= 0x2069)) {
                    throw std::invalid_argument("unsafe ask-password message");
                }
            }
        }

        void validate_socket_path(const std::filesystem::path &path) {
            const auto value = path.native();
            sockaddr_un address{};
            if (!path.is_absolute() || value.empty() || value.size() >= sizeof(address.sun_path) ||
                value.find('\0') != std::string::npos) {
                throw std::invalid_argument("unsafe ask-password socket path");
            }
            for (const auto &part : path) {
                if (part == "..")
                    throw std::invalid_argument("unsafe ask-password socket path");
            }
        }

        void validate_owned_directory(const std::filesystem::path &path, std::uint32_t uid) {
            struct stat metadata{};
            if (lstat(path.c_str(), &metadata) != 0 || !S_ISDIR(metadata.st_mode) || metadata.st_uid != uid ||
                (metadata.st_mode & 0022) != 0) {
                throw std::runtime_error("unsafe ask-password directory: " + path.string());
            }
        }

        sockaddr_un socket_address(const std::filesystem::path &path, std::uint32_t uid, socklen_t &length) {
            validate_socket_path(path);
            struct stat metadata{};
            if (lstat(path.c_str(), &metadata) != 0 || !S_ISSOCK(metadata.st_mode) || metadata.st_uid != uid) {
                throw std::runtime_error("unsafe ask-password socket path");
            }
            validate_owned_directory(path.parent_path(), uid);
            sockaddr_un address{};
            address.sun_family = AF_UNIX;
            const auto value = path.native();
            std::memcpy(address.sun_path, value.data(), value.size());
            length = static_cast<socklen_t>(offsetof(sockaddr_un, sun_path) + value.size() + 1);
            return address;
        }

    } // namespace

    AskRequest AskRequest::parse(AskRequestId id, std::string_view contents) {
        if (!valid_request_name(id.name))
            throw std::invalid_argument("invalid systemd ask request name");
        if (contents.size() > maximum_ask_request_bytes)
            throw std::length_error("ask request too large");
        static_cast<void>(decode_utf8(contents));
        std::map<std::string, std::string> fields;
        bool in_ask{};
        std::size_t line_number{};
        while (!contents.empty()) {
            ++line_number;
            const auto separator = contents.find('\n');
            auto line = contents.substr(0, separator);
            if (separator == std::string_view::npos)
                contents = {};
            else
                contents.remove_prefix(separator + 1);
            if (line.ends_with('\r'))
                line.remove_suffix(1);
            const auto cleaned = trim(line);
            if (cleaned.empty() || cleaned.starts_with('#') || cleaned.starts_with(';'))
                continue;
            if (cleaned.starts_with('[') && cleaned.ends_with(']')) {
                in_ask = cleaned == "[Ask]";
                continue;
            }
            if (!in_ask)
                continue;
            const auto equals = line.find('=');
            if (equals == std::string_view::npos) {
                throw std::invalid_argument(std::format("malformed [Ask] line {}", line_number));
            }
            const auto key = std::string(trim(line.substr(0, equals)));
            if (key != "Message" && key != "PID" && key != "Socket" && key != "Echo" && key != "Silent" &&
                key != "AcceptCached" && key != "NotAfter")
                continue;
            if (!fields.emplace(key, std::string(line.substr(equals + 1))).second) {
                throw std::invalid_argument("duplicate [Ask] field " + key);
            }
        }
        AskRequest request;
        request.id_ = std::move(id);
        request.message_ = fields.contains("Message") ? fields.at("Message") : "Password:";
        validate_message(request.message_);
        if (!fields.contains("PID") || !fields.contains("Socket")) {
            throw std::invalid_argument("missing required [Ask] field");
        }
        request.requester_pid_ = parse_integer<std::uint32_t>(trim(fields.at("PID")), "PID", 0);
        if (request.requester_pid_ == 0 || request.requester_pid_ > std::numeric_limits<std::int32_t>::max()) {
            throw std::invalid_argument("invalid [Ask] PID");
        }
        request.socket_ = fields.at("Socket");
        validate_socket_path(request.socket_);
        if (fields.contains("Echo"))
            request.echo_ = parse_boolean(trim(fields.at("Echo")), 0);
        if (fields.contains("Silent"))
            request.silent_ = parse_boolean(trim(fields.at("Silent")), 0);
        if (fields.contains("AcceptCached")) {
            request.accept_cached_requested_ = parse_boolean(trim(fields.at("AcceptCached")), 0);
        }
        if (fields.contains("NotAfter")) {
            request.not_after_microseconds_ = parse_integer<std::uint64_t>(trim(fields.at("NotAfter")), "NotAfter", 0);
        }
        return request;
    }

    const AskRequestId &AskRequest::id() const noexcept { return id_; }
    const std::string &AskRequest::message() const noexcept { return message_; }
    std::uint32_t AskRequest::requester_pid() const noexcept { return requester_pid_; }
    const std::filesystem::path &AskRequest::socket() const noexcept { return socket_; }
    bool AskRequest::echo() const noexcept { return echo_; }
    bool AskRequest::silent() const noexcept { return silent_; }
    bool AskRequest::accept_cached_requested() const noexcept { return accept_cached_requested_; }
    std::uint64_t AskRequest::not_after_microseconds() const noexcept { return not_after_microseconds_; }
    bool AskRequest::expired(std::uint64_t now) const noexcept {
        return not_after_microseconds_ != 0 && not_after_microseconds_ <= now;
    }

    AskScanResult scan_ask_requests(const std::filesystem::path &directory, std::uint32_t uid) {
        validate_owned_directory(directory, uid);
        AskScanResult result;
        std::size_t matches{};
        for (const auto &entry : std::filesystem::directory_iterator(directory)) {
            const auto name = entry.path().filename().string();
            if (!name.starts_with("ask."))
                continue;
            if (++matches > maximum_ask_request_files)
                throw std::runtime_error("too many ask requests");
            try {
                struct stat metadata{};
                if (lstat(entry.path().c_str(), &metadata) != 0 || !S_ISREG(metadata.st_mode) ||
                    metadata.st_uid != uid || metadata.st_nlink != 1 || (metadata.st_mode & 0022) != 0 ||
                    metadata.st_size < 0 || static_cast<std::uint64_t>(metadata.st_size) > maximum_ask_request_bytes) {
                    throw std::runtime_error("unsafe request file");
                }
                const auto descriptor = open(entry.path().c_str(), O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
                if (descriptor < 0)
                    throw std::system_error(errno, std::generic_category(), "open ask request");
                std::string contents;
                std::array<char, 1024> buffer{};
                while (contents.size() <= maximum_ask_request_bytes) {
                    const auto count = read(descriptor, buffer.data(), buffer.size());
                    if (count > 0)
                        contents.append(buffer.data(), static_cast<std::size_t>(count));
                    else if (count == 0)
                        break;
                    else if (errno != EINTR) {
                        close(descriptor);
                        throw std::system_error(errno, std::generic_category());
                    }
                }
                close(descriptor);
                result.requests.push_back(AskRequest::parse(
                    {name, static_cast<std::uint64_t>(metadata.st_dev), static_cast<std::uint64_t>(metadata.st_ino)},
                    contents));
            } catch (const std::exception &error) {
                result.rejected.push_back({name, error.what()});
            }
        }
        std::ranges::sort(result.requests, {}, [](const AskRequest &request) { return request.id(); });
        return result;
    }

    std::uint64_t monotonic_microseconds() {
        timespec value{};
        if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) {
            throw std::system_error(errno, std::generic_category(), "read monotonic clock");
        }
        return static_cast<std::uint64_t>(value.tv_sec) * 1'000'000 + static_cast<std::uint64_t>(value.tv_nsec / 1000);
    }

    bool requester_alive(std::uint32_t pid) {
        if (pid == 0 || pid > std::numeric_limits<std::int32_t>::max())
            return false;
        if (kill(static_cast<pid_t>(pid), 0) == 0)
            return true;
        if (errno == ESRCH)
            return false;
        if (errno == EPERM)
            return true;
        throw std::system_error(errno, std::generic_category(), "check requester liveness");
    }

    SystemdReplySocket::SystemdReplySocket()
        : descriptor_(socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0)) {
        if (descriptor_ < 0)
            throw std::system_error(errno, std::generic_category(), "create reply socket");
    }
    SystemdReplySocket::SystemdReplySocket(SystemdReplySocket &&other) noexcept
        : descriptor_(std::exchange(other.descriptor_, -1)) {}
    SystemdReplySocket &SystemdReplySocket::operator=(SystemdReplySocket &&other) noexcept {
        if (this != &other) {
            if (descriptor_ >= 0)
                close(descriptor_);
            descriptor_ = std::exchange(other.descriptor_, -1);
        }
        return *this;
    }
    SystemdReplySocket::~SystemdReplySocket() {
        if (descriptor_ >= 0)
            close(descriptor_);
    }

    void SystemdReplySocket::send_success(const AskRequest &request, SecureSecret &secret, std::uint32_t uid) {
        try {
            secret.expose([&](std::span<const std::byte> bytes) {
                const std::array<std::byte, 1> prefix{std::byte{'+'}};
                const std::array<std::span<const std::byte>, 2> parts{prefix, bytes};
                send(request, parts, uid);
            });
        } catch (...) {
            secret.clear();
            throw;
        }
        secret.clear();
    }

    void SystemdReplySocket::send_cancel(const AskRequest &request, std::uint32_t uid) {
        const std::array<std::byte, 1> marker{std::byte{'-'}};
        const std::array<std::span<const std::byte>, 1> parts{marker};
        send(request, parts, uid);
    }

    void SystemdReplySocket::send(const AskRequest &request, std::span<const std::span<const std::byte>> parts,
                                  std::uint32_t uid) {
        socklen_t address_length{};
        auto address = socket_address(request.socket(), uid, address_length);
        std::vector<iovec> vectors;
        std::size_t expected{};
        for (const auto part : parts) {
            vectors.push_back({const_cast<std::byte *>(part.data()), part.size()});
            expected += part.size();
        }
        msghdr message{};
        message.msg_name = &address;
        message.msg_namelen = address_length;
        message.msg_iov = vectors.data();
        message.msg_iovlen = vectors.size();
        const auto sent = sendmsg(descriptor_, &message, MSG_NOSIGNAL | MSG_DONTWAIT);
        if (sent < 0)
            throw std::system_error(errno, std::generic_category(), "send ask response");
        if (static_cast<std::size_t>(sent) != expected)
            throw std::runtime_error("short ask response");
    }

} // namespace sart::password
