#include "sart/core/cmdline.hpp"
#include "sart/core/process.hpp"
#include "sart/core/signals.hpp"
#include "sart/embedded/resources.hpp"
#include "sart/install/installer.hpp"
#include "sart/install/live.hpp"
#include "sart/password/native.hpp"
#include "sart/splash/client.hpp"
#include "sart/splash/daemon.hpp"
#include "sart/splash/protocol.hpp"
#include "sart/splash/runtime.hpp"
#include "sart/visual/art.hpp"
#include "sart/visual/renderer.hpp"
#include "sart/visual/terminal.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <sys/stat.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

    using sart::Art;

    struct VisualArguments {
        std::uint64_t duration_milliseconds{2500};
        std::uint64_t frames_per_second{30};
        std::uint64_t seed{42};
        bool no_color{};
        bool clear_first{};
        bool leave_final{};
        bool loop{};
        std::optional<std::filesystem::path> asset;
        std::optional<std::size_t> columns;
        std::optional<std::size_t> rows;
    };

    [[noreturn]] void usage_error(std::string_view message);

    template <typename Integer> Integer parse_integer(std::string_view value, std::string_view option);

    std::string_view required_value(const std::vector<std::string_view> &arguments, std::size_t &index,
                                    std::string_view option);

    std::filesystem::path extract_runtime_directory(std::vector<std::string_view> &arguments) {
        auto runtime = std::filesystem::path(sart::splash::default_runtime_directory);
        for (std::size_t index = 0; index < arguments.size();) {
            if (arguments[index] != "--runtime-dir") {
                ++index;
                continue;
            }
            if (index + 1 >= arguments.size()) {
                usage_error("--runtime-dir requires a path");
            }
            runtime = arguments[index + 1];
            arguments.erase(arguments.begin() + static_cast<std::ptrdiff_t>(index),
                            arguments.begin() + static_cast<std::ptrdiff_t>(index + 2));
        }
        return runtime;
    }

    sart::splash::Mode parse_mode(std::string_view value) {
        using sart::splash::Mode;
        if (value == "boot")
            return Mode::boot;
        if (value == "shutdown")
            return Mode::shutdown;
        if (value == "reboot")
            return Mode::reboot;
        if (value == "update")
            return Mode::update;
        if (value == "upgrade")
            return Mode::upgrade;
        usage_error("mode must be boot, shutdown, reboot, update, or upgrade");
    }

    sart::password::NativeAdapter parse_native_adapter(std::string_view value) {
        using sart::password::NativeAdapter;
        if (value == "dracut-classic")
            return NativeAdapter::dracut_classic;
        if (value == "initramfs-tools-busybox")
            return NativeAdapter::initramfs_tools_busybox;
        if (value == "mkinitfs-busybox")
            return NativeAdapter::mkinitfs_busybox;
        if (value == "mkinitfs-boot-deploy")
            return NativeAdapter::mkinitfs_boot_deploy;
        if (value == "mkinitcpio-busybox")
            return NativeAdapter::mkinitcpio_busybox;
        usage_error("unsupported native askpass adapter");
    }

    int run_control(std::filesystem::path runtime, const sart::splash::Frame &request) {
        const sart::splash::ClientConfig config(sart::splash::RuntimePaths(std::move(runtime)));
        const auto response = sart::splash::send_request(config, request);
        using sart::splash::Opcode;
        switch (response.opcode()) {
        case Opcode::ack:
            return 0;
        case Opcode::pong:
            std::cout << "pong\n";
            return 0;
        case Opcode::state_result:
            std::cout << response.payload_text() << '\n';
            return 0;
        case Opcode::error:
            std::cerr << "Daemon rejected request: " << response.payload_text() << '\n';
            return 1;
        default:
            std::cerr << "Unexpected daemon response\n";
            return 1;
        }
    }

    int run_control_command(std::string_view command, std::vector<std::string_view> arguments) {
        using namespace sart::splash;
        const auto runtime = extract_runtime_directory(arguments);
        const auto request_id = next_request_id();
        if (command == "show" || command == "hide" || command == "deactivate" || command == "reactivate" ||
            command == "ping") {
            if (!arguments.empty())
                usage_error("control command accepts no positional arguments");
            const auto opcode = command == "show"         ? Opcode::show
                                : command == "hide"       ? Opcode::hide
                                : command == "deactivate" ? Opcode::deactivate
                                : command == "reactivate" ? Opcode::reactivate
                                                          : Opcode::ping;
            return run_control(runtime, Frame::empty(opcode, request_id));
        }
        if (command == "status" || command == "hide-message") {
            if (arguments.size() > 1)
                usage_error("control command accepts at most one text value");
            const auto text = arguments.empty() ? std::string_view{} : arguments.front();
            return run_control(
                runtime, Frame::text(command == "status" ? Opcode::status : Opcode::hide_message, request_id, text));
        }
        if (command == "message") {
            if (arguments.size() != 1)
                usage_error("message requires exactly one text value");
            return run_control(runtime, Frame::text(Opcode::message, request_id, arguments.front()));
        }
        if (command == "progress") {
            if (arguments.size() != 1)
                usage_error("progress requires a percent");
            const auto percent = parse_integer<unsigned>(arguments.front(), "progress");
            if (percent > 100)
                usage_error("progress must be between 0 and 100");
            return run_control(runtime, Frame::progress(request_id, static_cast<std::uint8_t>(percent)));
        }
        if (command == "details") {
            if (arguments.size() != 1)
                usage_error("details requires show, hide, or toggle");
            const auto opcode = arguments.front() == "show"     ? Opcode::details_show
                                : arguments.front() == "hide"   ? Opcode::details_hide
                                : arguments.front() == "toggle" ? Opcode::details_toggle
                                                                : throw std::invalid_argument("invalid details action");
            return run_control(runtime, Frame::empty(opcode, request_id));
        }
        if (command == "mode") {
            if (arguments.size() != 1)
                usage_error("mode requires one presentation mode");
            return run_control(runtime, Frame::mode(request_id, parse_mode(arguments.front())));
        }
        if (command == "state") {
            if (arguments.size() != 1 || arguments.front() != "--json") {
                usage_error("state requires --json");
            }
            return run_control(runtime, Frame::empty(Opcode::state, request_id));
        }
        if (command == "quit") {
            if (arguments.size() > 1 || (!arguments.empty() && arguments.front() != "--retain-splash")) {
                usage_error("quit accepts only --retain-splash");
            }
            return run_control(runtime, Frame::quit(request_id, !arguments.empty()));
        }
        if (command == "update-root-fs") {
            if (arguments.size() != 1)
                usage_error("update-root-fs requires an absolute path");
            return run_control(runtime, Frame::text(Opcode::update_root_fs, request_id, arguments.front()));
        }
        usage_error("unsupported control command");
    }

    int run_daemon_command(std::vector<std::string_view> arguments) {
        auto runtime = extract_runtime_directory(arguments);
        auto mode = sart::splash::Mode::boot;
        bool test_buffer{};
        auto cmdline = std::filesystem::path(sart::cmdline::proc_cmdline);
        auto display = sart::TextVtConfig::open_query();
        auto engine = sart::splash::EngineConfig{};
        auto password_broker = sart::splash::PasswordBroker::none;
        for (std::size_t index = 0; index < arguments.size(); ++index) {
            if (arguments[index] == "--test-buffer") {
                test_buffer = true;
            } else if (arguments[index] == "--mode") {
                mode = parse_mode(required_value(arguments, index, arguments[index]));
            } else if (arguments[index] == "--cmdline") {
                cmdline = required_value(arguments, index, arguments[index]);
            } else if (arguments[index] == "--tty") {
                const auto value = required_value(arguments, index, arguments[index]);
                constexpr std::string_view prefix = "/dev/tty";
                if (!value.starts_with(prefix) || value.size() == prefix.size()) {
                    usage_error("TTY must have the exact form /dev/ttyN");
                }
                const auto number = parse_integer<std::uint16_t>(value.substr(prefix.size()), "--tty");
                if (std::string(prefix) + std::to_string(number) != value) {
                    usage_error("TTY must use its canonical /dev/ttyN path");
                }
                display = sart::TextVtConfig::configured(number);
            } else if (arguments[index] == "--fps") {
                engine.frames_per_second =
                    parse_integer<std::uint16_t>(required_value(arguments, index, "--fps"), "--fps");
            } else if (arguments[index] == "--cycle-ms") {
                engine.animation_cycle = std::chrono::milliseconds(
                    parse_integer<std::uint64_t>(required_value(arguments, index, "--cycle-ms"), "--cycle-ms"));
            } else if (arguments[index] == "--seed") {
                engine.seed = parse_integer<std::uint64_t>(required_value(arguments, index, "--seed"), "--seed");
            } else if (arguments[index] == "--no-color") {
                engine.no_color = true;
            } else if (arguments[index] == "--password-broker") {
                const auto broker = required_value(arguments, index, "--password-broker");
                if (broker == "none")
                    password_broker = sart::splash::PasswordBroker::none;
                else if (broker == "systemd")
                    password_broker = sart::splash::PasswordBroker::systemd;
                else if (broker == "native")
                    password_broker = sart::splash::PasswordBroker::native;
                else
                    usage_error("password broker must be none, systemd, or native");
            } else {
                usage_error("unsupported daemon option");
            }
        }
        if (test_buffer && display.selection().kind == sart::VtSelectionKind::configured) {
            usage_error("--test-buffer conflicts with --tty");
        }
        engine.validate();
        sart::signals::reset_stop_flag();
        sart::signals::SignalGuard signal_guard;
        sart::splash::DaemonConfig config{sart::splash::RuntimePaths(std::move(runtime)), mode, test_buffer};
        config.cmdline = std::move(cmdline);
        config.display = display;
        config.engine = engine;
        config.password_broker = password_broker;
        sart::splash::run_daemon(config);
        return 0;
    }

    bool fixed_vt_paths_are_ready() {
        struct stat control{};
        struct stat udev{};
        return lstat("/dev/tty0", &control) == 0 && S_ISCHR(control.st_mode) &&
               lstat("/run/udev/control", &udev) == 0 && S_ISSOCK(udev.st_mode);
    }

    bool wait_for_vt_readiness(std::chrono::milliseconds timeout) {
        const auto deadline = std::chrono::steady_clock::now() + timeout;
        do {
            if (fixed_vt_paths_are_ready())
                return true;
            if (std::chrono::steady_clock::now() >= deadline)
                return false;
            std::this_thread::sleep_for(std::chrono::milliseconds(25));
        } while (true);
    }

    [[noreturn]] void usage_error(std::string_view message) {
        std::cerr << "sart: " << message << '\n';
        std::cerr << "Try 'sart --help' for more information.\n";
        std::exit(2);
    }

    template <typename Integer> Integer parse_integer(std::string_view value, std::string_view option) {
        Integer result{};
        const auto [end, error] = std::from_chars(value.data(), value.data() + value.size(), result);
        if (error != std::errc{} || end != value.data() + value.size()) {
            usage_error(std::string(option) + " requires an integer");
        }
        return result;
    }

    std::string read_asset(const std::optional<std::filesystem::path> &path) {
        if (!path) {
            return std::string(sart::embedded::default_art);
        }
        std::ifstream input(*path, std::ios::binary);
        if (!input) {
            throw std::runtime_error("cannot open asset: " + path->string());
        }
        std::string result;
        result.reserve(4096);
        char buffer[8192];
        while (input) {
            input.read(buffer, sizeof buffer);
            result.append(buffer, static_cast<std::size_t>(input.gcount()));
            if (result.size() > sart::max_art_bytes) {
                throw std::runtime_error("asset exceeds maximum size");
            }
        }
        if (!input.eof()) {
            throw std::runtime_error("cannot read asset: " + path->string());
        }
        return result;
    }

    std::string_view required_value(const std::vector<std::string_view> &arguments, std::size_t &index,
                                    std::string_view option) {
        if (++index >= arguments.size()) {
            usage_error(std::string(option) + " requires a value");
        }
        return arguments[index];
    }

    VisualArguments parse_visual_arguments(const std::vector<std::string_view> &arguments, bool preview, bool final) {
        VisualArguments result;
        result.clear_first = !preview || final;
        result.leave_final = !preview || final;
        for (std::size_t index = 0; index < arguments.size(); ++index) {
            const auto argument = arguments[index];
            if (argument == "--no-color") {
                result.no_color = true;
            } else if (argument == "--clear-first" && !final) {
                result.clear_first = true;
            } else if (argument == "--leave-final" && !final) {
                result.leave_final = true;
            } else if (argument == "--loop" && preview) {
                result.loop = true;
            } else if (argument == "--duration-ms" && !final) {
                result.duration_milliseconds =
                    parse_integer<std::uint64_t>(required_value(arguments, index, argument), argument);
            } else if (argument == "--fps" && !final) {
                result.frames_per_second =
                    parse_integer<std::uint64_t>(required_value(arguments, index, argument), argument);
            } else if (argument == "--seed" && !final) {
                result.seed = parse_integer<std::uint64_t>(required_value(arguments, index, argument), argument);
            } else if (argument == "--asset") {
                result.asset = std::filesystem::path(required_value(arguments, index, argument));
            } else if (argument == "--cols") {
                result.columns = parse_integer<std::size_t>(required_value(arguments, index, argument), argument);
            } else if (argument == "--rows") {
                result.rows = parse_integer<std::size_t>(required_value(arguments, index, argument), argument);
            } else {
                usage_error("unknown visual option: " + std::string(argument));
            }
        }
        return result;
    }

    void print_help() {
        std::cout << R"HELP(Minimal Linux ASCII boot animation

Usage: sart [COMMAND]

Commands:
  early-boot-enabled  Check the kernel splash bypass tokens
  console-fallback-needed  Check whether the stock console agent is needed
  vt-ready            Wait for Linux VT and udev readiness
  play                Play animation once in terminal
  preview             Interactive or infinite preview
  render-final        Render final static state
  validate            Validate an ASCII logo file
  daemon              Run the foreground splash daemon
  install             Plan and manage the guarded host installation
  show, hide          Control splash visibility
  status, progress    Update boot status
  message             Show a message
  details             Control detailed output
  state, ping, quit   Inspect or stop the daemon
  help                Print help

Options:
  -h, --help     Print help
  -V, --version  Print version
)HELP";
    }

    void print_install_help() {
        std::cout << R"HELP(Guarded live-root installer

Usage: sart install <COMMAND>

Commands:
  plan       Inspect the exact live capability contract
  status     Inspect installed files and recovery state
  apply      Apply the exact live capability contract
  recover    Recover an interrupted transaction
  uninstall  Restore installer preimages
)HELP";
    }

    void print_install_action_help(std::string_view action) {
        if (action == "plan") {
            std::cout << "Usage: sart install plan [--json]\n";
        } else if (action == "status") {
            std::cout << "Usage: sart install status\n";
        } else if (action == "apply" || action == "recover" || action == "uninstall") {
            std::cout << "Usage: sart install " << action << " --confirm-host HOSTNAME\n";
        } else {
            usage_error("unknown installer command: " + std::string(action));
        }
    }

    int run_install_command(std::vector<std::string_view> arguments) {
        if (arguments.empty() || arguments.front() == "--help" || arguments.front() == "-h") {
            print_install_help();
            return 0;
        }
        const auto action = arguments.front();
        arguments.erase(arguments.begin());
        if (!arguments.empty() && (arguments.front() == "--help" || arguments.front() == "-h")) {
            if (arguments.size() != 1)
                usage_error("unexpected argument '" + std::string(arguments[1]) + "'");
            print_install_action_help(action);
            return 0;
        }
        if (action == "status") {
            if (!arguments.empty())
                usage_error("unexpected argument '" + std::string(arguments.front()) + "'");
            std::cout << sart::install::render_status(sart::install::Installer::live_root_read_only().status());
            return 0;
        }
        if (action == "plan") {
            bool json = false;
            for (const auto argument : arguments) {
                if (argument == "--json" && !json)
                    json = true;
                else
                    usage_error("unexpected argument '" + std::string(argument) + "'");
            }
            const auto discovery = sart::install::discover_exact_install_contract();
            const auto plan = sart::install::build_exact_self_install_plan(discovery);
            std::cout << sart::install::render_exact_install_plan(plan, discovery, json);
            return 0;
        }
        if (action == "apply" || action == "recover" || action == "uninstall") {
            std::optional<std::string_view> confirmation;
            bool package_hook = false;
            for (std::size_t index = 0; index < arguments.size(); ++index) {
                if (arguments[index] == "--confirm-host" && !confirmation) {
                    confirmation = required_value(arguments, index, "--confirm-host");
                } else if (action == "apply" && arguments[index] == "--package-hook" && !package_hook) {
                    package_hook = true;
                } else {
                    usage_error("unexpected argument '" + std::string(arguments[index]) + "'");
                }
            }
            if (!confirmation)
                usage_error("--confirm-host is required");
            auto installer = sart::install::Installer::live_root_mutating(*confirmation, package_hook);
            if (action == "recover") {
                const auto outcome = installer.recover();
                std::cout << "sart install recover: "
                          << (outcome == sart::install::RecoveryOutcome::no_transaction ? "nothing-to-recover"
                              : outcome == sart::install::RecoveryOutcome::rolled_back  ? "rolled-back"
                                                                                        : "completed-commit-cleaned")
                          << '\n';
                return 0;
            }
            const auto discovery = sart::install::discover_exact_install_contract();
            if (action == "apply") {
                const auto plan = sart::install::build_exact_self_install_plan(discovery);
                const auto outcome = installer.apply_exact(plan, discovery);
                std::cout << "sart install apply: "
                          << (outcome == sart::install::ApplyOutcome::installed   ? "installed"
                              : outcome == sart::install::ApplyOutcome::refreshed ? "refreshed"
                                                                                  : "already-current")
                          << '\n';
                return 0;
            }
            const auto report = installer.uninstall(&discovery);
            std::cout << "sart install uninstall: removed=" << report.removed << " restored=" << report.restored
                      << " preserved-modified=0 preserved-directories=" << report.preserved_directories.size() << '\n';
            return 0;
        }
        usage_error("unknown installer command: " + std::string(action));
    }

    int run_visual(std::string_view command, const std::vector<std::string_view> &arguments) {
        const auto final = command == "render-final";
        const auto preview = command == "preview";
        const auto options = parse_visual_arguments(arguments, preview, final);
        const auto art = Art::parse(read_asset(options.asset));
        const auto small =
            options.asset ? std::optional<Art>{} : std::optional<Art>{Art::parse(sart::embedded::small_art)};
        sart::StdoutTerminal terminal(options.columns, options.rows);
        sart::signals::SignalGuard signal_guard;
        if (final) {
            sart::render_final(terminal, art, small ? &*small : nullptr, options.no_color);
            return 0;
        }
        sart::RenderOptions render_options{
            options.duration_milliseconds, options.frames_per_second, options.seed, options.no_color,
            options.clear_first,           options.leave_final,
        };
        std::size_t iteration{};
        do {
            sart::play_animation(terminal, art, small ? &*small : nullptr, render_options, iteration++);
        } while (preview && options.loop && !sart::signals::should_stop());
        return 0;
    }

    int run(int argc, char **argv) {
        std::vector<std::string_view> arguments;
        arguments.reserve(static_cast<std::size_t>(argc > 1 ? argc - 1 : 0));
        for (int index = 1; index < argc; ++index) {
            arguments.emplace_back(argv[index]);
        }
        if (arguments.empty()) {
            return run_visual("play", {});
        }
        if (arguments.front() == "--help" || arguments.front() == "-h" || arguments.front() == "help") {
            print_help();
            return 0;
        }
        if (arguments.front() == "--version" || arguments.front() == "-V") {
            std::cout << "sart " SART_VERSION "\n";
            return 0;
        }
        const auto command = arguments.front();
        arguments.erase(arguments.begin());
        if (command == "play" || command == "preview" || command == "render-final") {
            return run_visual(command, arguments);
        }
        if (command == "daemon") {
            return run_daemon_command(std::move(arguments));
        }
        if (command == "install") {
            return run_install_command(std::move(arguments));
        }
        if (command == "show" || command == "hide" || command == "status" || command == "progress" ||
            command == "message" || command == "hide-message" || command == "details" || command == "deactivate" ||
            command == "reactivate" || command == "mode" || command == "state" || command == "quit" ||
            command == "update-root-fs" || command == "ping") {
            return run_control_command(command, std::move(arguments));
        }
        if (command == "validate") {
            const auto options = parse_visual_arguments(arguments, false, true);
            const auto art = Art::parse(read_asset(options.asset));
            std::cout << "Logo asset is valid! Dimensions: " << art.width() << 'x' << art.height() << '\n';
            return 0;
        }
        if (command == "early-boot-enabled") {
            std::filesystem::path path(sart::cmdline::proc_cmdline);
            if (!arguments.empty()) {
                if (arguments.size() != 2 || arguments[0] != "--cmdline") {
                    usage_error("early-boot-enabled accepts only --cmdline PATH");
                }
                path = arguments[1];
            }
            return sart::cmdline::early_boot_enabled_at(path) ? 0 : 1;
        }
        if (command == "console-fallback-needed") {
            auto runtime = extract_runtime_directory(arguments);
            std::uint64_t wait_ms = 5000;
            if (!arguments.empty()) {
                if (arguments.size() != 2 || arguments[0] != "--wait-ms") {
                    usage_error("console-fallback-needed accepts --runtime-dir and --wait-ms");
                }
                wait_ms = parse_integer<std::uint64_t>(arguments[1], "--wait-ms");
            }
            if (wait_ms < 100 || wait_ms > 10'000)
                usage_error("--wait-ms must be in 100..=10000");
            const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(wait_ms);
            do {
                try {
                    auto config = sart::splash::ClientConfig(sart::splash::RuntimePaths(runtime));
                    config.timeout = std::chrono::milliseconds(100);
                    const auto response =
                        sart::splash::send_request(config, sart::splash::Frame::empty(sart::splash::Opcode::ping,
                                                                                      sart::splash::next_request_id()));
                    if (response.opcode() == sart::splash::Opcode::pong)
                        return 1;
                } catch (...) {
                }
                if (std::chrono::steady_clock::now() >= deadline)
                    return 0;
                std::this_thread::sleep_for(std::chrono::milliseconds(50));
            } while (true);
        }
        if (command == "vt-ready") {
            std::uint64_t wait_ms = 3000;
            if (!arguments.empty()) {
                if (arguments.size() != 2 || arguments[0] != "--wait-ms") {
                    usage_error("vt-ready accepts only --wait-ms");
                }
                wait_ms = parse_integer<std::uint64_t>(arguments[1], "--wait-ms");
            }
            if (wait_ms < 100 || wait_ms > 5000)
                usage_error("--wait-ms must be in 100..=5000");
            return wait_for_vt_readiness(std::chrono::milliseconds(wait_ms)) ? 0 : 1;
        }
        if (command == "native-ready") {
            const auto runtime = extract_runtime_directory(arguments);
            if (!arguments.empty())
                usage_error("native-ready accepts only --runtime-dir");
            try {
                const auto response = sart::splash::send_request(
                    sart::splash::ClientConfig(sart::splash::RuntimePaths(runtime)),
                    sart::splash::Frame::empty(sart::splash::Opcode::native_ready, sart::splash::next_request_id()));
                return response.opcode() == sart::splash::Opcode::ack ? 0 : 75;
            } catch (...) {
                return 75;
            }
        }
        if (command == "native-askpass") {
            std::optional<sart::password::NativeAdapter> adapter;
            std::optional<std::string> prompt;
            std::uint16_t attempts = 1;
            std::size_t maximum_secret_bytes = 1024;
            for (std::size_t index = 0; index < arguments.size(); ++index) {
                if (arguments[index] == "--adapter") {
                    adapter = parse_native_adapter(required_value(arguments, index, "--adapter"));
                } else if (arguments[index] == "--prompt") {
                    prompt = std::string(required_value(arguments, index, "--prompt"));
                } else if (arguments[index] == "--attempts") {
                    attempts =
                        parse_integer<std::uint16_t>(required_value(arguments, index, "--attempts"), "--attempts");
                } else if (arguments[index] == "--maximum-secret-bytes") {
                    maximum_secret_bytes = parse_integer<std::size_t>(
                        required_value(arguments, index, "--maximum-secret-bytes"), "--maximum-secret-bytes");
                } else {
                    usage_error("unsupported native-askpass option");
                }
            }
            if (!adapter || !prompt)
                usage_error("native-askpass requires --adapter and --prompt");
            try {
                const auto outcome = sart::password::run_native_askpass_client(
                    *adapter, {*prompt, attempts, maximum_secret_bytes}, sart::password::claim_native_askpass_output());
                if (outcome == sart::password::NativeAskpassOutcome::delivered)
                    return 0;
                if (outcome == sart::password::NativeAskpassOutcome::user_cancelled) {
                    return sart::password::native_askpass_cancelled_exit_code;
                }
                return sart::password::native_askpass_transport_exit_code;
            } catch (...) {
                return sart::password::native_askpass_transport_exit_code;
            }
        }
        usage_error("unknown command: " + std::string(command));
    }

} // namespace

int main(int argc, char **argv) {
    if (!sart::process_is_allowed(static_cast<std::uint32_t>(getpid()))) {
        std::cerr << "sart refuses to run as PID 1\n";
        return sart::pid1_refusal_exit_code;
    }
    try {
        return run(argc, argv);
    } catch (const std::exception &error) {
        std::cerr << "sart: " << error.what() << '\n';
        return 1;
    }
}
