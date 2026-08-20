#pragma once

#include "bootart/password_input.hpp"
#include "bootart/password_native.hpp"
#include "bootart/password_systemd.hpp"
#include "bootart/splash/state.hpp"

#include <filesystem>
#include <functional>
#include <memory>
#include <set>
#include <span>

namespace bootart::password {

    class PromptCoordinator {
      public:
        virtual ~PromptCoordinator() = default;
        [[nodiscard]] virtual bool enabled() const noexcept = 0;
        virtual void poll(splash::SplashState &state) = 0;
        virtual void handle_input(splash::SplashState &state, std::span<const std::uint8_t> bytes) = 0;
        [[nodiscard]] virtual std::optional<InputFeedback> feedback() const noexcept = 0;
        virtual void with_visible_text(const std::function<void(std::string_view)> &action) const = 0;
        virtual void abandon(splash::SplashState &state) noexcept = 0;
    };

    class SystemdPromptCoordinator final : public PromptCoordinator {
      public:
        explicit SystemdPromptCoordinator(std::filesystem::path directory = ask_password_directory,
                                          std::uint32_t expected_uid = 0,
                                          std::size_t maximum_secret_size = default_secret_bytes);
        [[nodiscard]] bool enabled() const noexcept override;
        void poll(splash::SplashState &state) override;
        void handle_input(splash::SplashState &state, std::span<const std::uint8_t> bytes) override;
        [[nodiscard]] std::optional<InputFeedback> feedback() const noexcept override;
        void with_visible_text(const std::function<void(std::string_view)> &action) const override;
        void abandon(splash::SplashState &state) noexcept override;

      private:
        [[nodiscard]] std::uint64_t prompt_id() const noexcept;
        void finish(splash::SplashState &state, splash::PromptOutcome outcome);

        std::filesystem::path directory_;
        std::uint32_t expected_uid_{};
        std::size_t maximum_secret_size_{};
        SystemdReplySocket reply_;
        std::optional<AskRequest> request_;
        std::set<AskRequestId> retired_;
        std::unique_ptr<PromptInput> input_;
        bool enabled_{true};
    };

    class NativePromptCoordinator final : public PromptCoordinator {
      public:
        NativePromptCoordinator(splash::FileDescriptor listener, std::uint32_t required_uid);
        NativePromptCoordinator(const NativePromptCoordinator &) = delete;
        NativePromptCoordinator &operator=(const NativePromptCoordinator &) = delete;
        ~NativePromptCoordinator() override;
        [[nodiscard]] bool enabled() const noexcept override;
        void poll(splash::SplashState &state) override;
        void handle_input(splash::SplashState &state, std::span<const std::uint8_t> bytes) override;
        [[nodiscard]] std::optional<InputFeedback> feedback() const noexcept override;
        void with_visible_text(const std::function<void(std::string_view)> &action) const override;
        void abandon(splash::SplashState &state) noexcept override;

      private:
        class Impl;
        std::unique_ptr<Impl> impl_;
    };

} // namespace bootart::password
