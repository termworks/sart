#pragma once

#include "bootart/display.hpp"

#include <deque>

namespace bootart {

    struct BufferOperation {
        enum class Kind {
            acquire,
            show,
            hide,
            render,
            render_sensitive_text,
            poll_input,
            details,
            restore,
        };

        explicit BufferOperation(Kind operation_kind) : kind(operation_kind) {}

        Kind kind;
        std::uint16_t row{};
        std::uint16_t column{};
        std::size_t cells{};
        std::chrono::milliseconds timeout{};
        bool visible{};
        RestoreMode restore_mode{RestoreMode::clear};
        auto operator<=>(const BufferOperation &) const = default;
    };

    class BufferBackend final : public DisplayBackend {
      public:
        explicit BufferBackend(Dimensions dimensions);
        void queue_input(InputEvent event);
        [[nodiscard]] const std::vector<Scene> &frames() const noexcept;
        [[nodiscard]] const std::vector<BufferOperation> &operations() const noexcept;

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
        void require_owned(std::string_view operation) const;

        Dimensions dimensions_;
        DisplayState state_{DisplayState::unacquired};
        std::vector<Scene> frames_;
        std::deque<InputEvent> input_;
        std::vector<BufferOperation> operations_;
    };

} // namespace bootart
