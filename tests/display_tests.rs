use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use bootart::display::buffer::{BufferBackend, BufferOperation};
use bootart::display::{
    Cell, Dimensions, DisplayBackend, DisplayError, DisplayState, InputEvent, RestoreMode, Scene,
    SceneError, Style,
};

#[test]
fn scene_is_bounded_and_contains_no_terminal_controls() {
    assert!(matches!(
        Dimensions::new(0, 24),
        Err(SceneError::EmptyDimensions { .. })
    ));
    assert!(matches!(
        Cell::new('\u{1b}'),
        Err(SceneError::ControlGlyph { codepoint: 0x1b })
    ));

    let mut scene = Scene::from_rows(&["ab", "c"]).unwrap();
    assert_eq!(scene.dimensions(), Dimensions::new(2, 2).unwrap());
    assert_eq!(scene.get(1, 1).unwrap().glyph(), ' ');
    scene.set(1, 1, Cell::new('d').unwrap()).unwrap();
    assert_eq!(scene.get(1, 1).unwrap().glyph(), 'd');
    assert!(matches!(
        scene.set(2, 0, Cell::default()),
        Err(SceneError::OutOfBounds { .. })
    ));
}

#[test]
fn raw_input_debug_is_redacted() {
    let event = InputEvent::Bytes(b"not-for-logs".to_vec());
    let debug = format!("{event:?}");
    assert!(debug.contains("redacted"));
    assert!(!debug.contains("not-for-logs"));
}

#[test]
fn sensitive_text_bounds_are_conservative_for_non_ascii_cells() {
    let mut display = BufferBackend::new(Dimensions::new(3, 1).unwrap());
    display.acquire().unwrap();
    display.show().unwrap();
    assert!(matches!(
        display.render_sensitive_text(0, 0, "界界", Style::default()),
        Err(DisplayError::SensitiveTextOutOfBounds)
    ));
}

#[test]
fn buffer_backend_enforces_the_display_lifecycle_deterministically() {
    let size = Dimensions::new(2, 1).unwrap();
    let scene = Scene::from_rows(&["ok"]).unwrap();
    let mut display = BufferBackend::new(size);

    assert!(matches!(
        display.render(&scene),
        Err(DisplayError::InvalidState {
            state: DisplayState::Unacquired,
            ..
        })
    ));
    display.acquire().unwrap();
    assert_eq!(display.state(), DisplayState::Hidden);
    display.show().unwrap();
    display.show().unwrap();
    display.render(&scene).unwrap();

    display.queue_input(InputEvent::Bytes(vec![0x1b]));
    assert_eq!(
        display.poll_input(Duration::from_millis(5)).unwrap(),
        Some(InputEvent::Bytes(vec![0x1b]))
    );
    display.details(true).unwrap();
    assert_eq!(display.state(), DisplayState::Details);
    assert_eq!(display.poll_input(Duration::ZERO).unwrap(), None);
    display.queue_input(InputEvent::ReturnToSplash);
    assert_eq!(
        display.poll_input(Duration::ZERO).unwrap(),
        Some(InputEvent::ReturnToSplash)
    );
    display.details(false).unwrap();
    display.hide().unwrap();
    display.restore().unwrap();
    display.restore().unwrap();

    assert_eq!(display.frames(), &[scene]);
    assert_eq!(
        display.operations(),
        &[
            BufferOperation::Acquire,
            BufferOperation::Show,
            BufferOperation::Render,
            BufferOperation::PollInput(Duration::from_millis(5)),
            BufferOperation::Details(true),
            BufferOperation::PollInput(Duration::ZERO),
            BufferOperation::PollInput(Duration::ZERO),
            BufferOperation::Details(false),
            BufferOperation::Hide,
            BufferOperation::Restore(RestoreMode::Clear),
        ]
    );
}

#[cfg(target_os = "linux")]
mod linux_text_vt {
    use super::*;
    use bootart::display::text_vt::{TextVtBackend, TextVtConfig, VtIo};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Device {
        Control,
        Splash(u16),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        OpenControl(String),
        ActiveVt,
        OpenQuery,
        OpenVt(String, u16),
        Dimensions,
        TerminalState,
        SetRaw,
        RestoreTerminal,
        KdMode,
        SetKd(i32),
        KeyboardMode,
        SetKeyboard(i32),
        Activate(u16),
        WaitActive(u16, Duration),
        Disallocate(u16),
        Write(Vec<u8>),
        WriteSensitive(usize),
        Flush,
        Poll(Duration),
    }

