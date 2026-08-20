#include "sart/adapter.hpp"
#include "sart/animation.hpp"
#include "sart/art.hpp"
#include "sart/cmdline.hpp"
#include "sart/display.hpp"
#include "sart/display_buffer.hpp"
#include "sart/display_text_vt.hpp"
#include "sart/embedded.hpp"
#include "sart/frame_engine.hpp"
#include "sart/installer.hpp"
#include "sart/installer_backends.hpp"
#include "sart/integration.hpp"
#include "sart/integration_patch.hpp"
#include "sart/integration_resources.hpp"
#include "sart/password_coordinator.hpp"
#include "sart/password_input.hpp"
#include "sart/password_native.hpp"
#include "sart/password_secure.hpp"
#include "sart/password_systemd.hpp"
#include "sart/process.hpp"
#include "sart/renderer.hpp"
#include "sart/sha256.hpp"
#include "sart/splash/client.hpp"
#include "sart/splash/command.hpp"
#include "sart/splash/engine.hpp"
#include "sart/splash/protocol.hpp"
#include "sart/splash/runtime.hpp"
#include "sart/splash/state.hpp"
#include "sart/terminal.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <set>
#include <stdexcept>
#include <string>
#include <string_view>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <utility>
#include <vector>
#include <zlib.h>
#include <zstd.h>

namespace {

