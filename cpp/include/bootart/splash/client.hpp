#pragma once

#include "bootart/splash/protocol.hpp"
#include "bootart/splash/runtime.hpp"

#include <chrono>
#include <cstdint>

namespace bootart::splash {

    struct ClientConfig {
        RuntimePaths runtime;
        std::chrono::milliseconds timeout{2000};
        std::uint32_t expected_server_uid;

        explicit ClientConfig(RuntimePaths runtime = RuntimePaths());
    };

    [[nodiscard]] std::uint64_t next_request_id() noexcept;
    [[nodiscard]] Frame send_request(const ClientConfig &config, const Frame &request);

} // namespace bootart::splash
