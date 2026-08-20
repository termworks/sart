#pragma once

#include "bootart/display_text_vt.hpp"
#include "bootart/splash/engine.hpp"
#include "bootart/splash/runtime.hpp"
#include "bootart/splash/state.hpp"

namespace bootart::splash {

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

} // namespace bootart::splash