    #[derive(Debug)]
    struct FakeState {
        calls: Vec<Call>,
        original_vt: u16,
        free_vt: u16,
        active_vt: u16,
        dimensions: Dimensions,
        kd_mode: i32,
        keyboard_mode: i32,
        input: VecDeque<Vec<u8>>,
        fail_frame_write_once: bool,
        fail_restore_terminal: bool,
    }

    impl Default for FakeState {
        fn default() -> Self {
            Self {
                calls: Vec::new(),
                original_vt: 1,
                free_vt: 7,
                active_vt: 1,
                dimensions: Dimensions::new(3, 2).unwrap(),
                kd_mode: 1,
                keyboard_mode: 1,
                input: VecDeque::new(),
                fail_frame_write_once: false,
                fail_restore_terminal: false,
            }
        }
    }

    #[derive(Clone)]
    struct FakeIo {
        state: Rc<RefCell<FakeState>>,
    }

    impl FakeIo {
        fn new() -> (Self, Rc<RefCell<FakeState>>) {
            let state = Rc::new(RefCell::new(FakeState::default()));
            (
                Self {
                    state: Rc::clone(&state),
                },
                state,
            )
        }

        fn push(&self, call: Call) {
            self.state.borrow_mut().calls.push(call);
        }
    }

    impl VtIo for FakeIo {
        type Device = Device;
        type TerminalState = u32;

        fn open_control(&mut self, path: &Path) -> io::Result<Self::Device> {
            self.push(Call::OpenControl(path.display().to_string()));
            Ok(Device::Control)
        }

        fn active_vt(&mut self, _control: &Self::Device) -> io::Result<u16> {
            self.push(Call::ActiveVt);
            Ok(self.state.borrow().active_vt)
        }

        fn open_query(&mut self, _control: &Self::Device) -> io::Result<u16> {
            self.push(Call::OpenQuery);
            Ok(self.state.borrow().free_vt)
        }

        fn open_vt(&mut self, path: &Path, number: u16) -> io::Result<Self::Device> {
            self.push(Call::OpenVt(path.display().to_string(), number));
            Ok(Device::Splash(number))
        }

        fn dimensions(&mut self, _vt: &Self::Device) -> io::Result<Dimensions> {
            self.push(Call::Dimensions);
            Ok(self.state.borrow().dimensions)
        }

        fn terminal_state(&mut self, _vt: &Self::Device) -> io::Result<Self::TerminalState> {
            self.push(Call::TerminalState);
            Ok(42)
        }

        fn set_raw_terminal(
            &mut self,
            _vt: &Self::Device,
            _original: &Self::TerminalState,
        ) -> io::Result<()> {
            self.push(Call::SetRaw);
            Ok(())
        }

        fn restore_terminal(
            &mut self,
            _vt: &Self::Device,
            _original: &Self::TerminalState,
        ) -> io::Result<()> {
            self.push(Call::RestoreTerminal);
            if self.state.borrow().fail_restore_terminal {
                Err(io::Error::other("injected termios restoration failure"))
            } else {
                Ok(())
            }
        }

        fn kd_mode(&mut self, _vt: &Self::Device) -> io::Result<i32> {
            self.push(Call::KdMode);
            Ok(self.state.borrow().kd_mode)
        }

        fn set_kd_mode(&mut self, _vt: &Self::Device, mode: i32) -> io::Result<()> {
            self.push(Call::SetKd(mode));
            Ok(())
        }

        fn keyboard_mode(&mut self, _vt: &Self::Device) -> io::Result<i32> {
            self.push(Call::KeyboardMode);
            Ok(self.state.borrow().keyboard_mode)
        }

        fn set_keyboard_mode(&mut self, _vt: &Self::Device, mode: i32) -> io::Result<()> {
            self.push(Call::SetKeyboard(mode));
            Ok(())
        }

        fn activate(&mut self, _control: &Self::Device, number: u16) -> io::Result<()> {
            self.push(Call::Activate(number));
            self.state.borrow_mut().active_vt = number;
            Ok(())
        }

        fn wait_active(
            &mut self,
            _control: &Self::Device,
            number: u16,
            timeout: Duration,
        ) -> io::Result<()> {
            self.push(Call::WaitActive(number, timeout));
            if self.state.borrow().active_vt == number {
                Ok(())
            } else {
                Err(io::Error::other("wrong active VT"))
            }
        }

