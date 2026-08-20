#pragma once

#include "sart/display.hpp"

#include <filesystem>
#include <memory>
#include <termios.h>

namespace sart {

    inline constexpr std::uint16_t minimum_linux_vt = 1;
    inline constexpr std::uint16_t maximum_linux_vt = 63;

    enum class VtSelectionKind { open_query, configured };

    struct VtSelection {
        VtSelectionKind kind{VtSelectionKind::open_query};
        std::uint16_t number{};
        auto operator<=>(const VtSelection &) const = default;
    };

    class TextVtConfig {
      public:
        static TextVtConfig open_query() noexcept;
        static TextVtConfig configured(std::uint16_t number);
        [[nodiscard]] VtSelection selection() const noexcept;

      private:
        explicit TextVtConfig(VtSelection selection) noexcept;
        VtSelection selection_;
    };

    enum class VtDeallocation { deallocated, in_use };

    class VtIo {
      public:
        virtual ~VtIo() = default;
        [[nodiscard]] virtual int open_control(const std::filesystem::path &path) = 0;
        [[nodiscard]] virtual std::uint16_t active_vt(int control) = 0;
        [[nodiscard]] virtual std::uint16_t open_query(int control) = 0;
        [[nodiscard]] virtual int open_vt(const std::filesystem::path &path, std::uint16_t number) = 0;
        virtual void close_device(int device) noexcept = 0;
        [[nodiscard]] virtual Dimensions dimensions(int vt) = 0;
        [[nodiscard]] virtual termios terminal_state(int vt) = 0;
        virtual void set_raw_terminal(int vt, const termios &original) = 0;
        virtual void restore_terminal(int vt, const termios &original) = 0;
        [[nodiscard]] virtual int kd_mode(int vt) = 0;
        virtual void set_kd_mode(int vt, int mode) = 0;
        [[nodiscard]] virtual int keyboard_mode(int vt) = 0;
        virtual void set_keyboard_mode(int vt, int mode) = 0;
        virtual void activate(int control, std::uint16_t number) = 0;
        virtual void wait_active(int control, std::uint16_t number, std::chrono::milliseconds timeout) = 0;
        [[nodiscard]] virtual VtDeallocation disallocate(int control, std::uint16_t number) = 0;
        virtual void write_all(int vt, std::string_view bytes) = 0;
        virtual void write_restore(int vt, std::string_view bytes);
        virtual void write_sensitive(int vt, std::string_view bytes) = 0;
        virtual void flush(int vt) = 0;
        [[nodiscard]] virtual std::optional<std::vector<std::uint8_t>> poll_read(int vt,
                                                                                 std::chrono::milliseconds timeout) = 0;
    };

    class LinuxVtIo final : public VtIo {
      public:
        [[nodiscard]] int open_control(const std::filesystem::path &path) override;
        [[nodiscard]] std::uint16_t active_vt(int control) override;
        [[nodiscard]] std::uint16_t open_query(int control) override;
        [[nodiscard]] int open_vt(const std::filesystem::path &path, std::uint16_t number) override;
        void close_device(int device) noexcept override;
        [[nodiscard]] Dimensions dimensions(int vt) override;
        [[nodiscard]] termios terminal_state(int vt) override;
        void set_raw_terminal(int vt, const termios &original) override;
        void restore_terminal(int vt, const termios &original) override;
        [[nodiscard]] int kd_mode(int vt) override;
        void set_kd_mode(int vt, int mode) override;
        [[nodiscard]] int keyboard_mode(int vt) override;
        void set_keyboard_mode(int vt, int mode) override;
        void activate(int control, std::uint16_t number) override;
        void wait_active(int control, std::uint16_t number, std::chrono::milliseconds timeout) override;
        [[nodiscard]] VtDeallocation disallocate(int control, std::uint16_t number) override;
        void write_all(int vt, std::string_view bytes) override;
        void write_restore(int vt, std::string_view bytes) override;
        void write_sensitive(int vt, std::string_view bytes) override;
        void flush(int vt) override;
        [[nodiscard]] std::optional<std::vector<std::uint8_t>> poll_read(int vt,
                                                                         std::chrono::milliseconds timeout) override;
    };

    class TextVtBackend final : public DisplayBackend {
      public:
        explicit TextVtBackend(TextVtConfig config);
        TextVtBackend(TextVtConfig config, std::unique_ptr<VtIo> io);
        TextVtBackend(const TextVtBackend &) = delete;
        TextVtBackend &operator=(const TextVtBackend &) = delete;
        ~TextVtBackend() override;

        [[nodiscard]] std::optional<std::uint16_t> original_vt() const noexcept;
        [[nodiscard]] std::optional<std::uint16_t> splash_vt() const noexcept;
        [[nodiscard]] DisplayState state() const noexcept override;
        [[nodiscard]] std::optional<Dimensions> dimensions() const noexcept override;
        void acquire() override;
        void show() override;
        void hide() override;
        void render(const Scene &scene) override;
        void render_sensitive_text(std::uint16_t row, std::uint16_t column, std::string_view text,
                                   Style style) override;
        [[nodiscard]] std::optional<InputEvent> poll_input(std::chrono::milliseconds timeout) override;
        void details(bool visible) override;
        void restore() override;
        void restore(RestoreMode mode) override;

      private:
        [[noreturn]] void invalid_state(std::string_view operation) const;
        [[noreturn]] void fail_open(std::string_view operation, const std::exception &source);
        void switch_to(std::uint16_t number, std::string_view operation);
        void write_splash(std::string_view bytes, std::string_view operation);
        void write_sensitive_splash(std::string_view bytes, std::string_view operation);
        void cleanup(DisplayState final_state, RestoreMode mode);
        void close_splash() noexcept;
        void close_control() noexcept;

        TextVtConfig config_;
        std::unique_ptr<VtIo> io_;
        DisplayState state_{DisplayState::unacquired};
        int control_{-1};
        int splash_{-1};
        std::optional<std::uint16_t> original_vt_;
        std::optional<std::uint16_t> splash_vt_;
        std::optional<Dimensions> dimensions_;
        std::optional<termios> terminal_state_;
        std::optional<int> original_kd_mode_;
        std::optional<int> original_keyboard_mode_;
        bool terminal_changed_{};
        bool kd_changed_{};
        bool keyboard_changed_{};
        bool cursor_saved_{};
        bool cursor_hidden_{};
        bool allocated_vt_{};
    };

    [[nodiscard]] std::string encode_scene(const Scene &scene);

} // namespace sart