    struct TestFailure final : std::runtime_error {
        using std::runtime_error::runtime_error;
    };

#define CHECK(...)                                                                                                     \
    do {                                                                                                               \
        if (!(__VA_ARGS__)) {                                                                                          \
            throw TestFailure(std::string("check failed: ") + #__VA_ARGS__);                                           \
        }                                                                                                              \
    } while (false)

    template <typename Function> void check_art_error(sart::ArtErrorCode expected, Function &&function) {
        try {
            function();
        } catch (const sart::ArtError &error) {
            CHECK(error.code() == expected);
            return;
        }
        throw TestFailure("expected ArtError");
    }

    std::vector<std::byte> static_elf_fixture() {
        std::vector<std::byte> elf(120);
        const auto put = [&elf](std::size_t offset, std::uint64_t value, std::size_t width) {
            for (std::size_t index = 0; index < width; ++index) {
                elf[offset + index] = std::byte(static_cast<unsigned char>(value >> (index * 8)));
            }
        };
        elf[0] = std::byte{0x7f};
        elf[1] = std::byte{'E'};
        elf[2] = std::byte{'L'};
        elf[3] = std::byte{'F'};
        elf[4] = std::byte{2};
        elf[5] = std::byte{1};
        elf[6] = std::byte{1};
        put(16, 2, 2);
#if defined(__x86_64__)
        put(18, 62, 2);
#elif defined(__aarch64__)
        put(18, 183, 2);
#endif
        put(20, 1, 4);
        put(24, 0x400040, 8);
        put(32, 64, 8);
        put(52, 64, 2);
        put(54, 56, 2);
        put(56, 1, 2);
        put(64, 1, 4);
        put(68, 5, 4);
        put(72, 0, 8);
        put(80, 0x400000, 8);
        put(96, elf.size(), 8);
        put(104, elf.size(), 8);
        return elf;
    }

    std::string read_file(const std::filesystem::path &path) {
        std::ifstream input(path, std::ios::binary);
        if (!input) {
            throw TestFailure("cannot read fixture " + path.string());
        }
        return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
    }

    void art_validation_and_layout() {
        const auto art = sart::Art::parse("  foo  \r\n  bar\n");
        CHECK(art.width() == 5);
        CHECK(art.height() == 2);
        CHECK(art.cell(0, 0) == U' ');
        CHECK(art.cell(2, 0) == U'f');

        const auto unicode = sart::Art::parse(" ▄▄██ \n ▀▀██ ");
        CHECK(unicode.width() == 5);
        CHECK(unicode.cell(1, 0) == U'▄');
        check_art_error(sart::ArtErrorCode::empty, [] { sart::Art::parse(""); });
        check_art_error(sart::ArtErrorCode::no_visible_characters, [] { sart::Art::parse("   \n  \n"); });
        check_art_error(sart::ArtErrorCode::contains_tab, [] { sart::Art::parse("hello\tworld"); });
        check_art_error(sart::ArtErrorCode::contains_standalone_carriage_return,
                        [] { sart::Art::parse("hello\rworld"); });
        check_art_error(sart::ArtErrorCode::contains_control, [] { sart::Art::parse("left\xE2\x80\xAEright"); });

        const auto centered = sart::layout({10, 4}, {80, 24});
        CHECK(centered == sart::Layout{0, 0, 10, 4, 35, 10});
        const auto cropped = sart::layout({100, 30}, {80, 24});
        CHECK(cropped == sart::Layout{10, 3, 80, 24, 0, 0});
    }

    void animation_is_deterministic() {
        CHECK(sart::cell_hash(42, 10, 5) == sart::cell_hash(42, 10, 5));
        CHECK(sart::cell_hash(42, 10, 5) != sart::cell_hash(43, 10, 5));
        CHECK(sart::smoothstep(0.0F) == 0.0F);
        CHECK(sart::smoothstep(0.5F) == 0.5F);
        CHECK(sart::smoothstep(1.0F) == 1.0F);
        for (std::uint64_t seed = 0; seed < 10; ++seed) {
            const auto value = sart::normalized_hash(seed, 3, 4);
            CHECK(value >= 0.0F && value <= 1.0F);
        }
        const auto art = sart::Art::parse("X X\n XX");
        const sart::AnimationMetadata metadata(art, 42);
        CHECK(metadata.cell_at(0, 0)->glyph == U'X');
        CHECK(metadata.cell_at(1, 0) == nullptr);
        CHECK(metadata.cell_at(2, 1)->glyph == U'X');
        CHECK(metadata.cell_at(3, 0) == nullptr);
    }

    void renderer_matches_rust_goldens() {
        const auto art = sart::Art::parse("  ____  \n / __ \\ \n/ /_/ / \n/_____/ ");
        const sart::TerminalSize terminal_size{40, 10};
        const auto layout = sart::layout(art.size(), {40, 10});
        const sart::AnimationMetadata metadata(art, 42);
        constexpr std::array<float, 5> points{0.0F, 0.25F, 0.5F, 0.75F, 1.0F};
        constexpr std::array<std::string_view, 5> names{
            "frame_000.ans", "frame_025.ans", "frame_050.ans", "frame_075.ans", "frame_100.ans",
        };
        for (std::size_t index = 0; index < points.size(); ++index) {
            const auto actual = sart::generate_frame_bytes(art, metadata, layout,
                                                           {points[index], false, points[index] == 0.0F, true, 0});
            const auto expected = read_file(std::filesystem::path(SART_SOURCE_ROOT) / "tests/golden" / names[index]);
            CHECK(actual == expected);
        }

        const auto pixel = sart::Art::parse("X");
        sart::BufferTerminal terminal({10, 5});
        sart::render_final(terminal, pixel, nullptr, false);
        CHECK(terminal.contents().find("\x1b[?25l") != std::string::npos);
        CHECK(terminal.contents().find('X') != std::string::npos);
        CHECK(terminal.contents().find("\x1b[?25h") != std::string::npos);
        CHECK(terminal_size.width == 40);
    }

    template <typename Function> void check_scene_error(sart::SceneErrorCode expected, Function &&function) {
        try {
            function();
        } catch (const sart::SceneError &error) {
            CHECK(error.code() == expected);
            return;
        }
        throw TestFailure("expected SceneError");
    }

    template <typename Function> void check_display_error(sart::DisplayErrorCode expected, Function &&function) {
        try {
            function();
        } catch (const sart::DisplayError &error) {
            CHECK(error.code() == expected);
            return;
        }
        throw TestFailure("expected DisplayError");
    }

    void display_and_frame_engine_contracts() {
        check_scene_error(sart::SceneErrorCode::empty_dimensions, [] { sart::Dimensions(0, 24); });
        check_scene_error(sart::SceneErrorCode::control_glyph, [] { sart::Cell(U'\x1b'); });
        const std::array<std::string_view, 2> rows{"ab", "c"};
        auto scene = sart::Scene::from_rows(rows);
        CHECK(scene.dimensions() == sart::Dimensions(2, 2));
        CHECK(scene.get(1, 1)->glyph() == U' ');
        scene.set(1, 1, sart::Cell(U'd'));
        CHECK(scene.get(1, 1)->glyph() == U'd');
        check_scene_error(sart::SceneErrorCode::out_of_bounds, [&] { scene.set(2, 0, sart::Cell()); });

        sart::BufferBackend display(sart::Dimensions(2, 1));
        const std::array<std::string_view, 1> display_row{"ok"};
        const auto display_scene = sart::Scene::from_rows(display_row);
        check_display_error(sart::DisplayErrorCode::invalid_state, [&] { display.render(display_scene); });
        display.acquire();
        CHECK(display.state() == sart::DisplayState::hidden);
        display.show();
        display.show();
        display.render(display_scene);
        display.queue_input(sart::InputEvent::bytes({0x1b}));
        auto input = display.poll_input(std::chrono::milliseconds(5));
        const std::array<std::uint8_t, 1> escape{0x1b};
        CHECK(input && input->equals_bytes(escape));
        display.details(true);
        CHECK(display.state() == sart::DisplayState::details);
        CHECK(!display.poll_input(std::chrono::milliseconds(0)));
        display.queue_input(sart::InputEvent::return_to_splash());
        auto returned = display.poll_input(std::chrono::milliseconds(0));
        CHECK(returned && returned->kind() == sart::InputEvent::Kind::return_to_splash);
        display.details(false);
        display.hide();
        display.restore();
        display.restore();
        CHECK(display.frames().size() == 1 && display.frames()[0] == display_scene);
        CHECK(display.operations().size() == 10);
        CHECK(display.operations().front().kind == sart::BufferOperation::Kind::acquire);
        CHECK(display.operations().back().kind == sart::BufferOperation::Kind::restore);

        sart::BufferBackend narrow(sart::Dimensions(3, 1));
        narrow.acquire();
        narrow.show();
        check_display_error(sart::DisplayErrorCode::sensitive_text_out_of_bounds,
                            [&] { narrow.render_sensitive_text(0, 0, "界界", {}); });

        const auto art = sart::Art::parse("X");
        const sart::FrameEngine engine(art, 42);
        const auto frame = engine.render({5, 3}, 0.5F, true, 0);
        CHECK(frame.dimensions() == sart::Dimensions(5, 3));
        CHECK(frame.get(2, 1)->glyph() == U'X');
        CHECK(frame.get(2, 1)->style().foreground == sart::Color::white);
        CHECK(*frame.get(0, 0) == sart::Cell());
        check_scene_error(sart::SceneErrorCode::empty_dimensions,
                          [&] { static_cast<void>(engine.render({0, 24}, 0.5F, false, 0)); });
    }

    struct FakeVtState {
        std::uint16_t active{2};
        std::uint16_t available{7};
        sart::Dimensions dimensions{20, 6};
        termios terminal{};
        int kd_mode{1};
        int keyboard_mode{2};
        std::vector<std::uint16_t> activations;
        std::vector<std::uint16_t> deallocations;
        std::vector<int> closed;
        std::vector<std::string> ordinary_writes;
        std::vector<std::size_t> sensitive_lengths;
        std::vector<std::uint8_t> input;
        bool raw{};
        bool terminal_restored{};
    };

    class FakeVtIo final : public sart::VtIo {
      public:
        explicit FakeVtIo(FakeVtState &state) : state_(state) {}
        int open_control(const std::filesystem::path &path) override {
            CHECK(path == "/dev/tty0");
            return 10;
        }
        std::uint16_t active_vt(int) override { return state_.active; }
        std::uint16_t open_query(int) override { return state_.available; }
        int open_vt(const std::filesystem::path &path, std::uint16_t number) override {
            CHECK(path == std::filesystem::path("/dev/tty" + std::to_string(number)));
            return 11;
        }
        void close_device(int device) noexcept override { state_.closed.push_back(device); }
        sart::Dimensions dimensions(int) override { return state_.dimensions; }
        termios terminal_state(int) override { return state_.terminal; }
        void set_raw_terminal(int, const termios &) override { state_.raw = true; }
        void restore_terminal(int, const termios &) override { state_.terminal_restored = true; }
        int kd_mode(int) override { return state_.kd_mode; }
        void set_kd_mode(int, int mode) override { state_.kd_mode = mode; }
        int keyboard_mode(int) override { return state_.keyboard_mode; }
        void set_keyboard_mode(int, int mode) override { state_.keyboard_mode = mode; }
        void activate(int, std::uint16_t number) override {
            state_.activations.push_back(number);
            state_.active = number;
        }
        void wait_active(int, std::uint16_t number, std::chrono::milliseconds) override {
            CHECK(state_.active == number);
        }
        sart::VtDeallocation disallocate(int, std::uint16_t number) override {
            state_.deallocations.push_back(number);
            return sart::VtDeallocation::deallocated;
        }
        void write_all(int, std::string_view bytes) override { state_.ordinary_writes.emplace_back(bytes); }
        void write_restore(int, std::string_view bytes) override { state_.ordinary_writes.emplace_back(bytes); }
        void write_sensitive(int, std::string_view bytes) override { state_.sensitive_lengths.push_back(bytes.size()); }
        void flush(int) override {}
        std::optional<std::vector<std::uint8_t>> poll_read(int, std::chrono::milliseconds) override {
            if (state_.input.empty())
                return std::nullopt;
            return std::exchange(state_.input, {});
        }

      private:
        FakeVtState &state_;
    };

    void text_vt_backend_contracts() {
        FakeVtState state;
        sart::TextVtBackend backend(sart::TextVtConfig::open_query(), std::make_unique<FakeVtIo>(state));
        backend.acquire();
        CHECK(backend.state() == sart::DisplayState::hidden);
        CHECK(backend.original_vt() == 2);
        CHECK(backend.splash_vt() == 7);
        CHECK(state.raw);
        CHECK(state.kd_mode == 0);
        CHECK(state.keyboard_mode == 3);
        backend.show();
        CHECK(backend.state() == sart::DisplayState::splash);
        const std::array<std::string_view, 6> rows{"                    ", "                    ",
                                                   "        ok          ", "                    ",
                                                   "                    ", "                    "};
        backend.render(sart::Scene::from_rows(rows));
        CHECK(state.ordinary_writes.back().find("ok") != std::string::npos);
        backend.render_sensitive_text(1, 2, "secret", {});
        CHECK(state.sensitive_lengths == std::vector<std::size_t>{6});
        CHECK(std::none_of(state.ordinary_writes.begin(), state.ordinary_writes.end(),
                           [](const auto &write) { return write.find("secret") != std::string::npos; }));
        state.input = {0x1b};
        auto input = backend.poll_input(std::chrono::milliseconds(0));
        const std::array<std::uint8_t, 1> escape{0x1b};
        CHECK(input && input->equals_bytes(escape));
        state.dimensions = sart::Dimensions(21, 6);
        auto resized = backend.poll_input(std::chrono::milliseconds(0));
        CHECK(resized && resized->kind() == sart::InputEvent::Kind::resized);
        backend.details(true);
        CHECK(backend.state() == sart::DisplayState::details);
        state.active = 7;
        auto returned = backend.poll_input(std::chrono::milliseconds(0));
        CHECK(returned && returned->kind() == sart::InputEvent::Kind::return_to_splash);
        backend.details(false);
        backend.restore();
        CHECK(backend.state() == sart::DisplayState::restored);
        CHECK(state.terminal_restored);
        CHECK(state.kd_mode == 1);
        CHECK(state.keyboard_mode == 2);
        CHECK(state.active == 2);
        CHECK(state.deallocations == std::vector<std::uint16_t>{7});
        CHECK(state.closed == std::vector<int>({11, 10}));

        FakeVtState configured_state;
        sart::TextVtBackend configured(sart::TextVtConfig::configured(9), std::make_unique<FakeVtIo>(configured_state));
        configured.acquire();
        CHECK(configured.splash_vt() == 9);
        configured.restore();
        CHECK(configured_state.deallocations.empty());
    }

    void splash_engine_contracts() {
        using namespace sart::splash;
        class ObscuredPrompt final : public sart::password::PromptCoordinator {
          public:
            [[nodiscard]] bool enabled() const noexcept override { return true; }
            void poll(SplashState &) override {}
            void handle_input(SplashState &, std::span<const std::uint8_t>) override {}
            [[nodiscard]] std::optional<sart::password::InputFeedback> feedback() const noexcept override {
                return sart::password::InputFeedback{3, sart::password::EchoMode::obscured};
            }
            void with_visible_text(const std::function<void(std::string_view)> &) const override {}
            void abandon(SplashState &) noexcept override {}
        };
        const auto art = sart::Art::parse("X");
        auto owned_backend = std::make_unique<sart::BufferBackend>(sart::Dimensions(30, 8));
        auto *backend = owned_backend.get();
        EngineConfig config;
        config.frames_per_second = 20;
        config.animation_cycle = std::chrono::milliseconds(1000);
        SplashEngine engine(std::move(owned_backend), art, nullptr, config);
        SplashState state;
        static_cast<void>(state.apply(MarkRunning{}));
        static_cast<void>(state.apply(SetStatus{std::string("Loading")}));
        static_cast<void>(state.apply(SetProgress{50}));
        engine.start(state);
        const auto first = engine.tick_at(state, std::chrono::milliseconds(0));
        CHECK(first.frame_rendered && !first.stopped);
        CHECK(backend->frames().size() == 1);
        const auto encoded = sart::encode_scene(backend->frames().back());
        CHECK(encoded.find("Loading") != std::string::npos);
        CHECK(encoded.find("50%") != std::string::npos);
        CHECK(encoded.find("BOOTING") != std::string::npos);
        backend->queue_input(sart::InputEvent::bytes({0x1b}));
        static_cast<void>(engine.tick_at(state, std::chrono::milliseconds(1)));
        CHECK(state.view().base_view() == BaseView::details);
        backend->queue_input(sart::InputEvent::return_to_splash());
        static_cast<void>(engine.tick_at(state, std::chrono::milliseconds(2)));
        CHECK(state.view().base_view() == BaseView::splash);
        const auto [progress, iteration] =
            animation_position(std::chrono::milliseconds(1250), std::chrono::milliseconds(1000));
        CHECK(std::abs(progress - 0.25F) < 0.0001F);
        CHECK(iteration == 1);
        engine.shutdown(true);
        CHECK(backend->state() == sart::DisplayState::restored);
        CHECK(backend->operations().back().restore_mode == sart::RestoreMode::retain_pixels);

        auto prompt_backend = std::make_unique<sart::BufferBackend>(sart::Dimensions(40, 11));
        auto *prompt_backend_view = prompt_backend.get();
        SplashEngine prompt_engine(std::move(prompt_backend), art, nullptr);
        SplashState prompt_state;
        static_cast<void>(prompt_state.apply(MarkRunning{}));
        static_cast<void>(prompt_state.apply(BeginPrompt{PromptMetadata(7, "Disk password")}));
        ObscuredPrompt prompt;
        prompt_engine.start(prompt_state);
        const auto prompt_tick = prompt_engine.tick_at(prompt_state, std::chrono::milliseconds(0), &prompt);
        CHECK(prompt_tick.frame_rendered);
        const auto &prompt_scene = prompt_backend_view->frames().back();
        CHECK(prompt_scene.get(6, 3)->glyph() == U'╭');
        CHECK(prompt_scene.get(33, 3)->glyph() == U'╮');
        CHECK(prompt_scene.get(6, 6)->glyph() == U'╰');
        CHECK(prompt_scene.get(33, 6)->glyph() == U'╯');
        CHECK(prompt_scene.get(13, 4)->glyph() == U'D');
        CHECK(prompt_scene.get(18, 5)->glyph() == U'*');
    }

    void password_memory_and_input_contracts() {
        using namespace sart::password;
        try {
            SecureSecret invalid(0);
            static_cast<void>(invalid);
            throw TestFailure("zero-capacity secret was accepted");
        } catch (const std::invalid_argument &) {
        }
        SecureSecret secret(32);
        secret.push("ab🔐");
        CHECK(secret.size() == 6);
        CHECK(secret.pop() == U'🔐');
        CHECK(secret.expose([](std::span<const std::byte> bytes) {
            return std::string(reinterpret_cast<const char *>(bytes.data()), bytes.size());
        }) == "ab");
        secret.clear();
        CHECK(secret.empty());

        PromptInput obscured(32, false, false);
        for (const auto byte : std::string("a🔐")) {
            static_cast<void>(obscured.feed(static_cast<std::uint8_t>(byte)));
        }
        CHECK(obscured.feedback() == InputFeedback{2, EchoMode::obscured});
        CHECK(obscured.feed(127).kind == InputOutcomeKind::changed);
        CHECK(obscured.feedback().character_count == 1);
        CHECK(!obscured.with_visible_text([](auto text) { return text; }));

        PromptInput visible(8, true, false);
        CHECK(visible.handle({PromptKeyKind::character, U'x'}).kind == InputOutcomeKind::changed);
        CHECK(visible.with_visible_text([](auto text) { return text == "x"; }));
        CHECK(visible.handle({PromptKeyKind::enter}).kind == InputOutcomeKind::submit);
        visible.finish_with([](const SecureSecret &submitted) { CHECK(submitted.size() == 1); });
        CHECK(visible.empty());

        PromptInput silent(8, true, true);
        static_cast<void>(silent.feed('s'));
        CHECK(silent.feedback() == InputFeedback{0, EchoMode::silent});
        CHECK(silent.feed(27).kind == InputOutcomeKind::cancelled);
        CHECK(silent.empty());
    }

    void systemd_password_contracts() {
        using namespace sart::password;
        const auto root =
            std::filesystem::path(SART_SOURCE_ROOT) / "target/cpp" / ("ask-test-" + std::to_string(getpid()));
        std::error_code ignored;
        std::filesystem::remove_all(root, ignored);
        std::filesystem::create_directories(root);
        CHECK(chmod(root.c_str(), 0700) == 0);
        const auto socket_path = root / "reply.sock";
        const auto contents = std::string("[Ask]\nMessage=Unlock root\nPID=") + std::to_string(getpid()) +
                              "\nSocket=" + socket_path.string() +
                              "\nEcho=no\nSilent=no\nAcceptCached=yes\nNotAfter=999999999999999\n";
        const auto request = AskRequest::parse({"ask.test", 1, 2}, contents);
        CHECK(request.message() == "Unlock root");
        CHECK(request.requester_pid() == static_cast<std::uint32_t>(getpid()));
        CHECK(request.accept_cached_requested());
        CHECK(!request.expired(monotonic_microseconds()));
        CHECK(requester_alive(static_cast<std::uint32_t>(getpid())));

        const auto request_path = root / "ask.test";
        {
            std::ofstream output(request_path);
            output << contents;
        }
        CHECK(chmod(request_path.c_str(), 0600) == 0);
        const auto scan = scan_ask_requests(root, geteuid());
        CHECK(scan.requests.size() == 1);
        CHECK(scan.rejected.empty());

        const auto receiver = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
        CHECK(receiver >= 0);
        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        std::memcpy(address.sun_path, socket_path.c_str(), socket_path.string().size());
        CHECK(bind(receiver, reinterpret_cast<const sockaddr *>(&address),
                   static_cast<socklen_t>(offsetof(sockaddr_un, sun_path) + socket_path.string().size() + 1)) == 0);
        SystemdReplySocket sender;
        SecureSecret secret(32);
        secret.push("swordfish");
        sender.send_success(request, secret, geteuid());
        CHECK(secret.empty());
        std::array<char, 32> packet{};
        const auto count = recv(receiver, packet.data(), packet.size(), 0);
        CHECK(count == 10);
        CHECK(std::string_view(packet.data(), static_cast<std::size_t>(count)) == "+swordfish");
        sender.send_cancel(request, geteuid());
        CHECK(recv(receiver, packet.data(), packet.size(), 0) == 1);
        CHECK(packet[0] == '-');

        sart::splash::SplashState prompt_state;
        static_cast<void>(prompt_state.apply(sart::splash::MarkRunning{}));
        SystemdPromptCoordinator coordinator(root, geteuid(), 32);
        coordinator.poll(prompt_state);
        CHECK(prompt_state.view().prompt_metadata() != nullptr);
        const std::array<std::uint8_t, 3> answer{'p', 'w', '\n'};
        coordinator.handle_input(prompt_state, answer);
        CHECK(prompt_state.view().prompt_metadata() == nullptr);
        const auto answer_count = recv(receiver, packet.data(), packet.size(), 0);
        CHECK(answer_count == 3);
        CHECK(std::string_view(packet.data(), static_cast<std::size_t>(answer_count)) == "+pw");
        coordinator.poll(prompt_state);
        CHECK(prompt_state.view().prompt_metadata() == nullptr);
        close(receiver);
        std::filesystem::remove_all(root, ignored);
    }

    void digest_and_cpio_contracts() {
        CHECK(sart::sha256("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        const auto elf_text = std::string_view("\x7f"
                                               "ELFfake");
        const auto hook_text = std::string_view("#!/bin/sh");
        const std::array inputs{
            sart::integration::CpioInput{"usr/bin/sart", std::as_bytes(std::span(elf_text.data(), elf_text.size())),
                                         0755},
            sart::integration::CpioInput{"usr/lib/dracut/modules.d/60sart/module-setup.sh",
                                         std::as_bytes(std::span(hook_text.data(), hook_text.size())), 0755},
        };
        const auto archive = sart::integration::build_cpio_archive(inputs);
        const auto entries = sart::integration::parse_cpio_archive(archive);
        CHECK(entries.size() == 2);
        CHECK(entries[0].name == "usr/bin/sart");
        const auto report = sart::integration::inspect_candidate_archive(archive, sart::sha256(elf_text),
                                                                         sart::integration::AdapterId::dracut_systemd);
        CHECK(report.entries_count == 2);
        CHECK(report.candidate_digest == sart::sha256(archive));
    }

    void native_password_transport_contracts() {
        using namespace sart::password;
        {
            auto pair = native_credential_pair();
            SecureSecret secret(32);
            secret.push("native-secret");
            pair.responder.reply_secret(secret);
            CHECK(secret.empty());
            auto received = pair.client.receive(32);
            CHECK(received.has_value());
            CHECK(received->expose([](std::span<const std::byte> bytes) {
                return std::string(reinterpret_cast<const char *>(bytes.data()), bytes.size());
            }) == "native-secret");
        }
        {
            auto pair = native_credential_pair();
            pair.responder.reply_cancel();
            CHECK(!pair.client.receive(32));
        }
        std::array<int, 2> carriers{-1, -1};
        CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, carriers.data()) == 0);
        sart::splash::FileDescriptor first(carriers[0]);
        sart::splash::FileDescriptor second(carriers[1]);
        auto credentials = native_credential_pair();
        const auto metadata_text = std::string_view("bounded metadata");
        send_responder_packet(first.get(), geteuid(),
                              std::as_bytes(std::span(metadata_text.data(), metadata_text.size())),
                              std::move(credentials.responder));
        std::array<std::byte, 64> metadata{};
        auto transferred = receive_responder_packet(second.get(), geteuid(), metadata);
        CHECK(transferred.metadata_size == metadata_text.size());
        CHECK(std::string_view(reinterpret_cast<const char *>(metadata.data()), transferred.metadata_size) ==
              metadata_text);
        SecureSecret transferred_secret(32);
        transferred_secret.push("through-fd");
        transferred.responder.reply_secret(transferred_secret);
        auto final_secret = credentials.client.receive(32);
        CHECK(final_secret && final_secret->size() == 10);

        const auto now = monotonic_microseconds();
        NativeRequestMetadata request{NativeAdapter::mkinitfs_busybox,
                                      {7, 8, static_cast<std::uint32_t>(getpid()), process_start_ticks(getpid())},
                                      now + 1'000'000,
                                      1,
                                      3,
                                      128,
                                      "Unlock encrypted root:",
                                      false,
                                      false};
        const auto packet = encode_native_request(request);
        const auto decoded = decode_native_request(packet, now);
        CHECK(decoded.identity == request.identity);
        CHECK(decoded.prompt == request.prompt);
        CHECK(decoded.adapter == request.adapter);
    }

