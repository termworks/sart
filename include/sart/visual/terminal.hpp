#pragma once

#include <compare>
#include <cstddef>
#include <optional>
#include <string>
#include <string_view>

namespace sart::visual {

    struct TerminalSize {
        std::size_t width{80};
        std::size_t height{24};
        auto operator<=>(const TerminalSize &) const = default;
    };

    class TerminalOutput {
      public:
        virtual ~TerminalOutput() = default;
        [[nodiscard]] virtual TerminalSize dimensions() const = 0;
        virtual void write_frame(std::string_view bytes) = 0;
        virtual void flush() = 0;
    };

    class StdoutTerminal final : public TerminalOutput {
      public:
        StdoutTerminal();
        StdoutTerminal(std::optional<std::size_t> columns, std::optional<std::size_t> rows);
        StdoutTerminal(const StdoutTerminal &) = delete;
        StdoutTerminal &operator=(const StdoutTerminal &) = delete;
        ~StdoutTerminal() override;

        [[nodiscard]] TerminalSize dimensions() const override;
        void write_frame(std::string_view bytes) override;
        void flush() override;

      private:
        [[nodiscard]] int output_descriptor();
        void write_bounded(std::string_view bytes, int timeout_milliseconds, bool stop_aware);
        void restore_terminal();

        int output_{-1};
        std::optional<TerminalSize> override_size_;
        bool needs_restore_{};
    };

    class BufferTerminal final : public TerminalOutput {
      public:
        explicit BufferTerminal(TerminalSize size);
        [[nodiscard]] TerminalSize dimensions() const override;
        void write_frame(std::string_view bytes) override;
        void flush() override;
        [[nodiscard]] const std::string &contents() const noexcept;

      private:
        TerminalSize size_;
        std::string buffer_;
    };

} // namespace sart::visual

namespace sart {
    using visual::BufferTerminal;
    using visual::StdoutTerminal;
    using visual::TerminalOutput;
    using visual::TerminalSize;
} // namespace sart
