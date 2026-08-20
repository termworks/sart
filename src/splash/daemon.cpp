#include "sart/splash/daemon.hpp"

#include "sart/cmdline.hpp"
#include "sart/display_buffer.hpp"
#include "sart/display_text_vt.hpp"
#include "sart/embedded.hpp"
#include "sart/password_coordinator.hpp"
#include "sart/password_secure.hpp"
#include "sart/process.hpp"
#include "sart/signals.hpp"
#include "sart/splash/command.hpp"
#include "sart/splash/engine.hpp"
#include "sart/splash/protocol.hpp"

#include <cerrno>
#include <poll.h>
#include <stdexcept>
#include <sys/socket.h>
#include <unistd.h>

namespace sart::splash {

    void run_daemon(const DaemonConfig &config) {
        if (!process_is_allowed(static_cast<std::uint32_t>(getpid()))) {
            throw std::runtime_error("splash daemon refuses to run as PID 1");
        }
        if (config.test_buffer && config.runtime.is_production()) {
            throw std::runtime_error("test-buffer is forbidden with the production runtime directory");
        }
        if (cmdline::splash_disabled_at(config.cmdline))
            return;
        if (config.password_broker != PasswordBroker::none) {
            password::protect_process_secrets();
        }
        auto backend = config.test_buffer
                           ? std::unique_ptr<DisplayBackend>(std::make_unique<BufferBackend>(Dimensions(80, 24)))
                           : std::unique_ptr<DisplayBackend>(std::make_unique<TextVtBackend>(config.display));
        const auto main_art = Art::parse(embedded::default_art);
        const auto small_art = Art::parse(embedded::small_art);
        auto owner = RuntimeOwner::acquire(config.runtime);
        auto listener = owner.bind_listener();
        std::unique_ptr<password::PromptCoordinator> prompt;
        if (config.password_broker == PasswordBroker::systemd) {
            prompt = std::make_unique<password::SystemdPromptCoordinator>();
        } else if (config.password_broker == PasswordBroker::native) {
            prompt = std::make_unique<password::NativePromptCoordinator>(owner.bind_native_password_listener(),
                                                                         owner.required_client_uid());
        }
        SplashState state(config.mode);
        static_cast<void>(state.apply(MarkRunning{}));
        SplashEngine engine(std::move(backend), main_art, &small_art, config.engine);
        engine.start(state);
        const auto started = std::chrono::steady_clock::now();
        bool should_quit{};
        bool retain_splash{};
        LinuxSelfRootTransition root_transition;
        while (!should_quit && !signals::should_stop()) {
            const auto elapsed = std::chrono::steady_clock::now() - started;
            if (prompt) {
                prompt->poll(state);
                if (!prompt->enabled())
                    throw std::runtime_error("password broker became unavailable");
            }
            static_cast<void>(engine.tick_at(state, elapsed, prompt.get()));
            const auto until_frame = engine.time_until_next_frame(elapsed);
            const auto wait = std::clamp<std::int64_t>(
                std::chrono::duration_cast<std::chrono::milliseconds>(until_frame).count(), 0, 50);
            pollfd event{listener.get(), POLLIN, 0};
            const auto ready = poll(&event, 1, static_cast<int>(wait));
            if (ready < 0) {
                if (errno == EINTR)
                    continue;
                throw std::runtime_error("cannot poll control listener");
            }
            if (ready == 0)
                continue;
            FileDescriptor connection(accept4(listener.get(), nullptr, nullptr, SOCK_CLOEXEC));
            if (!connection) {
                if (errno == EINTR || errno == EAGAIN)
                    continue;
                throw std::runtime_error("cannot accept control client");
            }
            Frame response = Frame::error(0, "invalid control request");
            try {
                const auto request = Frame::read_exact_message(connection.get());
                const auto credentials = peer_credentials(connection.get());
                if (is_mutating(request.opcode()) && credentials.uid != owner.required_client_uid()) {
                    response = Frame::error(request.request_id(), "client UID is not authorized");
                } else if (request.opcode() == Opcode::native_ready &&
                           config.password_broker == PasswordBroker::native && prompt && prompt->enabled()) {
                    response = Frame::ack(request.request_id());
                } else {
                    auto outcome = handle_request(state, request, root_transition);
                    response = std::move(outcome.response);
                    should_quit = outcome.should_quit;
                    retain_splash = outcome.retain_splash;
                }
            } catch (const ProtocolError &error) {
                response = Frame::error(0, error.what());
            }
            response.write_to_fd(connection.get());
            shutdown(connection.get(), SHUT_WR);
        }
        if (state.lifecycle() != Lifecycle::quitting) {
            static_cast<void>(state.apply(Quit{}));
        }
        if (prompt)
            prompt->abandon(state);
        engine.shutdown(retain_splash);
        static_cast<void>(state.apply(MarkStopped{}));
    }

} // namespace sart::splash
