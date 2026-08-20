#include "bootart/password_coordinator.hpp"

#include <algorithm>
#include <cerrno>
#include <deque>
#include <poll.h>
#include <set>
#include <sys/socket.h>

namespace bootart::password {

    SystemdPromptCoordinator::SystemdPromptCoordinator(std::filesystem::path directory, std::uint32_t expected_uid,
                                                       std::size_t maximum_secret_size)
        : directory_(std::move(directory)), expected_uid_(expected_uid), maximum_secret_size_(maximum_secret_size) {
        if (maximum_secret_size == 0 || maximum_secret_size > maximum_secret_bytes) {
            throw std::invalid_argument("invalid coordinator secret capacity");
        }
        static_cast<void>(scan_ask_requests(directory_, expected_uid_));
    }

    bool SystemdPromptCoordinator::enabled() const noexcept { return enabled_; }

    void SystemdPromptCoordinator::poll(splash::SplashState &state) {
        if (!enabled_)
            return;
        try {
            auto scan = scan_ask_requests(directory_, expected_uid_);
            const auto now = monotonic_microseconds();
            std::erase_if(scan.requests, [&](const AskRequest &request) {
                return request.expired(now) || !requester_alive(request.requester_pid()) ||
                       retired_.contains(request.id());
            });
            if (request_) {
                const auto found = std::ranges::find_if(
                    scan.requests, [&](const AskRequest &candidate) { return candidate.id() == request_->id(); });
                if (found == scan.requests.end())
                    finish(state, splash::PromptOutcome::request_gone);
            }
            if (!request_ && !scan.requests.empty()) {
                request_ = std::move(scan.requests.front());
                input_ = std::make_unique<PromptInput>(maximum_secret_size_, request_->echo(), request_->silent());
                splash::PromptMetadata metadata(prompt_id(), request_->message());
                metadata.with_source(request_->id().name)
                    .with_requester_pid(request_->requester_pid())
                    .with_echo(request_->echo())
                    .with_silent(request_->silent())
                    .with_expiry(request_->not_after_microseconds() / 1000);
                static_cast<void>(state.apply(splash::BeginPrompt{std::move(metadata)}));
            }
        } catch (...) {
            enabled_ = false;
            abandon(state);
        }
    }

    void SystemdPromptCoordinator::handle_input(splash::SplashState &state, std::span<const std::uint8_t> bytes) {
        if (!enabled_ || !request_ || !input_)
            return;
        for (const auto byte : bytes) {
            const auto outcome = input_->feed(byte);
            if (outcome.kind == InputOutcomeKind::submit) {
                try {
                    input_->finish_with(
                        [&](SecureSecret &secret) { reply_.send_success(*request_, secret, expected_uid_); });
                    finish(state, splash::PromptOutcome::answered);
                } catch (...) {
                    finish(state, splash::PromptOutcome::cancelled);
                }
                return;
            }
            if (outcome.kind == InputOutcomeKind::cancelled) {
                try {
                    reply_.send_cancel(*request_, expected_uid_);
                } catch (...) {
                }
                finish(state, splash::PromptOutcome::cancelled);
                return;
            }
        }
    }

    std::optional<InputFeedback> SystemdPromptCoordinator::feedback() const noexcept {
        return input_ ? std::optional(input_->feedback()) : std::nullopt;
    }

    void SystemdPromptCoordinator::with_visible_text(const std::function<void(std::string_view)> &action) const {
        if (!input_)
            return;
        input_->with_visible_text([&](std::optional<std::string_view> text) {
            if (text)
                action(*text);
        });
    }

    void SystemdPromptCoordinator::abandon(splash::SplashState &state) noexcept {
        if (!request_)
            return;
        try {
            finish(state, splash::PromptOutcome::cancelled);
        } catch (...) {
            request_.reset();
            input_.reset();
        }
    }

    std::uint64_t SystemdPromptCoordinator::prompt_id() const noexcept {
        if (!request_)
            return 0;
        auto value = request_->id().device ^ (request_->id().inode + 0x9e3779b97f4a7c15ULL);
        for (const auto byte : request_->id().name)
            value = (value ^ static_cast<unsigned char>(byte)) * 1099511628211ULL;
        return value == 0 ? 1 : value;
    }

