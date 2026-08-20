#include "sart/installer_backends.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstring>
#include <fcntl.h>
#include <future>
#include <stdexcept>
#include <sys/stat.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>

namespace sart::install {
    namespace {

        class UniqueFd {
          public:
            explicit UniqueFd(int fd = -1) noexcept : fd_(fd) {}
            ~UniqueFd() {
                if (fd_ >= 0)
                    ::close(fd_);
            }
            UniqueFd(const UniqueFd &) = delete;
            UniqueFd &operator=(const UniqueFd &) = delete;
            UniqueFd(UniqueFd &&other) noexcept : fd_(other.fd_) { other.fd_ = -1; }
            UniqueFd &operator=(UniqueFd &&other) noexcept {
                if (this != &other) {
                    if (fd_ >= 0)
                        ::close(fd_);
                    fd_ = other.fd_;
                    other.fd_ = -1;
                }
                return *this;
            }
            [[nodiscard]] int get() const noexcept { return fd_; }
            [[nodiscard]] int release() noexcept {
                const int result = fd_;
                fd_ = -1;
                return result;
            }

          private:
            int fd_;
        };

        std::runtime_error system_error(std::string_view operation) {
            return std::runtime_error(std::string(operation) + ": " + std::strerror(errno));
        }

        std::chrono::seconds timeout(GeneratorKind kind) {
            switch (kind) {
            case GeneratorKind::dracut:
            case GeneratorKind::initramfs_tools:
            case GeneratorKind::mkinitcpio:
            case GeneratorKind::mkinitfs:
            case GeneratorKind::mkinitfs_boot_deploy:
                return std::chrono::seconds(300);
            case GeneratorKind::initramfs_inspection:
            case GeneratorKind::grub_update:
            case GeneratorKind::extlinux_update:
                return std::chrono::seconds(120);
            default:
                return std::chrono::seconds(30);
            }
        }

        std::future<std::vector<std::byte>> drain(int descriptor) {
            return std::async(std::launch::async, [descriptor] {
                UniqueFd input(descriptor);
                std::vector<std::byte> retained;
                std::array<std::byte, 8192> buffer{};
                while (true) {
                    const auto count = ::read(input.get(), buffer.data(), buffer.size());
                    if (count < 0 && errno == EINTR)
                        continue;
                    if (count < 0)
                        throw system_error("read generator output");
                    if (count == 0)
                        break;
                    if (retained.size() <= max_generator_output_bytes) {
                        const auto remaining = max_generator_output_bytes + 1 - retained.size();
                        retained.insert(retained.end(), buffer.begin(),
                                        buffer.begin() +
                                            static_cast<std::ptrdiff_t>(std::min<std::size_t>(count, remaining)));
                    }
                }
                return retained;
            });
        }

        bool validates(const GeneratorRequest &request, void (*validator)(const GeneratorRequest &)) {
            try {
                validator(request);
                return true;
            } catch (const std::runtime_error &) {
                return false;
            }
        }

    } // namespace

    std::string_view generator_kind_name(GeneratorKind kind) {
        switch (kind) {
        case GeneratorKind::dracut:
            return "dracut";
        case GeneratorKind::initramfs_inspection:
            return "initramfs_inspection";
        case GeneratorKind::grub_update:
            return "grub_update";
        case GeneratorKind::extlinux_update:
            return "extlinux_update";
        case GeneratorKind::initramfs_tools:
            return "initramfs_tools";
        case GeneratorKind::mkinitcpio:
            return "mkinitcpio";
        case GeneratorKind::mkinitfs:
            return "mkinitfs";
        case GeneratorKind::mkinitfs_boot_deploy:
            return "mkinitfs_boot_deploy";
        case GeneratorKind::systemd_reload:
            return "systemd_reload";
        case GeneratorKind::openrc_runlevel:
            return "openrc_runlevel";
        }
        throw std::runtime_error("unknown generator kind");
    }

    void validate_supported_generator_request(const GeneratorRequest &request) {
        if (validates(request, validate_dracut_systemd_generator_request) ||
            validates(request, validate_initramfs_tools_systemd_generator_request) ||
            validates(request, validate_mkinitcpio_systemd_generator_request) ||
            validates(request, validate_mkinitfs_openrc_generator_request) ||
            validates(request, validate_mkinitfs_boot_deploy_generator_request))
            return;
        throw std::runtime_error("generator request is outside every implemented mechanism contract");
    }