    void native_password_coordinator_contracts() {
        using namespace sart::password;
        using namespace sart::splash;
        const auto runtime =
            std::filesystem::path(SART_SOURCE_ROOT) / "target/cpp" / ("native-coordinator-" + std::to_string(getpid()));
        std::error_code ignored;
        std::filesystem::remove_all(runtime, ignored);
        const RuntimePaths paths(runtime);
        auto owner = RuntimeOwner::acquire(paths);
        NativePromptCoordinator coordinator(owner.bind_native_password_listener(), geteuid());
        std::array<int, 2> pipe_descriptors{-1, -1};
        CHECK(pipe2(pipe_descriptors.data(), O_CLOEXEC) == 0);
        FileDescriptor reader(pipe_descriptors[0]);
        FileDescriptor writer(pipe_descriptors[1]);
        std::optional<NativeAskpassOutcome> outcome;
        std::thread client([&outcome, writer = std::move(writer), &paths] mutable {
            outcome = run_native_askpass_client(NativeAdapter::dracut_classic, {"Unlock native root:", 1, 32},
                                                std::move(writer), paths.native_password_socket(), geteuid(),
                                                std::chrono::seconds(2));
        });
        SplashState state;
        static_cast<void>(state.apply(MarkRunning{}));
        for (std::size_t attempt = 0; attempt < 200 && state.view().prompt_metadata() == nullptr; ++attempt) {
            coordinator.poll(state);
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
        CHECK(state.view().prompt_metadata() != nullptr);
        const std::array<std::uint8_t, 3> answer{'p', 'w', '\n'};
        coordinator.handle_input(state, answer);
        client.join();
        CHECK(outcome == NativeAskpassOutcome::delivered);
        std::array<char, 8> bytes{};
        const auto count = read(reader.get(), bytes.data(), bytes.size());
        CHECK(count == 3);
        CHECK(std::string_view(bytes.data(), static_cast<std::size_t>(count)) == "pw\n");
        CHECK(state.view().prompt_metadata() == nullptr);
        coordinator.abandon(state);

        const auto broken_runtime = std::filesystem::path(SART_SOURCE_ROOT) / "target/cpp" /
                                    ("native-coordinator-broken-" + std::to_string(getpid()));
        std::filesystem::remove_all(broken_runtime, ignored);
        const RuntimePaths broken_paths(broken_runtime);
        auto broken_owner = RuntimeOwner::acquire(broken_paths);
        NativePromptCoordinator broken_coordinator(broken_owner.bind_native_password_listener(), geteuid());
        SplashState broken_state;
        static_cast<void>(broken_state.apply(MarkRunning{}));
        std::array<int, 2> broken_pipe{-1, -1};
        CHECK(pipe2(broken_pipe.data(), O_CLOEXEC) == 0);
        FileDescriptor broken_reader(broken_pipe[0]);
        FileDescriptor broken_writer(broken_pipe[1]);
        std::optional<NativeAskpassOutcome> broken_outcome;
        std::thread broken_client([&broken_outcome, writer = std::move(broken_writer), &broken_paths] mutable {
            broken_outcome = run_native_askpass_client(NativeAdapter::dracut_classic, {"Unlock native root:", 1, 32},
                                                       std::move(writer), broken_paths.native_password_socket(),
                                                       geteuid(), std::chrono::seconds(2));
        });
        for (std::size_t attempt = 0; attempt < 200 && broken_state.view().prompt_metadata() == nullptr; ++attempt) {
            broken_coordinator.poll(broken_state);
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
        const bool broken_prompt_ready = broken_state.view().prompt_metadata() != nullptr;
        if (broken_prompt_ready) {
            broken_reader = FileDescriptor();
            broken_coordinator.handle_input(broken_state, answer);
        }
        broken_client.join();
        CHECK(broken_prompt_ready);
        CHECK(broken_outcome == NativeAskpassOutcome::console_fallback);
        broken_coordinator.abandon(broken_state);
    }

    void embedded_and_cmdline_contracts() {
        CHECK(sart::Art::parse(sart::embedded::default_art).height() == 22);
        CHECK(sart::Art::parse(sart::embedded::small_art).height() == 3);
        CHECK(sart::embedded::resource_set_version == 13);
        CHECK(sart::embedded::default_config.ends_with('\n'));
        CHECK(sart::embedded::default_config.contains("runtime_dir=/run/sart\n"));
        CHECK(sart::embedded::template_ids.size() == 37);
        std::set<std::string_view> names;
        std::set<std::string_view> paths;
        for (const auto id : sart::embedded::template_ids) {
            const auto resource = sart::embedded::template_resource(id);
            CHECK(resource.id == id);
            CHECK(resource.experimental_unproven);
            CHECK(!resource.contents.empty());
            CHECK(resource.contents.ends_with('\n'));
            CHECK(resource.contents.find('\0') == std::string_view::npos);
            CHECK(resource.contents.find('\r') == std::string_view::npos);
            CHECK(names.insert(sart::embedded::template_name(id)).second);
            CHECK(resource.materialization.path.starts_with('/'));
            CHECK(resource.materialization.path.find("/../") == std::string_view::npos);
            if (resource.materialization.kind == sart::embedded::MaterializationKind::managed_snippet) {
                CHECK(resource.materialization.mode == 0);
                CHECK(!resource.materialization.insertion_point.empty());
                CHECK(resource.contents.contains("# sart:begin"));
                CHECK(resource.contents.contains("# sart:end"));
            } else {
                CHECK(resource.materialization.mode == 0644 || resource.materialization.mode == 0755);
                CHECK(paths.insert(resource.materialization.path).second);
                if (resource.materialization.mode == 0755) {
                    CHECK(resource.contents.starts_with("#!"));
                }
            }
            CHECK(!resource.contents.starts_with("\x7f"
                                                 "ELF"));
            CHECK(!resource.contents.contains("\nexec /usr/bin/sart "));
        }
        CHECK(!sart::cmdline::splash_disabled("quiet splash sart=1"));
        CHECK(sart::cmdline::splash_disabled("quiet sart=0 splash"));
        CHECK(sart::cmdline::splash_disabled("rd.sart=0"));
        CHECK(!sart::cmdline::splash_disabled("xsart=0 sart=00"));
        CHECK(!sart::process_is_allowed(1));
        CHECK(sart::process_is_allowed(2));

        CHECK(sart::adapter_ids.size() == 8);
        CHECK(sart::adapter_pairs().size() == 7);
        std::set<std::string_view> adapter_names;
        for (const auto id : sart::adapter_ids) {
            const auto &metadata = sart::adapter_metadata(id);
            CHECK(metadata.id == id);
            CHECK(adapter_names.insert(metadata.name).second);
            for (const auto resource : metadata.resources) {
                static_cast<void>(sart::embedded::template_resource(resource));
            }
        }
        CHECK(sart::adapter_pair(sart::AdapterId::dracut_classic, sart::AdapterId::openrc_real_root)->status ==
              sart::SupportStatus::experimental_unproven);
        CHECK(sart::adapter_pair(sart::AdapterId::mkinitfs_boot_deploy, sart::AdapterId::systemd_real_root)->status ==
              sart::SupportStatus::proven_supported);

        const std::string mkinitfs_source = "#!/bin/sh\nVERSION=3.14.0-r0\n\n"
                                            "# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n"
                                            "\t$MOCK nlplug-findfs\n"
                                            "\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n"
                                            "\t\tfi\n\tdone\n\t$MOCK sync\n";
        const auto mkinitfs_patched = sart::integration::patch_mkinitfs_init(mkinitfs_source);
        CHECK(mkinitfs_patched.has_value());
        CHECK(mkinitfs_patched->contains("# sart:begin mkinitfs-early-v1"));
        CHECK(mkinitfs_patched->contains("# sart:begin mkinitfs-handoff-v1"));
        CHECK(sart::integration::patch_mkinitfs_init(*mkinitfs_patched) == mkinitfs_patched);

        const std::string boot_deploy_source = R"SART(prefix
unlock_root_partition() {
	command -v cryptsetup >/dev/null || return
	if cryptsetup isLuks "$PMOS_ROOT"; then
		splash_hide
		tried=0
		until cryptsetup status root | grep -qwi active; do
			fde-unlock "$PMOS_ROOT" "$tried"
			tried=$((tried + 1))
		done
		PMOS_ROOT=/dev/mapper/root
		splash_set_message "Loading"
	fi
}
suffix
)SART";
        const auto deploy_patched = sart::integration::patch_boot_deploy_init_functions(
            boot_deploy_source, sart::integration::reviewed_boot_deploy_initramfs_version);
        CHECK(deploy_patched.has_value());
        CHECK(deploy_patched->contains("# sart:begin mkinitfs-boot-deploy-fde-v1"));
        CHECK(sart::integration::patch_boot_deploy_init_functions(
                  *deploy_patched, sart::integration::reviewed_boot_deploy_initramfs_version) == deploy_patched);

        const auto elf = static_elf_fixture();
        sart::install::validate_static_elf(elf);
        auto interpreted = elf;
        interpreted[64] = std::byte{3};
        bool rejected_interpreter = false;
        try {
            sart::install::validate_static_elf(interpreted);
        } catch (const std::runtime_error &) {
            rejected_interpreter = true;
        }
        CHECK(rejected_interpreter);
        const auto dracut_plan =
            sart::install::build_install_plan(elf, sart::AdapterId::dracut_systemd, sart::AdapterId::systemd_real_root);
        CHECK(dracut_plan.operations.size() == 9);
        CHECK(dracut_plan.managed_snippets.empty());
        CHECK(dracut_plan.activations.size() == 4);
        CHECK(dracut_plan.identity().size() == 64);
        CHECK(sart::install::render_plan_human(dracut_plan, true).contains("status: READY"));
        CHECK(sart::install::render_plan_json(dracut_plan, true).contains("\"actionable\":true"));
        const auto mkinitfs_plan = sart::install::build_install_plan(elf, sart::AdapterId::mkinitfs_busybox,
                                                                     sart::AdapterId::openrc_real_root);
        CHECK(mkinitfs_plan.operations.size() == 6);
        CHECK(mkinitfs_plan.managed_snippets.size() == 2);
        CHECK(mkinitfs_plan.activations.size() == 2);
    }

    void installer_transaction_contracts() {
        std::array<char, 64> root_template{};
        const auto source = std::string("/tmp/sart-cpp-installer-") + std::to_string(getpid()) + "-XXXXXX";
        CHECK(source.size() < root_template.size());
        std::copy(source.begin(), source.end(), root_template.begin());
        char *created = mkdtemp(root_template.data());
        CHECK(created != nullptr);
        const std::filesystem::path root(created);
        const auto elf = static_elf_fixture();
        const auto plan = sart::install::build_install_plan(elf, sart::AdapterId::dracut_systemd,
                                                            sart::AdapterId::systemd_real_root, false, root.string());
        sart::install::Installer installer(root.string(), geteuid(), true);
        CHECK(installer.apply(plan) == sart::install::ApplyOutcome::installed);
        const auto status = installer.status();
        CHECK(status.installed);
        CHECK(!status.recovery_required);
        CHECK(status.files.size() == 10);
        CHECK(std::ranges::all_of(
            status.files, [](const auto &file) { return file.state == sart::install::FileStatusState::exact; }));
        CHECK(installer.apply(plan) == sart::install::ApplyOutcome::already_current);

        const auto manifest = root / "var/lib/sart/install/manifest.v1";
        const auto canonical_manifest = read_file(manifest);
        auto noncanonical_manifest = canonical_manifest;
        const auto mode = noncanonical_manifest.find("\t493\t");
        CHECK(mode != std::string::npos);
        noncanonical_manifest.insert(mode + 1, "0");
        {
            std::ofstream output(manifest, std::ios::binary | std::ios::trunc);
            output << noncanonical_manifest;
        }
        bool rejected_noncanonical_manifest = false;
        try {
            static_cast<void>(installer.status());
        } catch (const std::runtime_error &) {
            rejected_noncanonical_manifest = true;
        }
        CHECK(rejected_noncanonical_manifest);
        {
            std::ofstream output(manifest, std::ios::binary | std::ios::trunc);
            output << canonical_manifest;
        }

        const auto binary = root / "usr/bin/sart";
        {
            std::ofstream output(binary, std::ios::binary | std::ios::trunc);
            output << "modified";
        }
        chmod(binary.c_str(), 0755);
        const auto modified = installer.status();
        CHECK(std::ranges::any_of(modified.files, [](const auto &file) {
            return file.state == sart::install::FileStatusState::content_modified;
        }));
        {
            std::ofstream output(binary, std::ios::binary | std::ios::trunc);
            output.write(reinterpret_cast<const char *>(elf.data()), static_cast<std::streamsize>(elf.size()));
        }
        chmod(binary.c_str(), 0755);
        const auto report = installer.uninstall();
        CHECK(report.removed == 10);
        CHECK(report.restored == 0);
        CHECK(!installer.status().installed);
        std::filesystem::remove_all(root);

        std::fill(root_template.begin(), root_template.end(), '\0');
        const auto patch_source = std::string("/tmp/sart-cpp-patch-") + std::to_string(getpid()) + "-XXXXXX";
        std::copy(patch_source.begin(), patch_source.end(), root_template.begin());
        created = mkdtemp(root_template.data());
        CHECK(created != nullptr);
        const std::filesystem::path patch_root(created);
        const auto target = patch_root / "usr/share/mkinitfs/initramfs-init";
        std::filesystem::create_directories(target.parent_path());
        const std::string original = "#!/bin/sh\nVERSION=3.14.0-r0\n\n"
                                     "# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n"
                                     "\t$MOCK nlplug-findfs\n"
                                     "\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n"
                                     "\t\tfi\n\tdone\n\t$MOCK sync\n";
        {
            std::ofstream output(target);
            output << original;
        }
        chmod(target.c_str(), 0755);
        const auto patch_plan = sart::install::build_install_plan(
            elf, sart::AdapterId::mkinitfs_busybox, sart::AdapterId::openrc_real_root, false, patch_root.string());
        sart::install::Installer patch_installer(patch_root.string(), geteuid(), true);
        CHECK(patch_installer.apply(patch_plan) == sart::install::ApplyOutcome::installed);
        CHECK(read_file(target).find("# sart:begin mkinitfs-early-v1") != std::string::npos);
        const auto patch_report = patch_installer.uninstall();
        CHECK(patch_report.removed == 8);
        CHECK(patch_report.restored == 1);
        CHECK(patch_report.preserved_directories.empty());
        CHECK(read_file(target) == original);
        std::filesystem::remove_all(patch_root);

        std::fill(root_template.begin(), root_template.end(), '\0');
        const auto recovery_source = std::string("/tmp/sart-cpp-recover-") + std::to_string(getpid()) + "-XXXXXX";
        std::copy(recovery_source.begin(), recovery_source.end(), root_template.begin());
        created = mkdtemp(root_template.data());
        CHECK(created != nullptr);
        const std::filesystem::path recovery_root(created);
        const auto recovery_plan = sart::install::build_install_plan(
            elf, sart::AdapterId::dracut_systemd, sart::AdapterId::systemd_real_root, false, recovery_root.string());
        const auto child = fork();
        CHECK(child >= 0);
        if (child == 0) {
            try {
                sart::install::Installer child_installer(recovery_root.string(), geteuid(), true);
                static_cast<void>(child_installer.apply(recovery_plan));
                _exit(0);
            } catch (...) {
                _exit(2);
            }
        }
        const auto journal = recovery_root / ".sart-installer-journal.v1";
        bool ready = false;
        for (std::size_t attempt = 0; attempt < 20000; ++attempt) {
            if (std::filesystem::exists(journal)) {
                try {
                    if (read_file(journal).find("phase\tready\n") != std::string::npos) {
                        ready = true;
                        break;
                    }
                } catch (...) {
                }
            }
            std::this_thread::sleep_for(std::chrono::microseconds(100));
        }
        CHECK(ready);
        CHECK(kill(child, SIGSTOP) == 0);
        CHECK(kill(child, SIGKILL) == 0);
        int child_status = 0;
        CHECK(waitpid(child, &child_status, 0) == child);
        sart::install::Installer recovery_installer(recovery_root.string(), geteuid(), true);
        CHECK(recovery_installer.status().recovery_required);
        CHECK(recovery_installer.recover() == sart::install::RecoveryOutcome::rolled_back);
        CHECK(!recovery_installer.status().installed);
        CHECK(!std::filesystem::exists(recovery_root / "usr/bin/sart"));
        CHECK(!std::filesystem::exists(journal));
        std::filesystem::remove_all(recovery_root);
    }

    template <typename Function> void check_state_error(sart::splash::StateErrorCode expected, Function &&function) {
        try {
            function();
        } catch (const sart::splash::StateError &error) {
            CHECK(error.code() == expected);
            return;
        }
        throw TestFailure("expected StateError");
    }

    sart::splash::SplashState running_state() {
        sart::splash::SplashState state;
        CHECK(state.apply(sart::splash::MarkRunning{}) == sart::splash::TransitionResult::changed);
        return state;
    }

    sart::splash::PromptMetadata prompt(std::uint64_t request_id) {
        sart::splash::PromptMetadata result(request_id, "Password for encrypted root:");
        result.with_source("systemd-cryptsetup").with_requester_pid(42).with_expiry(50'000);
        return result;
    }

    void installer_backend_contracts() {
        using namespace sart::install;
        const auto elf = static_elf_fixture();
        const auto digest = sart::sha256(std::string_view("known-good"));
        const auto tools = [](const std::vector<std::string_view> &paths) {
            std::vector<ToolFact> result;
            for (const auto path : paths)
                result.push_back(ToolFact::exact(path));
            return result;
        };
        const auto file = [](std::string path, std::uint16_t mode, std::vector<std::byte> contents) {
            return ArchiveEntry{std::move(path), ArchiveEntryKind::file, mode, std::move(contents), 0, 0};
        };
        const auto text_bytes = [](std::string_view text) {
            return std::vector<std::byte>{reinterpret_cast<const std::byte *>(text.data()),
                                          reinterpret_cast<const std::byte *>(text.data() + text.size())};
        };

        DracutSystemdFacts dracut_facts{
            std::string(product_architecture),
            "systemd",
            {"7.0.0-test"},
            1,
            2,
            true,
            min_boot_free_bytes,
            min_boot_free_inodes,
            {"systemd", "crypt"},
            DracutImageLayout::initrd_img,
            GrubRegeneration::update_grub,
            CryptsetupLocation::usr_sbin,
            tools({"/usr/bin/dracut", "/usr/bin/lsinitrd", "/usr/bin/findmnt", "/usr/lib/systemd/systemd",
                   "/usr/sbin/cryptsetup", "/usr/sbin/update-grub", "/usr/sbin/grub-probe"}),
            "/boot/initrd.img-7.0.0-test",
            digest,
            1024,
            "1625-E85D",
            "root=/dev/mapper/crypt-root ro quiet"};
        const auto dracut_contract = plan_dracut_systemd(dracut_facts);
        CHECK(dracut_contract.generate.arguments[3] == "--add");
        CHECK(dracut_systemd_managed_image_path(dracut_contract.candidate_image));
        CHECK(dracut_systemd_unpack_request(dracut_contract, "txn-1").working_directory.has_value());

        std::vector<ArchiveEntry> dracut_entries{file("usr/lib/systemd/systemd", 0755, text_bytes("systemd")),
                                                 file("usr/sbin/cryptsetup", 0755, text_bytes("cryptsetup")),
                                                 file("usr/bin/sart", 0755, elf)};
        for (const auto id :
             {sart::embedded::TemplateId::systemd_start_unit, sart::embedded::TemplateId::systemd_show_unit,
              sart::embedded::TemplateId::systemd_switch_root_unit,
              sart::embedded::TemplateId::systemd_console_agent_drop_in}) {
            const auto resource = sart::embedded::template_resource(id);
            std::string path(resource.materialization.path);
            path.erase(path.begin());
            dracut_entries.push_back(file(path, resource.materialization.mode, text_bytes(resource.contents)));
        }
        for (const auto &[path, target] :
             std::array{std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd.target.wants/sart-start.service", "../sart-start.service"},
                        std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd.target.wants/sart-show.service", "../sart-show.service"},
                        std::pair<std::string_view, std::string_view>{
                            "usr/lib/systemd/system/initrd-switch-root.target.wants/sart-switch-root.service",
                            "../sart-switch-root.service"}}) {
            dracut_entries.push_back({std::string(path), ArchiveEntryKind::symlink, 0777, text_bytes(target), 0, 0});
        }
        const auto dracut_inspection = inspect_dracut_inventory(dracut_entries, elf);
        CHECK(dracut_inspection.sart_digest == sart::sha256(elf));
        const std::array candidate_bytes{std::byte{'i'}, std::byte{'m'}, std::byte{'g'}};
        const auto dracut_record =
            verified_dracut_systemd_image_record(dracut_contract, candidate_bytes, dracut_inspection, elf);
        CHECK(dracut_record.active_digest == sart::sha256(candidate_bytes));
        CHECK(dracut_systemd_sart_free_generate_request(dracut_record).arguments[3] == "--omit");

        InitramfsToolsSystemdFacts initramfs_facts{
            std::string(product_architecture),
            "systemd",
            {"7.0.0-test"},
            1,
            2,
            true,
            min_boot_free_bytes,
            min_boot_free_inodes,
            GrubRegeneration::update_grub,
            CryptsetupLocation::usr_sbin,
            tools({"/usr/sbin/mkinitramfs", "/usr/bin/unmkinitramfs", "/usr/bin/findmnt", "/usr/lib/systemd/systemd",
                   "/usr/sbin/cryptsetup", "/usr/sbin/update-grub", "/usr/sbin/grub-probe"}),
            {{"/usr/share/initramfs-tools/hook-functions", true, true, false, false},
             {"/usr/share/initramfs-tools/hooks/cryptroot", true, true, false, true},
             {"/usr/share/initramfs-tools/scripts/local-top/cryptroot", true, true, false, true},
             {"/usr/lib/cryptsetup/functions", true, true, false, false},
             {"/usr/lib/cryptsetup/askpass", true, true, false, true}},
            "/boot/initrd.img-7.0.0-test",
            digest,
            1024,
            "1625-E85D",
            "root=/dev/mapper/crypt-root ro quiet"};
        const auto initramfs_contract = plan_initramfs_tools_systemd(initramfs_facts);
        CHECK(initramfs_contract.generate.executable == "/usr/sbin/mkinitramfs");
        CHECK(initramfs_tools_systemd_unpack_request(initramfs_contract, "txn-2").arguments.size() == 2);
        std::vector<ArchiveEntry> initramfs_entries{
            file("main/usr/bin/sart", 0755, elf), file("main/init", 0755, text_bytes("init")),
            file("main/scripts/local-top/cryptroot", 0755, text_bytes("cryptroot")),
            file("main/usr/lib/cryptsetup/askpass.sart-console", 0755, text_bytes("console")),
            file("main/usr/lib/cryptsetup/functions", 0644, text_bytes("functions"))};
        for (const auto [path, id] : std::array{
                 std::pair<std::string_view, sart::embedded::TemplateId>{
                     "main/scripts/init-top/sart", sart::embedded::TemplateId::initramfs_tools_early_hook},
                 std::pair<std::string_view, sart::embedded::TemplateId>{
                     "main/scripts/init-bottom/sart", sart::embedded::TemplateId::initramfs_tools_bottom_hook},
                 std::pair<std::string_view, sart::embedded::TemplateId>{
                     "main/usr/lib/cryptsetup/askpass", sart::embedded::TemplateId::initramfs_tools_askpass_wrapper}}) {
            const auto resource = sart::embedded::template_resource(id);
            initramfs_entries.push_back(
                file(std::string(path), resource.materialization.mode, text_bytes(resource.contents)));
        }
        const auto initramfs_inspection = inspect_initramfs_tools_inventory(initramfs_entries, elf);
        CHECK(verified_initramfs_tools_systemd_image_record(initramfs_contract, candidate_bytes, initramfs_inspection,
                                                            elf)
                  .candidate_bytes == 3);

        const std::string mkinitcpio_source =
            "MODULES=()\nHOOKS=(base udev autodetect block encrypt filesystems fsck)\n";
        MkinitcpioSystemdFacts mkinitcpio_facts{
            std::string(product_architecture),
            "systemd",
            {"7.0.0-arch"},
            "linux",
            1,
            2,
            true,
            min_boot_free_bytes,
            min_boot_free_inodes,
            CryptsetupLocation::usr_bin,
            tools({"/usr/bin/mkinitcpio", "/usr/bin/lsinitcpio", "/usr/bin/findmnt", "/usr/lib/systemd/systemd",
                   "/usr/bin/grub-mkconfig", "/usr/bin/grub-probe", "/usr/bin/cryptsetup"}),
            {{"/usr/lib/initcpio/functions", true, true, false, true},
             {"/usr/lib/initcpio/init", true, true, false, false},
             {"/usr/lib/initcpio/hooks/encrypt", true, true, false, false},
             {"/usr/lib/initcpio/install/encrypt", true, true, false, false}},
            mkinitcpio_source,
            0644,
            "ALL_kver='/boot/vmlinuz-linux'\nPRESETS=('default')\n"
            "default_image='/boot/initramfs-linux.img'\n",
            "/boot/initramfs-linux.img",
            digest,
            1024,
            "1625-E85D",
            "root=/dev/mapper/crypt-root ro quiet"};
        const auto mkinitcpio_contract = plan_mkinitcpio_systemd(mkinitcpio_facts);
        CHECK(std::string(reinterpret_cast<const char *>(mkinitcpio_contract.config_activated.data()),
                          mkinitcpio_contract.config_activated.size())
                  .contains("encrypt sart filesystems"));
        std::vector<ArchiveEntry> mkinitcpio_entries{file("usr/bin/sart", 0755, elf),
                                                     file("init", 0755, text_bytes("init")),
                                                     file("hooks/encrypt", 0755, text_bytes("encrypt")),
                                                     file("usr/bin/cryptsetup", 0755, text_bytes("cryptsetup"))};
        for (const auto [path, id] :
             std::array{std::pair<std::string_view, sart::embedded::TemplateId>{
                            "hooks/sart", sart::embedded::TemplateId::mkinitcpio_runtime_hook},
                        std::pair<std::string_view, sart::embedded::TemplateId>{
                            "usr/bin/plymouth", sart::embedded::TemplateId::mkinitcpio_plymouth_bridge}}) {
            const auto resource = sart::embedded::template_resource(id);
            mkinitcpio_entries.push_back(
                file(std::string(path), resource.materialization.mode, text_bytes(resource.contents)));
        }
        const auto mkinitcpio_inspection = inspect_mkinitcpio_inventory(mkinitcpio_entries, elf);
        CHECK(verified_mkinitcpio_systemd_image_record(mkinitcpio_contract, candidate_bytes, mkinitcpio_inspection, elf)
                  .kernel_version == "linux");

        const std::string init_source = "#!/bin/sh\nVERSION=3.14.0-r0\n\n"
                                        "# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n"
                                        "\t$MOCK nlplug-findfs\n"
                                        "\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n"
                                        "\t\tfi\n\tdone\n\t$MOCK sync\n";
        const std::string mkinitfs_config = "features=\"base ext4 virtio\"\n";
        const auto path_fact = [](std::string path, std::uint16_t mode, std::string_view contents) {
            return MkinitfsOpenRcPathFact{std::move(path), true, true, false, false, mode, sart::sha256(contents)};
        };
        MkinitfsOpenRcFacts mkinitfs_facts{
            std::string(product_architecture),
            "init",
            {"6.18.35-0-virt"},
            true,
            min_boot_free_bytes,
            min_boot_free_inodes,
            tools({"/sbin/mkinitfs", "/sbin/update-extlinux", "/sbin/extlinux", "/sbin/openrc"}),
            {path_fact("/usr/share/mkinitfs/initramfs-init", 0644, init_source),
             path_fact("/etc/mkinitfs/mkinitfs.conf", 0644, mkinitfs_config),
             path_fact("/etc/update-extlinux.conf", 0644, "safe"), path_fact("/boot/extlinux.conf", 0644, "safe"),
             path_fact("/boot/vmlinuz-virt", 0644, "kernel")},
            init_source,
            mkinitfs_config,
            {"base", "ext4", "virtio"},
            true,
            "virt",
            "root=LABEL=/ modules=ext4,virtio console=ttyS0,115200n8",
            "/boot/initramfs-virt",
            digest,
            1024};
        const auto mkinitfs_contract = plan_mkinitfs_openrc(mkinitfs_facts);
        CHECK(mkinitfs_contract.generate.arguments[1] == "none");
        CHECK(mkinitfs_openrc_managed_image_path(mkinitfs_contract.known_good_image));
        CHECK(parse_mkinitfs_features(mkinitfs_config).size() == 3);
        CHECK(text_bytes("features=\"base ext4 virtio sart\"\n") == activate_mkinitfs_sart_feature(mkinitfs_config));
        const auto settings =
            parse_update_extlinux_settings("overwrite=1\ndefault=virt\nroot=LABEL=/\nmodules=ext4,virtio\n"
                                           "default_kernel_opts=console=ttyS0,115200n8\n");
        CHECK(settings.overwrite && settings.default_label == "virt");
        CHECK(parse_extlinux_entry_command_line("LABEL virt\n  APPEND root=LABEL=/ quiet\n", "virt") ==
              "root=LABEL=/ quiet");
        const auto patched_init = sart::integration::patch_mkinitfs_init(init_source);
        CHECK(patched_init.has_value());
        const auto runtime_bytes = text_bytes(sart::integration::mkinitfs::runtime_hook);
        const auto findfs_bytes = text_bytes(sart::integration::mkinitfs::findfs_wrapper);
        const auto patched_bytes = text_bytes(*patched_init);
        const std::array mkinitfs_inputs{
            sart::integration::CpioInput{"usr/bin/sart", elf, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/mkinitfs-runtime", runtime_bytes, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/mkinitfs-findfs", findfs_bytes, 0100755},
            sart::integration::CpioInput{"init", patched_bytes, 0100755}};
        const auto mkinitfs_archive = sart::integration::build_cpio_archive(mkinitfs_inputs);
        const auto mkinitfs_inspection = inspect_mkinitfs_openrc_archive(mkinitfs_archive, elf);
        CHECK(verified_mkinitfs_openrc_image_record(mkinitfs_contract, mkinitfs_archive, mkinitfs_inspection, elf)
                  .kernel_version == "6.18.35-0-virt");

        const std::string pristine_functions = R"SART(prefix
unlock_root_partition() {
	command -v cryptsetup >/dev/null || return
	if cryptsetup isLuks "$PMOS_ROOT"; then
		splash_hide
		tried=0
		until cryptsetup status root | grep -qwi active; do
			fde-unlock "$PMOS_ROOT" "$tried"
			tried=$((tried + 1))
		done
		PMOS_ROOT=/dev/mapper/root
		splash_set_message "Loading"
	fi
}
suffix
)SART";
        MkinitfsBootDeployFacts deploy_facts{
            std::string(product_architecture),
            "init",
            1,
            2,
            true,
            64 * 1024 * 1024,
            4096,
            min_boot_free_inodes * 2,
            min_boot_free_inodes,
            tools({"/usr/sbin/mkinitfs", "/usr/bin/boot-deploy", "/usr/sbin/openrc"}),
            {{"/usr/share/initramfs/init.sh", true, true, false, true},
             {"/usr/share/initramfs/init_2nd.sh", true, true, false, true},
             {"/usr/share/initramfs/init_functions_2nd.sh", true, true, false, false},
             {"/usr/share/boot-deploy/boot-deploy-functions.sh", true, true, false, true},
             {"/usr/share/boot-deploy/os-customization", true, true, false, false}},
            std::string(sart::integration::reviewed_boot_deploy_initramfs_version),
            pristine_functions,
            "/boot/vmlinuz",
            8 * 1024 * 1024,
            "/boot/initramfs",
            MkinitfsBootDeployCompression::zstandard,
            digest,
            4096,
            "/boot/loader/entries/current.conf",
            0644,
            text_bytes("title Mobile Linux\nlinux vmlinuz\ninitrd initramfs\n"
                       "options quiet splash console=ttyAMA0 root=/dev/mapper/root\n"),
            "quiet splash console=ttyAMA0 root=/dev/mapper/root",
            std::nullopt};
        const auto deploy_contract = plan_mkinitfs_boot_deploy(deploy_facts, false);
        CHECK(deploy_contract.generate.arguments == std::vector<std::string>({"-d", "/boot/.sart-candidate"}));
        CHECK(text_bytes("title Mobile Linux\nlinux vmlinuz\ninitrd initramfs\n"
                         "options quiet console=ttyAMA0 root=/dev/mapper/root\n") ==
              deploy_contract.active_loader_entry_activated);
        CHECK(parse_mkinitfs_boot_deploy_loader_entry(
                  "title current\nlinux vmlinuz\ninitrd initramfs\noptions quiet root=/dev/mapper/root\n")
                  .first == "/boot/vmlinuz");
        CHECK(parse_mkinitfs_boot_deploy_version("INITRAMFS_PKG_VERSION=\"3.12.0-r0\"\n") == "3.12.0-r0");
        CHECK(mkinitfs_boot_deploy_initial_boot_bytes(4097, 1, 4096) == 16384);
        CHECK(mkinitfs_boot_deploy_preservation_bytes(4097, 1, 4096) == 12288);
        CHECK(mkinitfs_boot_deploy_managed_image_path("/boot/.sart-candidate/boot.img"));
        const std::array zstd_magic{std::byte{0x28}, std::byte{0xb5}, std::byte{0x2f}, std::byte{0xfd}};
        CHECK(detect_mkinitfs_boot_deploy_compression(zstd_magic) == MkinitfsBootDeployCompression::zstandard);
        const auto decoder_payload = text_bytes("bounded-newc-payload");
        std::vector<std::byte> zstd_compressed(ZSTD_compressBound(decoder_payload.size()));
        const auto zstd_size = ZSTD_compress(zstd_compressed.data(), zstd_compressed.size(), decoder_payload.data(),
                                             decoder_payload.size(), 1);
        CHECK(!ZSTD_isError(zstd_size));
        zstd_compressed.resize(zstd_size);
        CHECK(decompress_mkinitfs_boot_deploy_archive(zstd_compressed, MkinitfsBootDeployCompression::zstandard) ==
              decoder_payload);
        z_stream gzip_stream{};
        CHECK(deflateInit2(&gzip_stream, Z_BEST_SPEED, Z_DEFLATED, 15 + 16, 8, Z_DEFAULT_STRATEGY) == Z_OK);
        std::vector<std::byte> gzip_compressed(compressBound(decoder_payload.size()) + 32);
        gzip_stream.next_in = reinterpret_cast<Bytef *>(const_cast<std::byte *>(decoder_payload.data()));
        gzip_stream.avail_in = decoder_payload.size();
        gzip_stream.next_out = reinterpret_cast<Bytef *>(gzip_compressed.data());
        gzip_stream.avail_out = gzip_compressed.size();
        CHECK(deflate(&gzip_stream, Z_FINISH) == Z_STREAM_END);
        gzip_compressed.resize(gzip_stream.total_out);
        CHECK(deflateEnd(&gzip_stream) == Z_OK);
        CHECK(decompress_mkinitfs_boot_deploy_archive(gzip_compressed, MkinitfsBootDeployCompression::gzip) ==
              decoder_payload);
        auto zstd_trailing = zstd_compressed;
        zstd_trailing.insert(zstd_trailing.end(), zstd_compressed.begin(), zstd_compressed.end());
        bool rejected_concatenated = false;
        try {
            static_cast<void>(
                decompress_mkinitfs_boot_deploy_archive(zstd_trailing, MkinitfsBootDeployCompression::zstandard));
        } catch (const std::runtime_error &) {
            rejected_concatenated = true;
        }
        CHECK(rejected_concatenated);

        const auto patched_deploy = sart::integration::patch_boot_deploy_init_functions(
            pristine_functions, sart::integration::reviewed_boot_deploy_initramfs_version);
        CHECK(patched_deploy.has_value());
        const auto deploy_runtime = text_bytes(sart::integration::mkinitfs_boot_deploy::runtime_hook);
        const auto deploy_fde = text_bytes(sart::integration::mkinitfs_boot_deploy::fde_wrapper);
        const auto deploy_stock = text_bytes(sart::integration::mkinitfs_boot_deploy::stock_fde_unlock);
        const auto deploy_unl0kr = text_bytes(sart::integration::mkinitfs_boot_deploy::native_unl0kr);
        const auto deploy_start = text_bytes(sart::integration::mkinitfs_boot_deploy::start_hook);
        const auto deploy_cleanup = text_bytes(sart::integration::mkinitfs_boot_deploy::cleanup_hook);
        const auto patched_deploy_bytes = text_bytes(*patched_deploy);
        const std::array deploy_inputs{
            sart::integration::CpioInput{".", {}, 0040755},
            sart::integration::CpioInput{"usr/libexec/sart", {}, 0040755},
            sart::integration::CpioInput{"usr/libexec/sart/native-bin", {}, 0040755},
            sart::integration::CpioInput{"usr/bin/sart", elf, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/mkinitfs-boot-deploy-runtime", deploy_runtime, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/mkinitfs-boot-deploy-fde", deploy_fde, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/fde-unlock-stock", deploy_stock, 0100755},
            sart::integration::CpioInput{"usr/libexec/sart/native-bin/unl0kr", deploy_unl0kr, 0100755},
            sart::integration::CpioInput{"hooks-extra/50-sart-start.sh", deploy_start, 0100755},
            sart::integration::CpioInput{"hooks-cleanup/90-sart-handoff.sh", deploy_cleanup, 0100755},
            sart::integration::CpioInput{"init_functions_2nd.sh", patched_deploy_bytes, 0100644}};
        const auto deploy_archive = sart::integration::build_cpio_archive(deploy_inputs);
        const auto deploy_inspection = inspect_mkinitfs_boot_deploy_archive(deploy_archive, elf);
        CHECK(verified_mkinitfs_boot_deploy_image_record(deploy_contract, zstd_magic, deploy_inspection, elf)
                  .kernel_version == "vmlinuz");
        const auto pristine_bytes = text_bytes(pristine_functions);
        const auto shell_bytes = text_bytes("#!/bin/sh\n");
        const std::array clean_inputs{sart::integration::CpioInput{"init_functions_2nd.sh", pristine_bytes, 0100644},
                                      sart::integration::CpioInput{"usr/bin/fde-unlock", shell_bytes, 0100755},
                                      sart::integration::CpioInput{"usr/bin/unl0kr", shell_bytes, 0100755},
                                      sart::integration::CpioInput{"usr/sbin/cryptsetup", shell_bytes, 0100755}};
        CHECK(inspect_sart_free_mkinitfs_boot_deploy_archive(sart::integration::build_cpio_archive(clean_inputs))
                  .inspected_entries == 4);

        const std::string deviceinfo = "deviceinfo_arch='" + std::string(product_architecture) +
                                       "'\n"
                                       "deviceinfo_codename='proof-phone'\n"
                                       "deviceinfo_generate_bootimg='true'\n"
                                       "deviceinfo_flash_kernel_on_update='true'\n"
                                       "deviceinfo_flash_method='fastboot'\n"
                                       "deviceinfo_dtb='qcom/proof-phone'\n"
                                       "deviceinfo_header_version='2'\n";
        const auto android = parse_android_boot_deviceinfo(deviceinfo);
        CHECK(android.has_value());
        CHECK(text_bytes("false") != android->no_flash_deviceinfo);
        CHECK(android_slot_partition_label("quiet androidboot.slot_suffix=_a", "boot") == "boot_a");
        CHECK(deviceinfo_enables_kernel_flash(deviceinfo));
        CHECK(deviceinfo_generates_android_boot_image(deviceinfo));
        const auto android_kernel = text_bytes("kernel");
        const auto android_ramdisk = text_bytes("ramdisk");
        const auto android_dtb = text_bytes("dtb");
        constexpr std::size_t android_header = 1660;
        constexpr std::size_t android_page = 4096;
        const auto align_page = [](std::size_t value) {
            return (value + android_page - 1) / android_page * android_page;
        };
        const auto kernel_end = android_page + android_kernel.size();
        const auto ramdisk_start = align_page(kernel_end);
        const auto ramdisk_end = ramdisk_start + android_ramdisk.size();
        const auto dtb_start = align_page(ramdisk_end);
        const auto dtb_end = dtb_start + android_dtb.size();
        std::vector<std::byte> android_image(align_page(dtb_end));
        std::ranges::copy(text_bytes("ANDROID!"), android_image.begin());
        const auto put32 = [&android_image](std::size_t offset, std::uint32_t value) {
            for (std::size_t index = 0; index < 4; ++index) {
                android_image[offset + index] = std::byte(static_cast<unsigned char>(value >> (8 * index)));
            }
        };
        put32(8, android_kernel.size());
        put32(16, android_ramdisk.size());
        put32(24, 0);
        put32(36, android_page);
        put32(40, 2);
        put32(1632, 0);
        put32(1644, android_header);
        put32(1648, android_dtb.size());
        std::ranges::copy(android_kernel, android_image.begin() + android_page);
        std::ranges::copy(android_ramdisk, android_image.begin() + ramdisk_start);
        std::ranges::copy(android_dtb, android_image.begin() + dtb_start);
        CHECK(inspect_android_boot_image_v2(android_image, android_kernel, android_ramdisk, android_dtb).page_size ==
              android_page);
        std::vector<std::byte> original_partition(16384, std::byte{0x33});
        auto partition = original_partition;
        const auto activated_partition_digest =
            activate_android_boot_partition(partition, original_partition, android_image);
        CHECK(activated_partition_digest == sart::sha256(std::span(partition).first(original_partition.size())));
        CHECK(std::ranges::equal(std::span(partition).subspan(android_image.size()),
                                 std::span(original_partition).subspan(android_image.size())));
        restore_android_boot_partition(partition, original_partition);
        CHECK(partition == original_partition);
    }

    void splash_state_contracts() {
        using namespace sart::splash;
        auto state = running_state();
        CHECK(state.lifecycle() == Lifecycle::running);
        CHECK(state.view().base_view() == BaseView::splash);
        CHECK(state.mode() == Mode::boot);
        CHECK(state.root_stage() == RootStage::initramfs);
        CHECK(state.apply(SetMode{Mode::update}) == TransitionResult::changed);
        CHECK(state.apply(SetStatus{std::string("Mounting filesystems")}) == TransitionResult::changed);
        CHECK(state.apply(SetProgress{37}) == TransitionResult::changed);
        CHECK(state.apply(SetProgress{37}) == TransitionResult::unchanged);

        CHECK(state.apply(Hide{}) == TransitionResult::changed);
        const auto metadata = prompt(71);
        CHECK(state.apply(BeginPrompt{metadata}) == TransitionResult::changed);
        CHECK(state.view().prompt_metadata()->request_id() == 71);
        CHECK(state.apply(Show{}) == TransitionResult::unchanged);
        check_state_error(StateErrorCode::prompt_active, [&] { state.apply(Deactivate{}); });
        CHECK(state.apply(FinishPrompt{71, PromptOutcome::answered}) == TransitionResult::changed);
        CHECK(state.view().base_view() == BaseView::hidden);
        CHECK(state.apply(FinishPrompt{71, PromptOutcome::answered}) == TransitionResult::unchanged);

        auto conflicting = running_state();
        conflicting.apply(BeginPrompt{prompt(10)});
        const auto before = conflicting;
        check_state_error(StateErrorCode::prompt_conflict, [&] { conflicting.apply(BeginPrompt{prompt(11)}); });
        CHECK(conflicting == before);
        check_state_error(StateErrorCode::prompt_id_mismatch,
                          [&] { conflicting.apply(FinishPrompt{11, PromptOutcome::cancelled}); });
        CHECK(conflicting == before);

        auto roots = running_state();
        check_state_error(StateErrorCode::invalid_root_transition,
                          [&] { roots.apply(SetRootStage{RootStage::real_root}); });
        CHECK(roots.apply(SetRootStage{RootStage::switching}) == TransitionResult::changed);
        CHECK(roots.apply(SetRootStage{RootStage::switching}) == TransitionResult::unchanged);
        CHECK(roots.apply(SetRootStage{RootStage::real_root}) == TransitionResult::changed);

        auto atomic = running_state();
        atomic.apply(SetStatus{std::string("safe")});
        const auto atomic_before = atomic;
        check_state_error(StateErrorCode::invalid_text,
                          [&] { atomic.apply(SetStatus{std::string("unsafe\nstatus")}); });
        CHECK(atomic == atomic_before);

        auto quitting = running_state();
        quitting.apply(BeginPrompt{prompt(123)});
        CHECK(quitting.apply(Quit{}) == TransitionResult::changed);
        CHECK(quitting.lifecycle() == Lifecycle::quitting);
        CHECK(quitting.view().base_view() == BaseView::splash);
        CHECK(quitting.apply(MarkStopped{}) == TransitionResult::changed);
        check_state_error(StateErrorCode::invalid_lifecycle_transition,
                          [&] { quitting.apply(SetMessage{std::string("too late")}); });
    }

    template <typename Function>
    void check_protocol_error(sart::splash::ProtocolErrorCode expected, Function &&function) {
        try {
            function();
        } catch (const sart::splash::ProtocolError &error) {
            CHECK(error.code() == expected);
            return;
        }
        throw TestFailure("expected ProtocolError");
    }

    std::vector<std::uint8_t> raw_frame(std::uint16_t version, std::uint16_t opcode, std::uint32_t flags,
                                        std::uint64_t request_id, std::span<const std::uint8_t> payload) {
        std::vector<std::uint8_t> output{
            'B',
            'A',
            'R',
            'T',
            static_cast<std::uint8_t>(version >> 8),
            static_cast<std::uint8_t>(version),
            static_cast<std::uint8_t>(opcode >> 8),
            static_cast<std::uint8_t>(opcode),
        };
        for (int shift = 24; shift >= 0; shift -= 8)
            output.push_back(static_cast<std::uint8_t>(flags >> shift));
        for (int shift = 56; shift >= 0; shift -= 8)
            output.push_back(static_cast<std::uint8_t>(request_id >> shift));
        const auto length = static_cast<std::uint32_t>(payload.size());
        for (int shift = 24; shift >= 0; shift -= 8)
            output.push_back(static_cast<std::uint8_t>(length >> shift));
        output.insert(output.end(), payload.begin(), payload.end());
        return output;
    }

    void protocol_contracts() {
        using namespace sart::splash;
        const auto frame = Frame::text(Opcode::status, 0x0102030405060708ULL, "ready");
        const auto encoded = frame.encode();
        CHECK(std::equal(protocol_magic.begin(), protocol_magic.end(), encoded.begin()));
        CHECK(encoded[4] == 0 && encoded[5] == 1);
        CHECK(encoded[6] == 0 && encoded[7] == 4);
        CHECK(encoded[12] == 1 && encoded[19] == 8);
        CHECK(encoded[23] == 5);
        CHECK(Frame::decode_exact(encoded) == frame);
        CHECK(Frame::progress(8, 73).progress_value() == 73);
        CHECK(Frame::mode(9, Mode::upgrade).mode_value() == Mode::upgrade);
        CHECK(Frame::quit(10, true).retains_splash());

        const std::array<std::uint8_t, 3> short_header{'B', 'A', 'R'};
        check_protocol_error(ProtocolErrorCode::truncated, [&] { Frame::decode_exact(short_header); });
        auto trailing = Frame::empty(Opcode::ping, 11).encode();
        trailing.push_back(0);
        check_protocol_error(ProtocolErrorCode::trailing_bytes, [&] { Frame::decode_exact(trailing); });
        auto unsupported = raw_frame(protocol_version + 1, static_cast<std::uint16_t>(Opcode::ping), 0, 1, {});
        check_protocol_error(ProtocolErrorCode::unsupported_version, [&] { Frame::decode_exact(unsupported); });
        auto unknown = raw_frame(protocol_version, 0x7777, 0, 1, {});
        check_protocol_error(ProtocolErrorCode::unknown_opcode, [&] { Frame::decode_exact(unknown); });

        std::vector<std::uint8_t> oversized(maximum_payload_length + 1, 'x');
        check_protocol_error(ProtocolErrorCode::payload_too_large,
                             [&] { Frame(Opcode::state_result, 0, 1, oversized); });
        const std::array<std::uint8_t, 1> invalid_utf8{0xff};
        auto invalid = raw_frame(protocol_version, static_cast<std::uint16_t>(Opcode::status), 0, 1, invalid_utf8);
        check_protocol_error(ProtocolErrorCode::invalid_utf8, [&] { Frame::decode_exact(invalid); });
        check_protocol_error(ProtocolErrorCode::invalid_text, [&] { Frame::text(Opcode::message, 1, "hello\x1b[2J"); });
        check_protocol_error(ProtocolErrorCode::invalid_progress, [&] { Frame::progress(1, 101); });
        check_protocol_error(ProtocolErrorCode::invalid_payload_length, [&] { Frame(Opcode::ping, 0, 1, {'x'}); });
        check_protocol_error(ProtocolErrorCode::flags_not_allowed,
                             [&] { Frame(Opcode::show, retain_splash_flag, 1, {}); });
        check_protocol_error(ProtocolErrorCode::invalid_root_path,
                             [&] { Frame::text(Opcode::update_root_fs, 1, "relative/root"); });
    }

    void command_mapping_contracts() {
        using namespace sart::splash;
        auto state = running_state();
        auto status = handle_request(state, Frame::text(Opcode::status, 1, "Mounting filesystems"));
        CHECK(status.response.opcode() == Opcode::ack);
        CHECK(state.status() == "Mounting filesystems");
        auto root = handle_request(state, Frame::text(Opcode::update_root_fs, 2, "/sysroot"));
        CHECK(root.response.opcode() == Opcode::ack);
        CHECK(state.root_stage() == RootStage::real_root);

        state.apply(SetMessage{std::string("quote: \" and slash: \\")});
        const auto json = state_json(state);
        CHECK(json.find("quote: \\\" and slash: \\\\") != std::string::npos);
        CHECK(json.find("secret") == std::string::npos);
        CHECK(!is_mutating(Opcode::ping));
        CHECK(!is_mutating(Opcode::state));
        CHECK(!is_mutating(Opcode::native_ready));
        CHECK(is_mutating(Opcode::show));

        const auto readiness = handle_request(state, Frame::empty(Opcode::native_ready, 27));
        CHECK(readiness.response.opcode() == Opcode::error);
        auto quitting = running_state();
        const auto quit = handle_request(quitting, Frame::quit(10, true));
        CHECK(quit.response.opcode() == Opcode::ack);
        CHECK(quit.should_quit);
        CHECK(quit.retain_splash);
    }

    void runtime_and_client_contracts() {
        using namespace sart::splash;
        const auto root = std::filesystem::path(SART_SOURCE_ROOT) / "target/cpp";
        const auto runtime = root / ("runtime-test-" + std::to_string(getpid()));
        std::error_code ignored;
        std::filesystem::remove_all(runtime, ignored);
        const RuntimePaths paths(runtime);
        {
            auto owner = RuntimeOwner::acquire(paths);
            struct stat directory_metadata{};
            CHECK(stat(paths.directory().c_str(), &directory_metadata) == 0);
            CHECK((directory_metadata.st_mode & 0777) == 0700);
            try {
                auto duplicate = RuntimeOwner::acquire(paths);
                static_cast<void>(duplicate);
                throw TestFailure("duplicate runtime owner was accepted");
            } catch (const RuntimeError &error) {
                CHECK(error.code() == RuntimeErrorCode::already_running);
            }
            CHECK(owner.owned_entries_reachable());
            auto listener = owner.bind_listener();
            auto native = owner.bind_native_password_listener();
            int socket_type{};
            socklen_t socket_type_length = sizeof(socket_type);
            CHECK(getsockopt(native.get(), SOL_SOCKET, SO_TYPE, &socket_type, &socket_type_length) == 0);
            CHECK(socket_type == SOCK_SEQPACKET);
            CHECK(owner.owned_entries_reachable());

            std::thread server([&] {
                FileDescriptor connection(accept4(listener.get(), nullptr, nullptr, SOCK_CLOEXEC));
                CHECK(static_cast<bool>(connection));
                const auto credentials = peer_credentials(connection.get());
                CHECK(credentials.uid == effective_uid());
                auto request = Frame::read_exact_message(connection.get());
                auto state = running_state();
                const auto outcome = handle_request(state, request);
                outcome.response.write_to_fd(connection.get());
                shutdown(connection.get(), SHUT_WR);
            });
            const ClientConfig client(paths);
            const auto request = Frame::empty(Opcode::ping, next_request_id());
            const auto response = send_request(client, request);
            CHECK(response.opcode() == Opcode::pong);
            CHECK(response.request_id() == request.request_id());
            server.join();

            std::thread cli_server([&] {
                FileDescriptor connection(accept4(listener.get(), nullptr, nullptr, SOCK_CLOEXEC));
                auto cli_request = Frame::read_exact_message(connection.get());
                CHECK(cli_request.opcode() == Opcode::ping);
                Frame::pong(cli_request.request_id()).write_to_fd(connection.get());
                shutdown(connection.get(), SHUT_WR);
            });
            const auto *binary = std::getenv("SART_BINARY");
            CHECK(binary != nullptr);
            const auto command =
                std::string(binary) + " ping --runtime-dir " + runtime.string() + " >/tmp/sart-cpp-ping.out";
            CHECK(std::system(command.c_str()) == 0);
            CHECK(read_file("/tmp/sart-cpp-ping.out") == "pong\n");
            cli_server.join();
        }
        CHECK(!std::filesystem::exists(paths.lock()));
        CHECK(!std::filesystem::exists(paths.socket()));
        CHECK(!std::filesystem::exists(paths.native_password_socket()));
        CHECK(!std::filesystem::exists(paths.directory()));
    }

    void daemon_process_contracts() {
        using namespace sart::splash;
        const auto *binary = std::getenv("SART_BINARY");
        CHECK(binary != nullptr);
        const auto runtime =
            std::filesystem::path(SART_SOURCE_ROOT) / "target/cpp" / ("daemon-test-" + std::to_string(getpid()));
        std::error_code ignored;
        std::filesystem::remove_all(runtime, ignored);
        const auto child = fork();
        CHECK(child >= 0);
        if (child == 0) {
            execl(binary, binary, "daemon", "--test-buffer", "--runtime-dir", runtime.c_str(),
                  static_cast<char *>(nullptr));
            _exit(127);
        }
        const RuntimePaths paths(runtime);
        for (int attempt = 0; attempt < 200 && !std::filesystem::exists(paths.socket()); ++attempt) {
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
        CHECK(std::filesystem::exists(paths.socket()));
        const ClientConfig client(paths);
        const auto ping = Frame::empty(Opcode::ping, next_request_id());
        CHECK(send_request(client, ping).opcode() == Opcode::pong);
        const auto state = Frame::empty(Opcode::state, next_request_id());
        const auto snapshot = send_request(client, state);
        CHECK(snapshot.opcode() == Opcode::state_result);
        CHECK(snapshot.payload_text().find("\"lifecycle\":\"running\"") != std::string_view::npos);
        const auto quit = Frame::quit(next_request_id(), false);
        CHECK(send_request(client, quit).opcode() == Opcode::ack);
        int status{};
        CHECK(waitpid(child, &status, 0) == child);
        CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
        CHECK(!std::filesystem::exists(runtime));
    }

    void binary_smoke() {
        const auto *binary = std::getenv("SART_BINARY");
        CHECK(binary != nullptr);
        const auto version = std::string(binary) + " --version >/tmp/sart-cpp-version.out";
        CHECK(std::system(version.c_str()) == 0);
        CHECK(read_file("/tmp/sart-cpp-version.out") == "sart 0.1.0\n");
        const auto render =
            std::string(binary) + " render-final --no-color --cols 10 --rows 5 >/tmp/sart-cpp-render.out";
        CHECK(std::system(render.c_str()) == 0);
        const auto output = read_file("/tmp/sart-cpp-render.out");
        CHECK(output.find("\x1b[?25l") != std::string::npos);
        CHECK(output.find("\x1b[?25h") != std::string::npos);
    }

} // namespace

int main() {
    const std::array tests{
        std::pair{"art validation and layout", &art_validation_and_layout},
        std::pair{"animation determinism", &animation_is_deterministic},
        std::pair{"renderer golden parity", &renderer_matches_rust_goldens},
        std::pair{"display and frame engine", &display_and_frame_engine_contracts},
        std::pair{"Linux text VT backend", &text_vt_backend_contracts},
        std::pair{"splash engine", &splash_engine_contracts},
        std::pair{"password memory and input", &password_memory_and_input_contracts},
        std::pair{"systemd password transport", &systemd_password_contracts},
        std::pair{"digest and CPIO inspection", &digest_and_cpio_contracts},
        std::pair{"native password transport", &native_password_transport_contracts},
        std::pair{"native password coordinator", &native_password_coordinator_contracts},
        std::pair{"embedded and cmdline", &embedded_and_cmdline_contracts},
        std::pair{"installer transaction", &installer_transaction_contracts},
        std::pair{"installer backends", &installer_backend_contracts},
        std::pair{"splash state", &splash_state_contracts},
        std::pair{"control protocol", &protocol_contracts},
        std::pair{"command mapping", &command_mapping_contracts},
        std::pair{"runtime and client", &runtime_and_client_contracts},
        std::pair{"daemon process", &daemon_process_contracts},
        std::pair{"binary smoke", &binary_smoke},
    };
    std::size_t failures{};
    for (const auto &[name, test] : tests) {
        try {
            test();
            std::cout << "PASS: " << name << '\n';
        } catch (const std::exception &error) {
            ++failures;
            std::cerr << "FAIL: " << name << ": " << error.what() << '\n';
        }
    }
    if (failures != 0) {
        std::cerr << failures << " C++ test group(s) failed\n";
        return 1;
    }
    std::cout << "PASS: all C++ core tests\n";
    return 0;
}