        fn disallocate(&mut self, _control: &Self::Device, number: u16) -> io::Result<()> {
            self.push(Call::Disallocate(number));
            Ok(())
        }

        fn write_all(&mut self, _vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
            self.push(Call::Write(bytes.to_vec()));
            let is_frame = bytes.starts_with(b"\x1b[H\x1b[0m");
            if is_frame && self.state.borrow().fail_frame_write_once {
                self.state.borrow_mut().fail_frame_write_once = false;
                return Err(io::Error::other("injected render failure"));
            }
            Ok(())
        }

        fn write_sensitive(&mut self, _vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
            self.push(Call::WriteSensitive(bytes.len()));
            Ok(())
        }

        fn flush(&mut self, _vt: &mut Self::Device) -> io::Result<()> {
            self.push(Call::Flush);
            Ok(())
        }

        fn poll_read(
            &mut self,
            _vt: &mut Self::Device,
            timeout: Duration,
        ) -> io::Result<Option<Vec<u8>>> {
            self.push(Call::Poll(timeout));
            Ok(self.state.borrow_mut().input.pop_front())
        }
    }

    #[test]
    fn open_query_backend_captures_switches_and_restores_every_owned_state() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);

        display.acquire().unwrap();
        assert_eq!(display.state(), DisplayState::Hidden);
        assert_eq!(display.original_vt(), Some(1));
        assert_eq!(display.splash_vt(), Some(7));
        assert_eq!(display.dimensions(), Some(Dimensions::new(3, 2).unwrap()));
        assert!(
            !state
                .borrow()
                .calls
                .iter()
                .any(|call| matches!(call, Call::Activate(_)))
        );

        display.show().unwrap();
        let scene = Scene::from_rows(&["abc", "d e"]).unwrap();
        display.render(&scene).unwrap();
        state.borrow_mut().input.push_back(vec![0x1b]);
        assert_eq!(
            display.poll_input(Duration::from_millis(9)).unwrap(),
            Some(InputEvent::Bytes(vec![0x1b]))
        );
        display.details(true).unwrap();
        assert_eq!(display.state(), DisplayState::Details);
        state.borrow_mut().active_vt = 7;
        assert_eq!(
            display.poll_input(Duration::ZERO).unwrap(),
            Some(InputEvent::ReturnToSplash)
        );
        display.details(false).unwrap();
        display.hide().unwrap();
        display.restore().unwrap();
        let call_count = state.borrow().calls.len();
        display.restore().unwrap();
        assert_eq!(state.borrow().calls.len(), call_count);

        let calls = &state.borrow().calls;
        assert!(calls.contains(&Call::OpenControl("/dev/tty0".into())));
        assert!(calls.contains(&Call::OpenQuery));
        assert!(calls.contains(&Call::OpenVt("/dev/tty7".into(), 7)));
        assert!(calls.contains(&Call::SetRaw));
        assert!(calls.contains(&Call::SetKd(0)));
        assert!(calls.contains(&Call::SetKeyboard(3)));
        assert!(calls.contains(&Call::Activate(7)));
        assert!(calls.contains(&Call::WaitActive(7, Duration::from_millis(500))));
        assert!(calls.contains(&Call::Activate(1)));
        assert!(calls.contains(&Call::WaitActive(1, Duration::from_millis(500))));
        assert!(calls.contains(&Call::RestoreTerminal));
        assert!(calls.contains(&Call::SetKd(1)));
        assert!(calls.contains(&Call::SetKeyboard(1)));
        assert!(calls.contains(&Call::Disallocate(7)));
        assert!(calls.iter().any(|call| matches!(
            call,
            Call::Write(bytes) if bytes.windows(3).any(|window| window == b"abc")
        )));
    }

    #[test]
    fn sensitive_text_uses_non_retaining_io_path() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);
        display.acquire().unwrap();
        display.show().unwrap();

        display
            .render_sensitive_text(1, 0, "s3!", Style::default())
            .unwrap();

        let calls = &state.borrow().calls;
        assert!(calls.contains(&Call::WriteSensitive(3)));
        assert!(!format!("{calls:?}").contains("s3!"));
        assert!(!calls.iter().any(|call| matches!(
            call,
            Call::Write(bytes) if bytes.windows(3).any(|window| window == b"s3!")
        )));
    }

    #[test]
    fn configured_vt_does_not_query_or_disallocate_kernel_owned_vt() {
        let (io, state) = FakeIo::new();
        let config = TextVtConfig::configured(9).unwrap();
        let mut display = TextVtBackend::with_io(config, io);
        display.acquire().unwrap();
        assert_eq!(display.splash_vt(), Some(9));
        display.restore().unwrap();

        let calls = &state.borrow().calls;
        assert!(!calls.contains(&Call::OpenQuery));
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, Call::Disallocate(_)))
        );
        assert!(calls.contains(&Call::OpenVt("/dev/tty9".into(), 9)));
    }

    #[test]
    fn retain_pixels_still_restores_all_owned_console_state() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::configured(9).unwrap(), io);
        display.acquire().unwrap();
        display.show().unwrap();
        display
            .render(&Scene::from_rows(&["abc", "def"]).unwrap())
            .unwrap();
        display
            .restore_with_mode(RestoreMode::RetainPixels)
            .unwrap();

        let calls = &state.borrow().calls;
        assert!(!calls.contains(&Call::Write(b"\x1b[0m\x1b[2J\x1b[H".to_vec())));
        assert!(calls.contains(&Call::RestoreTerminal));
        assert!(calls.contains(&Call::SetKd(1)));
        assert!(calls.contains(&Call::SetKeyboard(1)));
        assert!(calls.contains(&Call::Activate(1)));
        assert!(calls.contains(&Call::WaitActive(1, Duration::from_millis(500))));
        assert_eq!(display.state(), DisplayState::Restored);
    }

    #[test]
    fn changed_kernel_dimensions_are_reported_before_input() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);
        display.acquire().unwrap();
        display.show().unwrap();
        let resized = Dimensions::new(5, 4).unwrap();
        state.borrow_mut().dimensions = resized;

        assert_eq!(
            display.poll_input(Duration::ZERO).unwrap(),
            Some(InputEvent::Resized(resized))
        );
        assert_eq!(display.dimensions(), Some(resized));
        display.restore().unwrap();
    }

    #[test]
    fn render_failure_restores_console_and_enters_failed_open_state() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);
        display.acquire().unwrap();
        display.show().unwrap();
        state.borrow_mut().fail_frame_write_once = true;

        let error = display.render(&Scene::from_rows(&["abc", "def"]).unwrap());
        assert!(matches!(error, Err(DisplayError::Backend { .. })));
        assert_eq!(display.state(), DisplayState::FailedOpen);
        assert_eq!(state.borrow().active_vt, 1);

        let calls = &state.borrow().calls;
        assert!(calls.contains(&Call::RestoreTerminal));
        assert!(calls.contains(&Call::SetKd(1)));
        assert!(calls.contains(&Call::SetKeyboard(1)));
        assert!(calls.contains(&Call::Activate(1)));
        assert!(calls.contains(&Call::WaitActive(1, Duration::from_millis(500))));
        assert!(calls.contains(&Call::Disallocate(7)));
    }

    #[test]
    fn cleanup_failure_is_not_hidden_by_the_triggering_render_error() {
        let (io, state) = FakeIo::new();
        let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);
        display.acquire().unwrap();
        display.show().unwrap();
        state.borrow_mut().fail_frame_write_once = true;
        state.borrow_mut().fail_restore_terminal = true;

        let error = display
            .render(&Scene::from_rows(&["abc", "def"]).unwrap())
            .expect_err("render and restoration must both fail");

        assert!(error.restoration_failed());
        assert!(
            error
                .to_string()
                .contains("display restoration also failed")
        );
        assert_eq!(display.state(), DisplayState::FailedOpen);
        // Cleanup remains best-effort: later restoration operations still run
        // even though restoring termios was the first cleanup failure.
        let calls = &state.borrow().calls;
        assert!(calls.contains(&Call::SetKd(1)));
        assert!(calls.contains(&Call::SetKeyboard(1)));
        assert!(calls.contains(&Call::Activate(1)));
        assert!(calls.contains(&Call::Disallocate(7)));
    }

    #[test]
    fn drop_restores_once_without_touching_real_devices() {
        let (io, state) = FakeIo::new();
        {
            let mut display = TextVtBackend::with_io(TextVtConfig::open_query(), io);
            display.acquire().unwrap();
            display.show().unwrap();
        }

        let calls = &state.borrow().calls;
        assert_eq!(
            calls
                .iter()
                .filter(|call| matches!(call, Call::RestoreTerminal))
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| matches!(call, Call::SetKeyboard(1)))
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| matches!(call, Call::Disallocate(7)))
                .count(),
            1
        );
        assert_eq!(state.borrow().active_vt, 1);
    }
}