    void SystemdPromptCoordinator::finish(splash::SplashState &state, splash::PromptOutcome outcome) {
        if (!request_)
            return;
        const auto id = prompt_id();
        retired_.insert(request_->id());
        input_.reset();
        request_.reset();
        static_cast<void>(state.apply(splash::FinishPrompt{id, outcome}));
    }

    class NativePromptCoordinator::Impl {
      public:
        struct Request {
            NativeRequestMetadata metadata;
            NativeCredentialResponder responder;
        };
        struct Pending {
            splash::FileDescriptor carrier;
            std::uint32_t pid;
            std::uint64_t accepted_at;
        };
        struct Active {
            Request request;
            std::uint64_t presentation_id;
            std::unique_ptr<PromptInput> input;
        };

        Impl(splash::FileDescriptor owned_listener, std::uint32_t uid)
            : listener(std::move(owned_listener)), required_uid(uid) {}

        void poll(splash::SplashState &state) {
            if (!is_enabled)
                return;
            try {
                const auto now = monotonic_microseconds();
                accept(now);
                receive(now);
                reap(state, now);
                activate(state, now);
            } catch (...) {
                disable(state);
            }
        }

        void accept(std::uint64_t now) {
            for (std::size_t count = 0; count < 8; ++count) {
                splash::FileDescriptor carrier(accept4(listener.get(), nullptr, nullptr, SOCK_CLOEXEC | SOCK_NONBLOCK));
                if (!carrier) {
                    if (errno == EAGAIN || errno == EWOULDBLOCK)
                        return;
                    if (errno == EINTR)
                        continue;
                    throw std::system_error(errno, std::generic_category(), "accept native password request");
                }
                const auto credentials = splash::peer_credentials(carrier.get());
                if (credentials.uid == required_uid && credentials.pid != 0 && pending.size() < 32) {
                    pending.push_back({std::move(carrier), credentials.pid, now});
                }
            }
        }

        void receive(std::uint64_t now) {
            const auto attempts = std::min<std::size_t>(pending.size(), 16);
            for (std::size_t count = 0; count < attempts; ++count) {
                auto carrier = std::move(pending.front());
                pending.pop_front();
                std::array<std::byte, 52 + 1024> packet{};
                try {
                    auto received = receive_responder_packet(carrier.carrier.get(), required_uid, packet);
                    auto metadata =
                        decode_native_request(std::span<const std::byte>(packet).first(received.metadata_size), now);
                    if (metadata.identity.requester_pid != carrier.pid)
                        continue;
                    if (!retired.contains(metadata.identity) && queue.size() + (active ? 1 : 0) < 32) {
                        queue.push_back({std::move(metadata), std::move(received.responder)});
                    }
                } catch (const std::system_error &error) {
                    if ((error.code().value() == EAGAIN || error.code().value() == EWOULDBLOCK) &&
                        now - carrier.accepted_at < 2'000'000) {
                        pending.push_back(std::move(carrier));
                    }
                } catch (...) {
                }
            }
        }

        bool alive(const NativeRequestMetadata &metadata) const {
            try {
                return process_start_ticks(metadata.identity.requester_pid) == metadata.identity.requester_start_ticks;
            } catch (...) {
                return false;
            }
        }

        void reap(splash::SplashState &state, std::uint64_t now) {
            if (active && (active->request.metadata.deadline_microseconds <= now || !alive(active->request.metadata))) {
                finish(state, splash::PromptOutcome::request_gone);
            }
            std::erase_if(queue, [&](Request &request) {
                if (request.metadata.deadline_microseconds > now && alive(request.metadata))
                    return false;
                retire(request.metadata.identity);
                return true;
            });
        }

