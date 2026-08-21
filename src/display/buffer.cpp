#include "sart/display/buffer.hpp"

#include <utility>

namespace sart::display {

    BufferBackend::BufferBackend(Dimensions dimensions) : dimensions_(dimensions) {}
    void BufferBackend::queue_input(InputEvent event) { input_.push_back(std::move(event)); }
    const std::vector<Scene> &BufferBackend::frames() const noexcept { return frames_; }
    const std::vector<BufferOperation> &BufferBackend::operations() const noexcept { return operations_; }
    DisplayState BufferBackend::state() const noexcept { return state_; }
    std::optional<Dimensions> BufferBackend::dimensions() const noexcept {
        return owns_resources(state_) ? std::optional{dimensions_} : std::nullopt;
    }

    void BufferBackend::require_owned(std::string_view operation) const {
        if (!owns_resources(state_) || state_ == DisplayState::acquiring) {
            throw DisplayError(DisplayErrorCode::invalid_state,
                               std::string("cannot ") + std::string(operation) + " display");
        }
    }

    void BufferBackend::acquire() {
        if (state_ == DisplayState::unacquired) {
            operations_.emplace_back(BufferOperation::Kind::acquire);
            state_ = DisplayState::hidden;
        } else if (!owns_resources(state_) || state_ == DisplayState::acquiring) {
            throw DisplayError(DisplayErrorCode::invalid_state, "cannot acquire display");
        }
    }

    void BufferBackend::show() {
        require_owned("show");
        if (state_ != DisplayState::splash) {
            operations_.emplace_back(BufferOperation::Kind::show);
            state_ = DisplayState::splash;
        }
    }

    void BufferBackend::hide() {
        require_owned("hide");
        if (state_ != DisplayState::hidden) {
            operations_.emplace_back(BufferOperation::Kind::hide);
            state_ = DisplayState::hidden;
        }
    }

    void BufferBackend::render(const Scene &scene) {
        if (state_ != DisplayState::splash) {
            throw DisplayError(DisplayErrorCode::invalid_state, "cannot render display");
        }
        if (scene.dimensions() != dimensions_) {
            throw DisplayError(DisplayErrorCode::size_mismatch, "frame size does not match display");
        }
        operations_.emplace_back(BufferOperation::Kind::render);
        frames_.push_back(scene);
    }

    void BufferBackend::render_sensitive_text(std::uint16_t row, std::uint16_t column, std::string_view text, Style) {
        if (state_ != DisplayState::splash) {
            throw DisplayError(DisplayErrorCode::invalid_state, "cannot render sensitive text");
        }
        const auto cells = validate_sensitive_text(dimensions_, row, column, text);
        BufferOperation operation(BufferOperation::Kind::render_sensitive_text);
        operation.row = row;
        operation.column = column;
        operation.cells = cells;
        operations_.push_back(operation);
    }

    std::optional<InputEvent> BufferBackend::poll_input(std::chrono::milliseconds timeout) {
        require_owned("poll input");
        BufferOperation operation{BufferOperation::Kind::poll_input};
        operation.timeout = timeout;
        operations_.push_back(operation);
        if (input_.empty())
            return std::nullopt;
        if (state_ != DisplayState::splash &&
            !(state_ == DisplayState::details && input_.front().kind() == InputEvent::Kind::return_to_splash)) {
            return std::nullopt;
        }
        auto event = std::move(input_.front());
        input_.pop_front();
        if (event.kind() == InputEvent::Kind::resized)
            dimensions_ = *event.resized_dimensions();
        return event;
    }

    void BufferBackend::details(bool visible) {
        require_owned("change details visibility");
        const auto target = visible ? DisplayState::details : DisplayState::splash;
        if (state_ != target) {
            BufferOperation operation{BufferOperation::Kind::details};
            operation.visible = visible;
            operations_.push_back(operation);
            state_ = target;
        }
    }

    void BufferBackend::restore() { restore(RestoreMode::clear); }

    void BufferBackend::restore(RestoreMode mode) {
        if (state_ == DisplayState::restored || state_ == DisplayState::failed_open)
            return;
        BufferOperation operation{BufferOperation::Kind::restore};
        operation.restore_mode = mode;
        operations_.push_back(operation);
        state_ = DisplayState::restored;
    }

} // namespace sart::display
