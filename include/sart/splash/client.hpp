#pragma once

#include "sart/splash/protocol.hpp"
#include "sart/splash/runtime.hpp"

#include <chrono>
#include <cstdint>

namespace sart::splash {

    struct ClientConfig {
        RuntimePaths runtime;
        std::chrono::milliseconds timeout{2000};
        std::uint32_t expected_server_uid;

        explicit ClientConfig(RuntimePaths runtime = RuntimePaths());
    };

    [[nodiscard]] std::uint64_t next_request_id() noexcept;
    [[nodiscard]] Frame send_request(const ClientConfig &config, const Frame &request);

} // namespace sart::splash
