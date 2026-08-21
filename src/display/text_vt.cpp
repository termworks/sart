#include "sart/display/text_vt.hpp"

#include "sart/core/signals.hpp"
#include "sart/visual/art.hpp"

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <poll.h>
#include <stdexcept>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <thread>
#include <unistd.h>
#include <utility>

namespace sart::display {
    namespace {

        constexpr std::string_view control_device = "/dev/tty0";
        constexpr int kd_text = 0;
        constexpr int k_unicode = 3;
        constexpr auto vt_switch_timeout = std::chrono::milliseconds(500);
        constexpr std::string_view save_cursor = "\x1b\x37";
        constexpr std::string_view hide_cursor_clear = "\x1b[?25l\x1b[0m\x1b[2J\x1b[H";
        constexpr std::string_view restore_screen_state = "\x1b[0m\x1b\x38\x1b[?25h";
        constexpr std::string_view clear_splash = "\x1b[0m\x1b[2J\x1b[H";
        constexpr unsigned long vt_open_query = 0x5600;
        constexpr unsigned long vt_get_state = 0x5603;
        constexpr unsigned long vt_activate = 0x5606;
        constexpr unsigned long vt_disallocate = 0x5608;
        constexpr unsigned long kd_set_mode = 0x4B3A;
        constexpr unsigned long kd_get_mode = 0x4B3B;
        constexpr unsigned long keyboard_get_mode = 0x4B44;
        constexpr unsigned long keyboard_set_mode = 0x4B45;

        struct VtStat {
            std::uint16_t active{};
            std::uint16_t signal{};
            std::uint16_t state{};
        };

        void validate_vt_number(std::uint16_t number) {
            if (number < minimum_linux_vt || number > maximum_linux_vt) {
                throw std::invalid_argument("Linux VT number must be in 1..=63");
            }
        }

        [[noreturn]] void system_error(std::string_view operation) {
            throw std::system_error(errno, std::generic_category(), std::string(operation));
        }

        int open_device(const std::filesystem::path &path) {
            const auto descriptor = open(path.c_str(), O_RDWR | O_CLOEXEC | O_NOCTTY | O_NONBLOCK | O_NOFOLLOW);
            if (descriptor < 0)
                system_error("open Linux VT");
            struct stat metadata{};
            if (fstat(descriptor, &metadata) != 0 || !S_ISCHR(metadata.st_mode)) {
                const auto saved = errno == 0 ? EINVAL : errno;
                close(descriptor);
                errno = saved;
                system_error("validate Linux VT");
            }
            return descriptor;
        }

        void write_bounded(int descriptor, std::string_view bytes, std::chrono::milliseconds timeout, bool stop_aware) {
            const auto deadline = std::chrono::steady_clock::now() + timeout;
            while (!bytes.empty()) {
                if (std::chrono::steady_clock::now() >= deadline) {
                    throw std::system_error(ETIMEDOUT, std::generic_category(), "VT write deadline");
                }
                if (stop_aware && signals::should_stop()) {
                    throw std::system_error(EINTR, std::generic_category(), "VT write interrupted");
                }
                const auto written = write(descriptor, bytes.data(), bytes.size());
                if (written > 0) {
                    bytes.remove_prefix(static_cast<std::size_t>(written));
                    continue;
                }
                if (written < 0 && errno == EINTR)
                    continue;
                if (written < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                    pollfd event{descriptor, POLLOUT, 0};
                    const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
                        deadline - std::chrono::steady_clock::now());
                    const auto wait = static_cast<int>(std::clamp<std::int64_t>(remaining.count(), 1, 25));
                    if (poll(&event, 1, wait) >= 0)
                        continue;
                }
                system_error("write Linux VT");
            }
        }

        int foreground_code(Color color) {
            static constexpr int values[]{39, 30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93, 94, 95, 96, 97};
            return values[static_cast<std::size_t>(color)];
        }

        int background_code(Color color) {
            static constexpr int values[]{49, 40, 41, 42, 43, 44, 45, 46, 47, 100, 101, 102, 103, 104, 105, 106, 107};
            return values[static_cast<std::size_t>(color)];
        }

