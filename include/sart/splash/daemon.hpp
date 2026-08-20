#pragma once

#include "sart/display_text_vt.hpp"
#include "sart/splash/engine.hpp"
#include "sart/splash/runtime.hpp"
#include "sart/splash/state.hpp"

namespace sart::splash {

    enum class PasswordBroker { none, systemd, native };

    struct DaemonConfig {
        RuntimePaths runtime;
        Mode mode{Mode::boot};
        bool test_buffer{};
        std::filesystem::path cmdline{"/proc/cmdline"};
        TextVtConfig display{TextVtConfig::open_query()};
        EngineConfig engine{};
        PasswordBroker password_broker{PasswordBroker::none};
    };

    void run_daemon(const DaemonConfig &config);

} // namespace sart::splash