        void activate(splash::SplashState &state, std::uint64_t now) {
            while (!active && !queue.empty()) {
                auto request = std::move(queue.front());
                queue.pop_front();
                if (request.metadata.deadline_microseconds <= now || !alive(request.metadata)) {
                    retire(request.metadata.identity);
                    continue;
                }
                const auto id = next_presentation_id++;
                auto input = std::make_unique<PromptInput>(request.metadata.maximum_secret_bytes, request.metadata.echo,
                                                           request.metadata.silent);
                splash::PromptMetadata presentation(id, request.metadata.prompt);
                presentation.with_source(std::string(prompt_source(request.metadata.adapter)))
                    .with_requester_pid(request.metadata.identity.requester_pid)
                    .with_echo(request.metadata.echo)
                    .with_silent(request.metadata.silent)
                    .with_expiry(request.metadata.deadline_microseconds / 1000);
                static_cast<void>(state.apply(splash::BeginPrompt{std::move(presentation)}));
                active = Active{std::move(request), id, std::move(input)};
            }
        }

        void input(splash::SplashState &state, std::span<const std::uint8_t> bytes) {
            if (!is_enabled || !active)
                return;
            for (const auto byte : bytes) {
                const auto outcome = active->input->feed(byte);
                if (outcome.kind == InputOutcomeKind::submit) {
                    try {
                        active->input->finish_with(
                            [&](SecureSecret &secret) { active->request.responder.reply_secret(secret); });
                        finish(state, splash::PromptOutcome::answered);
                    } catch (...) {
                        finish(state, splash::PromptOutcome::request_gone);
                    }
                    return;
                }
                if (outcome.kind == InputOutcomeKind::cancelled) {
                    try {
                        active->request.responder.reply_cancel();
                        finish(state, splash::PromptOutcome::cancelled);
                    } catch (...) {
                        finish(state, splash::PromptOutcome::request_gone);
                    }
                    return;
                }
            }
        }

        void finish(splash::SplashState &state, splash::PromptOutcome outcome) {
            if (!active)
                return;
            const auto identity = active->request.metadata.identity;
            const auto id = active->presentation_id;
            active.reset();
            retire(identity);
            static_cast<void>(state.apply(splash::FinishPrompt{id, outcome}));
        }

        void retire(NativeRequestIdentity identity) {
            if (retired.size() >= 64)
                retired.erase(retired.begin());
            retired.insert(identity);
        }

        void disable(splash::SplashState &state) noexcept {
            is_enabled = false;
            pending.clear();
            queue.clear();
            listener = splash::FileDescriptor();
            try {
                finish(state, splash::PromptOutcome::request_gone);
            } catch (...) {
                active.reset();
            }
        }

        splash::FileDescriptor listener;
        std::uint32_t required_uid;
        std::deque<Pending> pending;
        std::deque<Request> queue;
        std::optional<Active> active;
        std::set<NativeRequestIdentity> retired;
        std::uint64_t next_presentation_id{1};
        bool is_enabled{true};
    };

    NativePromptCoordinator::NativePromptCoordinator(splash::FileDescriptor listener, std::uint32_t required_uid)
        : impl_(std::make_unique<Impl>(std::move(listener), required_uid)) {}
    NativePromptCoordinator::~NativePromptCoordinator() = default;
    bool NativePromptCoordinator::enabled() const noexcept { return impl_->is_enabled; }
    void NativePromptCoordinator::poll(splash::SplashState &state) { impl_->poll(state); }
    void NativePromptCoordinator::handle_input(splash::SplashState &state, std::span<const std::uint8_t> bytes) {
        impl_->input(state, bytes);
    }
    std::optional<InputFeedback> NativePromptCoordinator::feedback() const noexcept {
        return impl_->active ? std::optional(impl_->active->input->feedback()) : std::nullopt;
    }
    void NativePromptCoordinator::with_visible_text(const std::function<void(std::string_view)> &action) const {
        if (!impl_->active)
            return;
        impl_->active->input->with_visible_text([&](std::optional<std::string_view> text) {
            if (text)
                action(*text);
        });
    }
    void NativePromptCoordinator::abandon(splash::SplashState &state) noexcept { impl_->disable(state); }

} // namespace bootart::password