        void append_style(std::string &output, Style style) {
            output.append("\x1b[0");
            if (style.bold)
                output.append(";1");
            output.append(std::format(";{};{}m", foreground_code(style.foreground), background_code(style.background)));
        }

    } // namespace

    TextVtConfig::TextVtConfig(VtSelection selection) noexcept : selection_(selection) {}
    TextVtConfig TextVtConfig::open_query() noexcept { return TextVtConfig({VtSelectionKind::open_query, 0}); }
    TextVtConfig TextVtConfig::configured(std::uint16_t number) {
        validate_vt_number(number);
        return TextVtConfig({VtSelectionKind::configured, number});
    }
    VtSelection TextVtConfig::selection() const noexcept { return selection_; }

    void VtIo::write_restore(int vt, std::string_view bytes) { write_all(vt, bytes); }

    int LinuxVtIo::open_control(const std::filesystem::path &path) { return open_device(path); }
    std::uint16_t LinuxVtIo::active_vt(int control) {
        VtStat state{};
        if (ioctl(control, vt_get_state, &state) < 0)
            system_error("query active VT");
        validate_vt_number(state.active);
        return state.active;
    }
    std::uint16_t LinuxVtIo::open_query(int control) {
        int number = -1;
        if (ioctl(control, vt_open_query, &number) < 0)
            system_error("query unused VT");
        if (number < 0 || number > maximum_linux_vt)
            throw std::runtime_error("no unused Linux VT");
        validate_vt_number(static_cast<std::uint16_t>(number));
        return static_cast<std::uint16_t>(number);
    }
    int LinuxVtIo::open_vt(const std::filesystem::path &path, std::uint16_t) { return open_device(path); }
    void LinuxVtIo::close_device(int device) noexcept {
        if (device >= 0)
            close(device);
    }
    Dimensions LinuxVtIo::dimensions(int vt) {
        winsize size{};
        if (ioctl(vt, TIOCGWINSZ, &size) < 0)
            system_error("query VT dimensions");
        return Dimensions(size.ws_col, size.ws_row);
    }
    termios LinuxVtIo::terminal_state(int vt) {
        termios state{};
        if (tcgetattr(vt, &state) < 0)
            system_error("capture VT termios");
        return state;
    }
    void LinuxVtIo::set_raw_terminal(int vt, const termios &original) {
        auto raw = original;
        cfmakeraw(&raw);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 0;
        if (tcsetattr(vt, TCSANOW, &raw) < 0)
            system_error("set raw VT termios");
    }
    void LinuxVtIo::restore_terminal(int vt, const termios &original) {
        if (tcsetattr(vt, TCSANOW, &original) < 0)
            system_error("restore VT termios");
    }
    int LinuxVtIo::kd_mode(int vt) {
        int mode = -1;
        if (ioctl(vt, kd_get_mode, &mode) < 0)
            system_error("query KD mode");
        return mode;
    }
    void LinuxVtIo::set_kd_mode(int vt, int mode) {
        if (ioctl(vt, kd_set_mode, mode) < 0)
            system_error("set KD mode");
    }
    int LinuxVtIo::keyboard_mode(int vt) {
        int mode = -1;
        if (ioctl(vt, keyboard_get_mode, &mode) < 0)
            system_error("query keyboard mode");
        return mode;
    }
    void LinuxVtIo::set_keyboard_mode(int vt, int mode) {
        if (ioctl(vt, keyboard_set_mode, mode) < 0)
            system_error("set keyboard mode");
    }
    void LinuxVtIo::activate(int control, std::uint16_t number) {
        if (ioctl(control, vt_activate, static_cast<int>(number)) < 0)
            system_error("activate VT");
    }
    void LinuxVtIo::wait_active(int control, std::uint16_t number, std::chrono::milliseconds timeout) {
        const auto deadline = std::chrono::steady_clock::now() + timeout;
        while (active_vt(control) != number) {
            if (std::chrono::steady_clock::now() >= deadline) {
                throw std::system_error(ETIMEDOUT, std::generic_category(), "wait for VT");
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
    }
    VtDeallocation LinuxVtIo::disallocate(int control, std::uint16_t number) {
        if (ioctl(control, vt_disallocate, static_cast<int>(number)) == 0) {
            return VtDeallocation::deallocated;
        }
        if (errno == EBUSY)
            return VtDeallocation::in_use;
        system_error("deallocate VT");
    }
    void LinuxVtIo::write_all(int vt, std::string_view bytes) {
        write_bounded(vt, bytes, std::chrono::milliseconds(250), true);
    }
    void LinuxVtIo::write_restore(int vt, std::string_view bytes) {
        write_bounded(vt, bytes, std::chrono::milliseconds(100), false);
    }
    void LinuxVtIo::write_sensitive(int vt, std::string_view bytes) {
        write_bounded(vt, bytes, std::chrono::milliseconds(250), true);
    }
    void LinuxVtIo::flush(int) {}
    std::optional<std::vector<std::uint8_t>> LinuxVtIo::poll_read(int vt, std::chrono::milliseconds timeout) {
        pollfd event{vt, POLLIN, 0};
        const auto ready = poll(&event, 1, static_cast<int>(std::min<std::int64_t>(timeout.count(), INT_MAX)));
        if (ready < 0) {
            if (errno == EINTR)
                return std::nullopt;
            system_error("poll VT input");
        }
        if (ready == 0 || (event.revents & POLLIN) == 0)
            return std::nullopt;
        if ((event.revents & (POLLERR | POLLNVAL)) != 0)
            throw std::runtime_error("VT polling error");
        std::vector<std::uint8_t> bytes(256);
        const auto count = read(vt, bytes.data(), bytes.size());
        if (count <= 0) {
            if (count < 0 && errno != EAGAIN && errno != EWOULDBLOCK)
                system_error("read VT input");
            return std::nullopt;
        }
        bytes.resize(static_cast<std::size_t>(count));
        return bytes;
    }

    TextVtBackend::TextVtBackend(TextVtConfig config) : TextVtBackend(config, std::make_unique<LinuxVtIo>()) {}
    TextVtBackend::TextVtBackend(TextVtConfig config, std::unique_ptr<VtIo> io) : config_(config), io_(std::move(io)) {
        if (!io_)
            throw std::invalid_argument("VT I/O implementation is required");
    }
    TextVtBackend::~TextVtBackend() {
        try {
            restore();
        } catch (...) {
        }
    }
    std::optional<std::uint16_t> TextVtBackend::original_vt() const noexcept { return original_vt_; }
    std::optional<std::uint16_t> TextVtBackend::splash_vt() const noexcept { return splash_vt_; }
    DisplayState TextVtBackend::state() const noexcept { return state_; }
    std::optional<Dimensions> TextVtBackend::dimensions() const noexcept { return dimensions_; }

    [[noreturn]] void TextVtBackend::invalid_state(std::string_view operation) const {
        throw DisplayError(DisplayErrorCode::invalid_state, std::format("cannot {} Linux VT display", operation));
    }

    [[noreturn]] void TextVtBackend::fail_open(std::string_view operation, const std::exception &source) {
        try {
            cleanup(DisplayState::failed_open, RestoreMode::clear);
        } catch (const std::exception &restoration) {
            throw DisplayError(DisplayErrorCode::operation_and_restore,
                               std::format("{}: {}; display restoration also failed: {}", operation, source.what(),
                                           restoration.what()),
                               true);
        }
        throw DisplayError(DisplayErrorCode::backend,
                           std::format("linux-text-vt failed to {}: {}", operation, source.what()));
    }

    void TextVtBackend::switch_to(std::uint16_t number, std::string_view operation) {
        if (control_ < 0)
            invalid_state(operation);
        try {
            io_->activate(control_, number);
            io_->wait_active(control_, number, vt_switch_timeout);
        } catch (const std::exception &error) {
            fail_open(operation, error);
        }
    }

    void TextVtBackend::write_splash(std::string_view bytes, std::string_view operation) {
        if (splash_ < 0)
            invalid_state(operation);
        try {
            io_->write_all(splash_, bytes);
            io_->flush(splash_);
        } catch (const std::exception &error) {
            fail_open(operation, error);
        }
    }

    void TextVtBackend::write_sensitive_splash(std::string_view bytes, std::string_view operation) {
        if (splash_ < 0)
            invalid_state(operation);
        try {
            io_->write_sensitive(splash_, bytes);
        } catch (const std::exception &error) {
            fail_open(operation, error);
        }
    }

    void TextVtBackend::close_splash() noexcept {
        if (splash_ >= 0)
            io_->close_device(std::exchange(splash_, -1));
    }
    void TextVtBackend::close_control() noexcept {
        if (control_ >= 0)
            io_->close_device(std::exchange(control_, -1));
    }

    void TextVtBackend::cleanup(DisplayState final_state, RestoreMode mode) {
        std::string first_error;
        const auto attempt = [&](auto &&operation) {
            try {
                operation();
            } catch (const std::exception &error) {
                if (first_error.empty())
                    first_error = error.what();
            }
        };
        if (splash_ >= 0) {
            if (mode == RestoreMode::clear)
                attempt([&] { io_->write_restore(splash_, clear_splash); });
            if (cursor_saved_ || cursor_hidden_) {
                attempt([&] { io_->write_restore(splash_, restore_screen_state); });
                attempt([&] { io_->flush(splash_); });
            }
            if (terminal_changed_ && terminal_state_) {
                attempt([&] { io_->restore_terminal(splash_, *terminal_state_); });
            }
            if (keyboard_changed_ && original_keyboard_mode_) {
                attempt([&] { io_->set_keyboard_mode(splash_, *original_keyboard_mode_); });
            }
            if (kd_changed_ && original_kd_mode_) {
                attempt([&] { io_->set_kd_mode(splash_, *original_kd_mode_); });
            }
        }
        close_splash();
        if (control_ >= 0 && original_vt_) {
            attempt([&] { io_->activate(control_, *original_vt_); });
            attempt([&] { io_->wait_active(control_, *original_vt_, vt_switch_timeout); });
            if (allocated_vt_ && splash_vt_) {
                attempt([&] { static_cast<void>(io_->disallocate(control_, *splash_vt_)); });
            }
        }
        close_control();
        original_vt_.reset();
        splash_vt_.reset();
        dimensions_.reset();
        terminal_state_.reset();
        original_kd_mode_.reset();
        original_keyboard_mode_.reset();
        terminal_changed_ = false;
        kd_changed_ = false;
        keyboard_changed_ = false;
        cursor_saved_ = false;
        cursor_hidden_ = false;
        allocated_vt_ = false;
        state_ = final_state;
        if (!first_error.empty()) {
            throw DisplayError(DisplayErrorCode::backend, first_error);
        }
    }

    void TextVtBackend::acquire() {
        if (state_ == DisplayState::hidden || state_ == DisplayState::splash || state_ == DisplayState::details)
            return;
        if (state_ != DisplayState::unacquired)
            invalid_state("acquire");
        state_ = DisplayState::acquiring;
        try {
            control_ = io_->open_control(control_device);
            original_vt_ = io_->active_vt(control_);
            if (config_.selection().kind == VtSelectionKind::open_query) {
                allocated_vt_ = true;
                splash_vt_ = io_->open_query(control_);
            } else {
                splash_vt_ = config_.selection().number;
            }
            validate_vt_number(*splash_vt_);
            if (splash_vt_ == original_vt_)
                throw std::runtime_error("splash VT is already active");
            splash_ = io_->open_vt(std::format("/dev/tty{}", *splash_vt_), *splash_vt_);
            dimensions_ = io_->dimensions(splash_);
            terminal_state_ = io_->terminal_state(splash_);
            original_kd_mode_ = io_->kd_mode(splash_);
            original_keyboard_mode_ = io_->keyboard_mode(splash_);
            io_->write_all(splash_, save_cursor);
            io_->flush(splash_);
            cursor_saved_ = true;
            io_->set_raw_terminal(splash_, *terminal_state_);
            terminal_changed_ = true;
            if (*original_kd_mode_ != kd_text) {
                io_->set_kd_mode(splash_, kd_text);
                kd_changed_ = true;
            }
            if (*original_keyboard_mode_ != k_unicode) {
                io_->set_keyboard_mode(splash_, k_unicode);
                keyboard_changed_ = true;
            }
            state_ = DisplayState::hidden;
        } catch (const std::exception &error) {
            fail_open("acquire display", error);
        }
    }

    void TextVtBackend::show() {
        if (state_ == DisplayState::splash)
            return;
        if (state_ != DisplayState::hidden && state_ != DisplayState::details)
            invalid_state("show");
        if (!splash_vt_)
            invalid_state("show");
        switch_to(*splash_vt_, "activate splash VT");
        if (!cursor_hidden_) {
            write_splash(hide_cursor_clear, "prepare splash VT");
            cursor_hidden_ = true;
        }
        state_ = DisplayState::splash;
    }

    void TextVtBackend::hide() {
        if (state_ == DisplayState::hidden)
            return;
        if (state_ == DisplayState::details) {
            state_ = DisplayState::hidden;
            return;
        }
        if (state_ != DisplayState::splash || !original_vt_)
            invalid_state("hide");
        switch_to(*original_vt_, "activate original VT");
        state_ = DisplayState::hidden;
    }

    void TextVtBackend::render(const Scene &scene) {
        if (state_ != DisplayState::splash)
            invalid_state("render");
        if (!dimensions_ || scene.dimensions() != *dimensions_) {
            throw DisplayError(DisplayErrorCode::size_mismatch, "frame size does not match display");
        }
        write_splash(encode_scene(scene), "render frame");
    }

    void TextVtBackend::render_sensitive_text(std::uint16_t row, std::uint16_t column, std::string_view text,
                                              Style style) {
        if (state_ != DisplayState::splash || !dimensions_)
            invalid_state("render sensitive text");
        static_cast<void>(validate_sensitive_text(*dimensions_, row, column, text));
        auto prefix = std::format("\x1b[{};{}H", row + 1, column + 1);
        append_style(prefix, style);
        write_splash(prefix, "position sensitive text");
        write_sensitive_splash(text, "render sensitive text");
        write_splash("\x1b[0m", "finish sensitive text");
    }

    std::optional<InputEvent> TextVtBackend::poll_input(std::chrono::milliseconds timeout) {
        if (state_ == DisplayState::hidden)
            return std::nullopt;
        if (state_ == DisplayState::details) {
            if (control_ < 0)
                invalid_state("observe active details VT");
            try {
                return io_->active_vt(control_) == splash_vt_ ? std::optional{InputEvent::return_to_splash()}
                                                              : std::nullopt;
            } catch (const std::exception &error) {
                fail_open("observe active details VT", error);
            }
        }
        if (state_ != DisplayState::splash || splash_ < 0)
            invalid_state("poll input");
        try {
            const auto measured = io_->dimensions(splash_);
            if (measured != dimensions_) {
                dimensions_ = measured;
                return InputEvent::resized(measured);
            }
            auto bytes = io_->poll_read(splash_, timeout);
            if (bytes && !bytes->empty())
                return InputEvent::bytes(std::move(*bytes));
            return std::nullopt;
        } catch (const std::exception &error) {
            fail_open("poll splash input", error);
        }
    }

    void TextVtBackend::details(bool visible) {
        if (state_ != DisplayState::hidden && state_ != DisplayState::splash && state_ != DisplayState::details)
            invalid_state("change details visibility");
        if (visible) {
            if (state_ == DisplayState::details)
                return;
            if (state_ == DisplayState::splash)
                switch_to(*original_vt_, "activate details VT");
            state_ = DisplayState::details;
        } else {
            if (state_ == DisplayState::splash)
                return;
            switch_to(*splash_vt_, "reactivate splash VT");
            if (!cursor_hidden_) {
                write_splash(hide_cursor_clear, "prepare splash VT");
                cursor_hidden_ = true;
            }
            state_ = DisplayState::splash;
        }
    }

    void TextVtBackend::restore() { restore(RestoreMode::clear); }
    void TextVtBackend::restore(RestoreMode mode) {
        if (state_ == DisplayState::restored || state_ == DisplayState::failed_open)
            return;
        if (state_ == DisplayState::unacquired) {
            state_ = DisplayState::restored;
            return;
        }
        cleanup(DisplayState::restored, mode);
    }

    std::string encode_scene(const Scene &scene) {
        std::string output("\x1b[H\x1b[0m");
        output.reserve(scene.cells().size() * 4 + 64);
        Style active_style;
        for (std::uint16_t row = 0; row < scene.dimensions().rows(); ++row) {
            output.append(std::format("\x1b[{};1H", row + 1));
            for (std::uint16_t column = 0; column < scene.dimensions().columns(); ++column) {
                const auto cell = *scene.get(column, row);
                if (cell.style() != active_style) {
                    append_style(output, cell.style());
                    active_style = cell.style();
                }
                output.append(encode_utf8(cell.glyph()));
            }
        }
        output.append("\x1b[0m");
        return output;
    }

} // namespace sart::display
