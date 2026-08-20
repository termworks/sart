#pragma once

namespace sart::signals {

    class SignalGuard {
      public:
        SignalGuard();
        SignalGuard(const SignalGuard &) = delete;
        SignalGuard &operator=(const SignalGuard &) = delete;
        SignalGuard(SignalGuard &&other) noexcept;
        SignalGuard &operator=(SignalGuard &&) = delete;
        ~SignalGuard();

      private:
        bool active_{true};
    };

    [[nodiscard]] bool should_stop() noexcept;
    void reset_stop_flag() noexcept;

} // namespace sart::signals
