#include "sart/splash/command.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <fcntl.h>
#include <format>
#include <linux/openat2.h>
#include <optional>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <system_error>
#include <unistd.h>

namespace sart::splash {
    namespace {

        std::string json_escape(std::string_view value) {
            std::string escaped;
            escaped.reserve(value.size());
            for (const auto character : value) {
                if (character == '"' || character == '\\')
                    escaped.push_back('\\');
                escaped.push_back(character);
            }
            return escaped;
        }

        std::string json_optional_string(const std::optional<std::string> &value) {
            return value ? std::format("\"{}\"", json_escape(*value)) : "null";
        }

        std::string_view lifecycle_name(Lifecycle lifecycle) {
            switch (lifecycle) {
            case Lifecycle::starting:
                return "starting";
            case Lifecycle::running:
                return "running";
            case Lifecycle::deactivated:
                return "deactivated";
            case Lifecycle::quitting:
                return "quitting";
            case Lifecycle::stopped:
                return "stopped";
            case Lifecycle::failed_open:
                return "failed-open";
            }
            return "failed-open";
        }

        std::string_view view_name(const View &view) {
            if (view.prompt_metadata())
                return "prompt";
            switch (*view.base_view()) {
            case BaseView::hidden:
                return "hidden";
            case BaseView::splash:
                return "splash";
            case BaseView::details:
                return "details";
            }
            return "hidden";
        }

        std::string_view root_stage_name(RootStage stage) {
            switch (stage) {
            case RootStage::initramfs:
                return "initramfs";
            case RootStage::switching:
                return "switching";
            case RootStage::real_root:
                return "real-root";
            }
            return "initramfs";
        }

        CommandOutcome state_error(std::uint64_t request_id, const std::exception &error) {
            return {Frame::error(request_id, error.what()), false, false, false};
        }

        class OwnedDescriptor {
          public:
            explicit OwnedDescriptor(int value = -1) : value_(value) {}
            OwnedDescriptor(const OwnedDescriptor &) = delete;
            OwnedDescriptor &operator=(const OwnedDescriptor &) = delete;
            ~OwnedDescriptor() {
                if (value_ >= 0)
                    close(value_);
            }
            [[nodiscard]] int get() const noexcept { return value_; }

          private:
            int value_;
        };

        [[noreturn]] void root_error(std::string_view operation) {
            throw std::system_error(errno, std::generic_category(), std::string(operation));
        }

        OwnedDescriptor secure_open(int directory, const char *path, std::uint64_t flags, std::uint64_t resolve) {
            open_how how{};
            how.flags = flags;
            how.resolve = resolve;
            const auto descriptor = static_cast<int>(syscall(SYS_openat2, directory, path, &how, sizeof(how)));
            if (descriptor < 0)
                root_error("securely open root transition object");
            return OwnedDescriptor(descriptor);
        }

        struct RootObject {
            dev_t device;
            ino_t inode;
            mode_t mode;
            uid_t uid;
            off_t size;
            auto operator<=>(const RootObject &) const = default;
        };

        RootObject inspect_root_object(int descriptor, bool directory, std::string_view name) {
            struct stat metadata{};
            if (fstat(descriptor, &metadata) != 0)
                root_error("inspect root transition object");
            if ((directory && !S_ISDIR(metadata.st_mode)) || (!directory && !S_ISREG(metadata.st_mode)) ||
                metadata.st_uid != 0 || (metadata.st_mode & 0022) != 0 || (metadata.st_mode & S_IXUSR) == 0 ||
                (!directory && (metadata.st_size <= 0 || metadata.st_size > 64 * 1024 * 1024))) {
                throw std::runtime_error("unsafe root transition object: " + std::string(name));
            }
            return {metadata.st_dev, metadata.st_ino, metadata.st_mode, metadata.st_uid, metadata.st_size};
        }

