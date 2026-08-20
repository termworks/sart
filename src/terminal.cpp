#include "sart/terminal.hpp"

#include "sart/signals.hpp"

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <poll.h>
#include <stdexcept>
#include <sys/ioctl.h>
#include <system_error>
#include <unistd.h>

namespace sart {
    namespace {

        constexpr std::string_view hide_cursor = "\x1b[?25l";
        constexpr std::string_view show_cursor = "\x1b[?25h";
        constexpr std::string_view reset_and_show_cursor = "\x1b[0m\x1b[?25h";

        std::optional<bool> cursor_state_after(std::string_view bytes) {
            std::optional<bool> needs_restore;
            for (std::size_t offset = 0; offset + hide_cursor.size() <= bytes.size(); ++offset) {
                const auto window = bytes.substr(offset, hide_cursor.size());
                if (window == hide_cursor) {
                    needs_restore = true;
                } else if (window == show_cursor) {
                    needs_restore = false;
                }
            }
            return needs_restore;
        }

        [[noreturn]] void throw_errno(std::string_view operation) {
            throw std::system_error(errno, std::generic_category(), std::string(operation));
        }

    } // namespace

    StdoutTerminal::StdoutTerminal() = default;

    StdoutTerminal::StdoutTerminal(std::optional<std::size_t> columns, std::optional<std::size_t> rows) {
        if (columns && rows && *columns > 0 && *rows > 0) {
            override_size_ = TerminalSize{*columns, *rows};
        }
    }

    StdoutTerminal::~StdoutTerminal() {
        if (needs_restore_) {
            try {
                restore_terminal();
            } catch (...) {
            }
        }
        if (output_ >= 0) {
            close(output_);
        }
    }

    TerminalSize StdoutTerminal::dimensions() const {
        if (override_size_) {
            return *override_size_;
        }
        struct winsize size{};
        if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &size) == 0 && size.ws_col > 0 && size.ws_row > 0) {
            return {size.ws_col, size.ws_row};
        }
        return {};
    }

    int StdoutTerminal::output_descriptor() {
        if (output_ >= 0) {
            return output_;
        }
        const auto flags = fcntl(STDOUT_FILENO, F_GETFL);
        if (flags < 0) {
            throw_errno("inspect stdout");
        }
        if ((flags & O_ACCMODE) == O_RDONLY) {
            throw std::system_error(EACCES, std::generic_category(), "stdout is not writable");
        }
        output_ = open(std::format("/proc/self/fd/{}", STDOUT_FILENO).c_str(),
                       O_WRONLY | O_CLOEXEC | O_NONBLOCK | O_NOCTTY | (flags & O_APPEND));
        if (output_ < 0) {
            throw_errno("reopen stdout");
        }
        const auto offset = lseek(STDOUT_FILENO, 0, SEEK_CUR);
        if (offset >= 0 && lseek(output_, offset, SEEK_SET) < 0) {
            const auto saved = errno;
            close(output_);
            output_ = -1;
            errno = saved;
            throw_errno("position stdout");
        }
        return output_;
    }

    void StdoutTerminal::write_bounded(std::string_view bytes, int timeout_milliseconds, bool stop_aware) {
        const auto descriptor = output_descriptor();
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeout_milliseconds);
        while (!bytes.empty()) {
            if (std::chrono::steady_clock::now() >= deadline) {
                throw std::system_error(ETIMEDOUT, std::generic_category(), "terminal write deadline expired");
            }
            if (stop_aware && signals::should_stop()) {
                throw std::system_error(EINTR, std::generic_category(), "terminal write interrupted");
            }
            const auto written = write(descriptor, bytes.data(), bytes.size());
            if (written > 0) {
                bytes.remove_prefix(static_cast<std::size_t>(written));
                continue;
            }
            if (written == 0) {
                throw std::system_error(EIO, std::generic_category(), "terminal write returned zero");
            }
            if (errno == EINTR) {
                continue;
            }
            if (errno != EAGAIN && errno != EWOULDBLOCK) {
                throw_errno("write terminal");
            }
            const auto now = std::chrono::steady_clock::now();
            const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(deadline - now);
            const auto wait = static_cast<int>(std::clamp<std::int64_t>(remaining.count(), 1, 25));
            struct pollfd event{descriptor, POLLOUT, 0};
            const auto ready = poll(&event, 1, wait);
            if (ready < 0 && errno != EINTR) {
                throw_errno("poll terminal");
            }
            if (ready > 0 && (event.revents & POLLNVAL) != 0) {
                throw std::system_error(EBADF, std::generic_category(), "invalid terminal descriptor");
            }
            if (ready > 0 && (event.revents & (POLLERR | POLLHUP)) != 0 && (event.revents & POLLOUT) == 0) {
                throw std::system_error(EPIPE, std::generic_category(), "terminal output closed");
            }
        }
    }

    void StdoutTerminal::restore_terminal() {
        write_bounded(reset_and_show_cursor, 100, false);
        needs_restore_ = false;
    }

    void StdoutTerminal::write_frame(std::string_view bytes) {
        if (bytes.find(hide_cursor) != std::string_view::npos) {
            needs_restore_ = true;
        }
        const auto final_state = cursor_state_after(bytes);
        const auto restoration = final_state && !*final_state;
        try {
            write_bounded(bytes, restoration ? 100 : 250, !restoration);
            if (final_state) {
                needs_restore_ = *final_state;
            }
        } catch (const std::system_error &error) {
            if (error.code().value() == EINTR && signals::should_stop()) {
                if (needs_restore_) {
                    restore_terminal();
                }
                return;
            }
            if (needs_restore_) {
                try {
                    restore_terminal();
                } catch (...) {
                }
            }
            throw;
        }
    }

    void StdoutTerminal::flush() {}

    BufferTerminal::BufferTerminal(TerminalSize size) : size_(size) {}

    TerminalSize BufferTerminal::dimensions() const { return size_; }

    void BufferTerminal::write_frame(std::string_view bytes) { buffer_.append(bytes); }

    void BufferTerminal::flush() {}

    const std::string &BufferTerminal::contents() const noexcept { return buffer_; }

} // namespace sart
