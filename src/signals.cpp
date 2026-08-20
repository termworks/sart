#include "sart/signals.hpp"

#include <atomic>
#include <csignal>
#include <stdexcept>
#include <utility>

namespace sart::signals {
    namespace {

        std::atomic_flag stop_flag;
        struct sigaction previous_interrupt{};
        struct sigaction previous_terminate{};
        bool handlers_installed{};

        extern "C" void handle_signal(int) { stop_flag.test_and_set(std::memory_order_relaxed); }

    } // namespace

    SignalGuard::SignalGuard() {
        if (handlers_installed) {
            throw std::runtime_error("signal handlers are already installed");
        }
        struct sigaction action{};
        action.sa_handler = handle_signal;
        sigemptyset(&action.sa_mask);
        action.sa_flags = 0;
        if (sigaction(SIGINT, &action, &previous_interrupt) != 0) {
            throw std::runtime_error("cannot install SIGINT handler");
        }
        if (sigaction(SIGTERM, &action, &previous_terminate) != 0) {
            sigaction(SIGINT, &previous_interrupt, nullptr);
            throw std::runtime_error("cannot install SIGTERM handler");
        }
        handlers_installed = true;
    }

    SignalGuard::SignalGuard(SignalGuard &&other) noexcept : active_(std::exchange(other.active_, false)) {}

    SignalGuard::~SignalGuard() {
        if (!active_) {
            return;
        }
        sigaction(SIGINT, &previous_interrupt, nullptr);
        sigaction(SIGTERM, &previous_terminate, nullptr);
        handlers_installed = false;
    }

    bool should_stop() noexcept { return stop_flag.test(std::memory_order_relaxed); }

    void reset_stop_flag() noexcept { stop_flag.clear(std::memory_order_relaxed); }

} // namespace sart::signals