        void compare_executables(int candidate, int running, off_t size) {
            std::array<std::byte, 64 * 1024> left{};
            std::array<std::byte, 64 * 1024> right{};
            off_t offset{};
            while (offset < size) {
                const auto wanted = std::min<std::size_t>(left.size(), static_cast<std::size_t>(size - offset));
                const auto left_count = pread(candidate, left.data(), wanted, offset);
                const auto right_count = pread(running, right.data(), wanted, offset);
                if (left_count != static_cast<ssize_t>(wanted) || right_count != left_count) {
                    root_error("read root transition executable");
                }
                if (!std::equal(left.begin(), left.begin() + left_count, right.begin())) {
                    throw std::runtime_error("new-root sart differs from the running executable");
                }
                offset += left_count;
            }
        }

    } // namespace

    void DeferredRootTransition::transition(const std::filesystem::path &) {}

    void LinuxSelfRootTransition::transition(const std::filesystem::path &new_root) {
        const auto value = new_root.native();
        if (geteuid() != 0)
            throw std::runtime_error("root transition requires UID 0");
        if (!new_root.is_absolute() || value == "/" || value.empty() || value.size() >= 4096 || value.ends_with('/') ||
            value.find("//") != std::string::npos) {
            throw std::invalid_argument("candidate root path is not normalized and absolute");
        }
        for (const auto &component : new_root) {
            if (component == "." || component == "..") {
                throw std::invalid_argument("candidate root path contains a dot component");
            }
        }
        const auto root = secure_open(AT_FDCWD, value.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW,
                                      RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS);
        const auto root_before = inspect_root_object(root.get(), true, "candidate root");
        const auto usr = secure_open(root.get(), "usr", O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW,
                                     RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS);
        const auto usr_before = inspect_root_object(usr.get(), true, "candidate usr");
        const auto bin = secure_open(usr.get(), "bin", O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW,
                                     RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS);
        const auto bin_before = inspect_root_object(bin.get(), true, "candidate usr/bin");
        const auto candidate = secure_open(bin.get(), "sart", O_RDONLY | O_CLOEXEC | O_NOFOLLOW,
                                           RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS);
        const auto candidate_before = inspect_root_object(candidate.get(), false, "candidate sart");
        const OwnedDescriptor running(open("/proc/self/exe", O_RDONLY | O_CLOEXEC));
        if (running.get() < 0)
            root_error("open running executable");
        const auto running_before = inspect_root_object(running.get(), false, "running sart");
        if (candidate_before.size != running_before.size) {
            throw std::runtime_error("new-root sart size differs from the running executable");
        }
        compare_executables(candidate.get(), running.get(), candidate_before.size);
        if (inspect_root_object(root.get(), true, "candidate root") != root_before ||
            inspect_root_object(usr.get(), true, "candidate usr") != usr_before ||
            inspect_root_object(bin.get(), true, "candidate usr/bin") != bin_before ||
            inspect_root_object(candidate.get(), false, "candidate sart") != candidate_before ||
            inspect_root_object(running.get(), false, "running sart") != running_before) {
            throw std::runtime_error("root transition object changed during verification");
        }
        const OwnedDescriptor old_root(open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC));
        const OwnedDescriptor old_cwd(open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC));
        if (old_root.get() < 0 || old_cwd.get() < 0)
            root_error("save root transition rollback point");
        if (fchdir(root.get()) != 0)
            root_error("enter candidate root");
        if (chroot(".") != 0) {
            if (fchdir(old_cwd.get()) != 0) {
            }
            root_error("chroot to candidate root");
        }
        if (chdir("/") != 0) {
            if (fchdir(old_root.get()) != 0) {
            }
            if (chroot(".") != 0) {
            }
            if (fchdir(old_cwd.get()) != 0) {
            }
            root_error("change directory in candidate root");
        }
    }

    bool is_mutating(Opcode opcode) noexcept {
        return opcode != Opcode::ping && opcode != Opcode::state && opcode != Opcode::native_ready;
    }

    std::string_view mode_name(Mode mode) noexcept {
        switch (mode) {
        case Mode::boot:
            return "boot";
        case Mode::shutdown:
            return "shutdown";
        case Mode::reboot:
            return "reboot";
        case Mode::update:
            return "update";
        case Mode::upgrade:
            return "upgrade";
        }
        return "boot";
    }

    std::string state_json(const SplashState &state) {
        const auto progress = state.progress() ? std::to_string(*state.progress()) : std::string("null");
        return std::format("{{\"lifecycle\":\"{}\",\"view\":\"{}\",\"mode\":\"{}\",\"root_stage\":\"{}\",\"status\":{},"
                           "\"message\":{},\"progress\":{}}}",
                           lifecycle_name(state.lifecycle()), view_name(state.view()), mode_name(state.mode()),
                           root_stage_name(state.root_stage()), json_optional_string(state.status()),
                           json_optional_string(state.message()), progress);
    }

    CommandOutcome handle_request(SplashState &state, const Frame &request) {
        DeferredRootTransition transition;
        return handle_request(state, request, transition);
    }

    CommandOutcome handle_request(SplashState &state, const Frame &request, RootTransition &root_transition) {
        const auto request_id = request.request_id();
        if (request.opcode() == Opcode::ping) {
            return {Frame::pong(request_id), false, false, false};
        }
        if (request.opcode() == Opcode::state) {
            return {Frame::state_result(request_id, state_json(state)), false, false, false};
        }
        if (request.opcode() == Opcode::native_ready) {
            return {Frame::error(request_id, "native password broker is unavailable in this command context"), false,
                    false, false};
        }

        std::optional<StateAction> action;
        bool should_quit{};
        bool retain_splash{};
        switch (request.opcode()) {
        case Opcode::show:
            action = Show{};
            break;
        case Opcode::hide:
            action = Hide{};
            break;
        case Opcode::status: {
            const auto text = request.payload_text();
            action = SetStatus{text.empty() ? std::nullopt : std::optional{std::string(text)}};
            break;
        }
        case Opcode::progress:
            action = SetProgress{request.progress_value()};
            break;
        case Opcode::message:
            action = SetMessage{std::string(request.payload_text())};
            break;
        case Opcode::hide_message: {
            const auto requested = request.payload_text();
            if (requested.empty() || (state.message() && *state.message() == requested)) {
                action = SetMessage{std::nullopt};
            }
            break;
        }
        case Opcode::details_show:
            action = ShowDetails{};
            break;
        case Opcode::details_hide:
            action = HideDetails{};
            break;
        case Opcode::details_toggle:
            action = ToggleDetails{};
            break;
        case Opcode::deactivate:
            action = Deactivate{};
            break;
        case Opcode::reactivate:
            action = Reactivate{};
            break;
        case Opcode::set_mode:
            action = SetMode{*request.mode_value()};
            break;
        case Opcode::update_root_fs: {
            if (state.root_stage() == RootStage::initramfs) {
                try {
                    state.apply(SetRootStage{RootStage::switching});
                } catch (const StateError &error) {
                    try {
                        state.apply(FailOpen{});
                    } catch (...) {
                    }
                    auto outcome = state_error(request_id, error);
                    outcome.should_quit = true;
                    return outcome;
                }
            }
            if (state.root_stage() == RootStage::switching) {
                try {
                    root_transition.transition(std::filesystem::path(request.payload_text()));
                } catch (const std::exception &error) {
                    try {
                        state.apply(FailOpen{});
                    } catch (...) {
                    }
                    auto outcome = state_error(request_id, error);
                    outcome.should_quit = true;
                    outcome.fatal_root_transition = true;
                    return outcome;
                }
                action = SetRootStage{RootStage::real_root};
            }
            break;
        }
        case Opcode::quit:
            should_quit = true;
            retain_splash = request.retains_splash();
            action = Quit{};
            break;
        case Opcode::ack:
        case Opcode::error:
        case Opcode::pong:
        case Opcode::state_result:
            return {Frame::error(request_id, "response opcode is invalid in a request"), false, false, false};
        case Opcode::ping:
        case Opcode::state:
        case Opcode::native_ready:
            break;
        }
        if (action) {
            try {
                state.apply(std::move(*action));
            } catch (const StateError &error) {
                return state_error(request_id, error);
            }
        }
        return {Frame::ack(request_id), should_quit, retain_splash, false};
    }

} // namespace sart::splash