    CommandOutput run_generator(const GeneratorRequest &request) {
        validate_supported_generator_request(request);
        if (request.alternate_root != "/")
            throw std::runtime_error("generator runner accepts only the live root");
        UniqueFd executable(::open(request.executable.c_str(), O_PATH | O_NOFOLLOW | O_CLOEXEC));
        if (executable.get() < 0)
            throw system_error("open approved generator executable");
        struct stat executable_stat{};
        if (::fstat(executable.get(), &executable_stat) != 0)
            throw system_error("inspect generator executable");
        if (!S_ISREG(executable_stat.st_mode) || executable_stat.st_uid != 0 || executable_stat.st_nlink != 1 ||
            (executable_stat.st_mode & 0111) == 0 || (executable_stat.st_mode & 0022) != 0) {
            throw std::runtime_error("generator executable failed descriptor metadata checks");
        }
        const int descriptor_flags = ::fcntl(executable.get(), F_GETFD);
        if (descriptor_flags < 0 || ::fcntl(executable.get(), F_SETFD, descriptor_flags & ~FD_CLOEXEC) < 0) {
            throw system_error("retain generator executable descriptor");
        }
        std::vector<std::string> arguments;
        arguments.reserve(request.arguments.size() + 1);
        arguments.push_back(request.executable);
        arguments.insert(arguments.end(), request.arguments.begin(), request.arguments.end());
        std::vector<char *> argv;
        argv.reserve(arguments.size() + 1);
        for (auto &argument : arguments) {
            if (argument.contains('\0'))
                throw std::runtime_error("generator argument contains NUL");
            argv.push_back(argument.data());
        }
        argv.push_back(nullptr);
        std::array environment_storage{std::string("PATH=/usr/sbin:/usr/bin:/sbin:/bin"), std::string("LANG=C"),
                                       std::string("LC_ALL=C"), std::string("HOME=/root")};
        std::array<char *, 5> environment{environment_storage[0].data(), environment_storage[1].data(),
                                          environment_storage[2].data(), environment_storage[3].data(), nullptr};

        UniqueFd working_directory;
        struct stat working_before{};
        if (request.working_directory) {
            working_directory =
                UniqueFd(::open(request.working_directory->c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
            if (working_directory.get() < 0 || ::fstat(working_directory.get(), &working_before) != 0) {
                throw system_error("open generator working directory");
            }
            if (!S_ISDIR(working_before.st_mode) || working_before.st_uid != 0 ||
                (working_before.st_mode & 07777) != 0700) {
                throw std::runtime_error("generator working directory is not private and root-owned");
            }
        }

        int stdout_pipe[2]{-1, -1};
        int stderr_pipe[2]{-1, -1};
        if (::pipe2(stdout_pipe, O_CLOEXEC) != 0)
            throw system_error("create generator stdout pipe");
        UniqueFd stdout_read(stdout_pipe[0]), stdout_write(stdout_pipe[1]);
        if (::pipe2(stderr_pipe, O_CLOEXEC) != 0)
            throw system_error("create generator stderr pipe");
        UniqueFd stderr_read(stderr_pipe[0]), stderr_write(stderr_pipe[1]);
        const pid_t child = ::fork();
        if (child < 0)
            throw system_error("fork generator");
        if (child == 0) {
            if (::setpgid(0, 0) != 0 || ::dup2(stdout_write.get(), STDOUT_FILENO) < 0 ||
                ::dup2(stderr_write.get(), STDERR_FILENO) < 0)
                _exit(126);
            const int null_input = ::open("/dev/null", O_RDONLY | O_CLOEXEC);
            if (null_input < 0 || ::dup2(null_input, STDIN_FILENO) < 0)
                _exit(126);
            ::close(null_input);
            ::close(stdout_read.get());
            ::close(stdout_write.get());
            ::close(stderr_read.get());
            ::close(stderr_write.get());
            if (working_directory.get() >= 0 && ::fchdir(working_directory.get()) != 0)
                _exit(126);
            ::fexecve(executable.get(), argv.data(), environment.data());
            _exit(127);
        }
        stdout_write = UniqueFd();
        stderr_write = UniqueFd();
        auto standard_output = drain(stdout_read.release());
        auto standard_error = drain(stderr_read.release());
        const auto deadline = std::chrono::steady_clock::now() + timeout(request.generator);
        int raw_status = 0;
        bool timed_out = false;
        while (true) {
            const auto waited = ::waitpid(child, &raw_status, WNOHANG);
            if (waited == child)
                break;
            if (waited < 0) {
                ::kill(-child, SIGTERM);
                std::this_thread::sleep_for(std::chrono::milliseconds(500));
                ::kill(-child, SIGKILL);
                ::waitpid(child, &raw_status, 0);
                throw system_error("wait for generator process group");
            }
            if (std::chrono::steady_clock::now() >= deadline) {
                timed_out = true;
                ::kill(-child, SIGTERM);
                std::this_thread::sleep_for(std::chrono::milliseconds(500));
                ::kill(-child, SIGKILL);
                if (::waitpid(child, &raw_status, 0) != child)
                    throw system_error("reap timed-out generator");
                break;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
        }
        if (working_directory.get() >= 0) {
            if (::fchmod(working_directory.get(), 0700) != 0 || ::fsync(working_directory.get()) != 0) {
                throw system_error("secure generator working directory");
            }
            struct stat after{};
            if (::lstat(request.working_directory->c_str(), &after) != 0 || !S_ISDIR(after.st_mode) ||
                after.st_uid != 0 || (after.st_mode & 07777) != 0700 || after.st_dev != working_before.st_dev ||
                after.st_ino != working_before.st_ino) {
                throw std::runtime_error("generator working directory changed identity");
            }
        }
        auto output = standard_output.get();
        auto error = standard_error.get();
        if (timed_out)
            throw std::runtime_error("generator timed out");
        if (output.size() > max_generator_output_bytes || error.size() > max_generator_output_bytes) {
            throw std::runtime_error("generator output exceeded its bound");
        }
        const int status = WIFEXITED(raw_status) ? WEXITSTATUS(raw_status)
                                                 : (WIFSIGNALED(raw_status) ? 128 + WTERMSIG(raw_status) : 255);
        return {status, std::move(output), std::move(error)};
    }

} // namespace sart::install
