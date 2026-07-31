//! Linux virtual-terminal display backend.
//!
//! The real implementation is deliberately hidden behind [`VtIo`].  Unit
//! tests inject a fake implementation and never open `/dev` or issue ioctls.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    Color, Dimensions, DisplayBackend, DisplayError, DisplayState, InputEvent, RestoreMode, Scene,
    Style, validate_sensitive_text,
};
use crate::terminal::write_all_bounded;

const BACKEND_NAME: &str = "linux-text-vt";
const CONTROL_DEVICE: &str = "/dev/tty0";
const MIN_VT: u16 = 1;
const MAX_VT: u16 = 63;
const KD_TEXT: i32 = 0;
const K_UNICODE: i32 = 3;
const VT_SWITCH_TIMEOUT: Duration = Duration::from_millis(500);
const VT_SWITCH_POLL_INTERVAL: Duration = Duration::from_millis(5);
const VT_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const VT_RESTORE_WRITE_TIMEOUT: Duration = Duration::from_millis(100);

const SAVE_CURSOR: &[u8] = b"\x1b7";
const HIDE_CURSOR_CLEAR: &[u8] = b"\x1b[?25l\x1b[0m\x1b[2J\x1b[H";
const RESTORE_SCREEN_STATE: &[u8] = b"\x1b[0m\x1b8\x1b[?25h";
const CLEAR_SPLASH: &[u8] = b"\x1b[0m\x1b[2J\x1b[H";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtSelection {
    /// Ask the kernel for an unused VT with `VT_OPENQRY`.
    OpenQuery,
    /// Use a specifically configured, dedicated VT.
    Configured(u16),
}

/// Result of asking the kernel to discard an inactive VT's backing storage.
///
/// `VT_OPENQRY` only reports a VT that is unused at that instant; it does not
/// reserve the VT against later users.  During a long initramfs-to-real-root
/// handoff, a console manager can legitimately open the splash VT before
/// Bootart exits.  In that case Linux returns `EBUSY` from `VT_DISALLOCATE`
/// even after Bootart has closed its own descriptor and switched back to the
/// original VT.  That is successful release by Bootart, but not deallocation
/// of a VT that is now independently owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtDeallocation {
    Deallocated,
    InUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextVtConfig {
    selection: VtSelection,
}

impl TextVtConfig {
    pub const fn open_query() -> Self {
        Self {
            selection: VtSelection::OpenQuery,
        }
    }

    pub fn configured(number: u16) -> Result<Self, VtConfigError> {
        validate_vt_number(number)?;
        Ok(Self {
            selection: VtSelection::Configured(number),
        })
    }

    pub const fn selection(self) -> VtSelection {
        self.selection
    }
}

impl Default for TextVtConfig {
    fn default() -> Self {
        Self::open_query()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtConfigError {
    pub number: u16,
}

impl std::fmt::Display for VtConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Linux VT number must be in {MIN_VT}..={MAX_VT}, got {}",
            self.number
        )
    }
}

impl std::error::Error for VtConfigError {}

fn validate_vt_number(number: u16) -> Result<(), VtConfigError> {
    if (MIN_VT..=MAX_VT).contains(&number) {
        Ok(())
    } else {
        Err(VtConfigError { number })
    }
}

/// Injectable boundary around filesystem, termios, polling, and Linux VT
/// ioctls.  The operations are intentionally high-level so tests cannot make a
/// fake ioctl number accidentally look like supported kernel behavior.
pub trait VtIo {
    type Device;
    type TerminalState;

    fn open_control(&mut self, path: &Path) -> io::Result<Self::Device>;
    fn active_vt(&mut self, control: &Self::Device) -> io::Result<u16>;
    fn open_query(&mut self, control: &Self::Device) -> io::Result<u16>;
    fn open_vt(&mut self, path: &Path, number: u16) -> io::Result<Self::Device>;
    fn dimensions(&mut self, vt: &Self::Device) -> io::Result<Dimensions>;
    fn terminal_state(&mut self, vt: &Self::Device) -> io::Result<Self::TerminalState>;
    fn set_raw_terminal(
        &mut self,
        vt: &Self::Device,
        original: &Self::TerminalState,
    ) -> io::Result<()>;
    fn restore_terminal(
        &mut self,
        vt: &Self::Device,
        original: &Self::TerminalState,
    ) -> io::Result<()>;
    fn kd_mode(&mut self, vt: &Self::Device) -> io::Result<i32>;
    fn set_kd_mode(&mut self, vt: &Self::Device, mode: i32) -> io::Result<()>;
    fn keyboard_mode(&mut self, vt: &Self::Device) -> io::Result<i32>;
    fn set_keyboard_mode(&mut self, vt: &Self::Device, mode: i32) -> io::Result<()>;
    fn activate(&mut self, control: &Self::Device, number: u16) -> io::Result<()>;
    fn wait_active(
        &mut self,
        control: &Self::Device,
        number: u16,
        timeout: Duration,
    ) -> io::Result<()>;
    fn disallocate(&mut self, control: &Self::Device, number: u16) -> io::Result<VtDeallocation>;
    fn write_all(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()>;
    /// Bounded best-effort restoration output. The real implementation must
    /// continue attempting this write after a shutdown signal instead of
    /// treating the already-set stop flag as a reason to skip restoration.
    fn write_restore(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
        self.write_all(vt, bytes)
    }
    /// Write secret bytes directly. Implementations must not copy, log, or
    /// retain `bytes` after this call.
    fn write_sensitive(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()>;
    fn flush(&mut self, vt: &mut Self::Device) -> io::Result<()>;
    fn poll_read(
        &mut self,
        vt: &mut Self::Device,
        timeout: Duration,
    ) -> io::Result<Option<Vec<u8>>>;
}

/// Concrete system boundary.  Constructing it performs no I/O; `/dev/tty0`
/// is opened only when [`DisplayBackend::acquire`] is called.
#[derive(Debug, Default)]
pub struct LinuxVtIo;

/// Exact `termios` snapshot returned by `tcgetattr`.
pub struct LinuxTerminalState(libc::termios);

#[repr(C)]
#[derive(Default)]
struct VtStat {
    active: u16,
    signal: u16,
    state: u16,
}

// libc intentionally models ioctl's request type differently for glibc
// (`c_ulong`) and musl (`c_int`).  Use that target-specific ABI type rather
// than baking the host libc's representation into the static build.
const VT_OPENQRY: libc::Ioctl = 0x5600;
const VT_GETSTATE: libc::Ioctl = 0x5603;
const VT_ACTIVATE: libc::Ioctl = 0x5606;
const VT_DISALLOCATE: libc::Ioctl = 0x5608;
const KDSETMODE: libc::Ioctl = 0x4B3A;
const KDGETMODE: libc::Ioctl = 0x4B3B;
const KDGKBMODE: libc::Ioctl = 0x4B44;
const KDSKBMODE: libc::Ioctl = 0x4B45;

fn ioctl_failed(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl VtIo for LinuxVtIo {
    type Device = File;
    type TerminalState = LinuxTerminalState;

    fn open_control(&mut self, path: &Path) -> io::Result<Self::Device> {
        open_device(path)
    }

    fn active_vt(&mut self, control: &Self::Device) -> io::Result<u16> {
        let mut state = VtStat::default();
        // SAFETY: `state` has the Linux UAPI layout and remains valid for the
        // duration of the ioctl.  `control` owns a live descriptor.
        let result = unsafe { libc::ioctl(control.as_raw_fd(), VT_GETSTATE, &mut state) };
        ioctl_failed(result)?;
        validate_kernel_vt(state.active)?;
        Ok(state.active)
    }

    fn open_query(&mut self, control: &Self::Device) -> io::Result<u16> {
        let mut number: libc::c_int = -1;
        // SAFETY: `number` is the output integer required by VT_OPENQRY.
        let result = unsafe { libc::ioctl(control.as_raw_fd(), VT_OPENQRY, &mut number) };
        ioctl_failed(result)?;
        let number = u16::try_from(number)
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "no unused Linux VT"))?;
        validate_kernel_vt(number)?;
        Ok(number)
    }

    fn open_vt(&mut self, path: &Path, _number: u16) -> io::Result<Self::Device> {
        open_device(path)
    }

    fn dimensions(&mut self, vt: &Self::Device) -> io::Result<Dimensions> {
        // SAFETY: `winsize` is initialized and passed by writable pointer.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGWINSZ writes a `winsize` to the provided pointer.
        let result = unsafe { libc::ioctl(vt.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
        ioctl_failed(result)?;
        Dimensions::new(size.ws_col, size.ws_row)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn terminal_state(&mut self, vt: &Self::Device) -> io::Result<Self::TerminalState> {
        // SAFETY: `tcgetattr` initializes the supplied termios value.
        let mut state: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `state` is valid writable memory and the descriptor is owned.
        let result = unsafe { libc::tcgetattr(vt.as_raw_fd(), &mut state) };
        ioctl_failed(result)?;
        Ok(LinuxTerminalState(state))
    }

    fn set_raw_terminal(
        &mut self,
        vt: &Self::Device,
        original: &Self::TerminalState,
    ) -> io::Result<()> {
        let mut raw = original.0;
        // SAFETY: `raw` is an initialized termios snapshot.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: `raw` remains valid for this call.
        let result = unsafe { libc::tcsetattr(vt.as_raw_fd(), libc::TCSANOW, &raw) };
        ioctl_failed(result)
    }

    fn restore_terminal(
        &mut self,
        vt: &Self::Device,
        original: &Self::TerminalState,
    ) -> io::Result<()> {
        // SAFETY: `original` contains the exact initialized tcgetattr snapshot.
        let result = unsafe { libc::tcsetattr(vt.as_raw_fd(), libc::TCSANOW, &original.0) };
        ioctl_failed(result)
    }

    fn kd_mode(&mut self, vt: &Self::Device) -> io::Result<i32> {
        let mut mode: libc::c_int = -1;
        // SAFETY: KDGETMODE writes one integer to `mode`.
        let result = unsafe { libc::ioctl(vt.as_raw_fd(), KDGETMODE, &mut mode) };
        ioctl_failed(result)?;
        Ok(mode)
    }

    fn set_kd_mode(&mut self, vt: &Self::Device, mode: i32) -> io::Result<()> {
        // SAFETY: KDSETMODE consumes its third argument by value.
        let result = unsafe { libc::ioctl(vt.as_raw_fd(), KDSETMODE, mode) };
        ioctl_failed(result)
    }

    fn keyboard_mode(&mut self, vt: &Self::Device) -> io::Result<i32> {
        let mut mode: libc::c_int = -1;
        // SAFETY: KDGKBMODE writes one integer to `mode`.
        let result = unsafe { libc::ioctl(vt.as_raw_fd(), KDGKBMODE, &mut mode) };
        ioctl_failed(result)?;
        Ok(mode)
    }

    fn set_keyboard_mode(&mut self, vt: &Self::Device, mode: i32) -> io::Result<()> {
        // SAFETY: KDSKBMODE consumes its third argument by value.
        let result = unsafe { libc::ioctl(vt.as_raw_fd(), KDSKBMODE, mode) };
        ioctl_failed(result)
    }

    fn activate(&mut self, control: &Self::Device, number: u16) -> io::Result<()> {
        // SAFETY: VT_ACTIVATE consumes a validated VT number by value.
        let result =
            unsafe { libc::ioctl(control.as_raw_fd(), VT_ACTIVATE, number as libc::c_int) };
        ioctl_failed(result)
    }

    fn wait_active(
        &mut self,
        control: &Self::Device,
        number: u16,
        timeout: Duration,
    ) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.active_vt(control)? == number {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Linux VT {number} did not activate within {timeout:?}"),
                ));
            }
            std::thread::sleep(VT_SWITCH_POLL_INTERVAL.min(deadline - now));
        }
    }

    fn disallocate(&mut self, control: &Self::Device, number: u16) -> io::Result<VtDeallocation> {
        // SAFETY: VT_DISALLOCATE consumes a validated, inactive VT number.
        let result =
            unsafe { libc::ioctl(control.as_raw_fd(), VT_DISALLOCATE, number as libc::c_int) };
        if result != -1 {
            return Ok(VtDeallocation::Deallocated);
        }

        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EBUSY) {
            // Linux rejects deallocation when the VT is open elsewhere,
            // foreground, or owns a console selection. The backend closes
            // its splash descriptor before this call and separately records
            // any failure to reactivate the original VT, so EBUSY here means
            // another kernel/user-space owner has legitimately claimed it.
            Ok(VtDeallocation::InUse)
        } else {
            Err(error)
        }
    }

    fn write_all(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
        write_all_bounded(vt.as_raw_fd(), bytes, VT_WRITE_TIMEOUT, true)
    }

    fn write_restore(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
        write_all_bounded(vt.as_raw_fd(), bytes, VT_RESTORE_WRITE_TIMEOUT, false)
    }

    fn write_sensitive(&mut self, vt: &mut Self::Device, bytes: &[u8]) -> io::Result<()> {
        write_all_bounded(vt.as_raw_fd(), bytes, VT_WRITE_TIMEOUT, true)
    }

    fn flush(&mut self, vt: &mut Self::Device) -> io::Result<()> {
        vt.flush()
    }

    fn poll_read(
        &mut self,
        vt: &mut Self::Device,
        timeout: Duration,
    ) -> io::Result<Option<Vec<u8>>> {
        let mut descriptor = libc::pollfd {
            fd: vt.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        // SAFETY: `descriptor` is one initialized pollfd and remains live.
        let ready = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(error);
        }
        if ready == 0 {
            return Ok(None);
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "splash VT descriptor is invalid",
            ));
        }
        if descriptor.revents & libc::POLLERR != 0 {
            return Err(io::Error::other("splash VT reported a polling error"));
        }
        if descriptor.revents & libc::POLLIN == 0 {
            return Ok(None);
        }

        let mut bytes = vec![0_u8; 256];
        match vt.read(&mut bytes) {
            Ok(0) => Ok(None),
            Ok(count) => {
                bytes.truncate(count);
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn open_device(path: &Path) -> io::Result<File> {
    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    if !device.metadata()?.file_type().is_char_device() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Linux VT path is not a character device: {}",
                path.display()
            ),
        ));
    }
    Ok(device)
}

fn validate_kernel_vt(number: u16) -> io::Result<()> {
    validate_vt_number(number).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub struct TextVtBackend<I: VtIo = LinuxVtIo> {
    config: TextVtConfig,
    io: I,
    state: DisplayState,
    control: Option<I::Device>,
    splash: Option<I::Device>,
    original_vt: Option<u16>,
    splash_vt: Option<u16>,
    dimensions: Option<Dimensions>,
    terminal_state: Option<I::TerminalState>,
    original_kd_mode: Option<i32>,
    original_keyboard_mode: Option<i32>,
    terminal_changed: bool,
    kd_changed: bool,
    keyboard_changed: bool,
    cursor_saved: bool,
    cursor_hidden: bool,
    allocated_vt: bool,
}

impl TextVtBackend<LinuxVtIo> {
    pub fn new(config: TextVtConfig) -> Self {
        Self::with_io(config, LinuxVtIo)
    }
}

impl<I: VtIo> TextVtBackend<I> {
    pub fn with_io(config: TextVtConfig, io: I) -> Self {
        Self {
            config,
            io,
            state: DisplayState::Unacquired,
            control: None,
            splash: None,
            original_vt: None,
            splash_vt: None,
            dimensions: None,
            terminal_state: None,
            original_kd_mode: None,
            original_keyboard_mode: None,
            terminal_changed: false,
            kd_changed: false,
            keyboard_changed: false,
            cursor_saved: false,
            cursor_hidden: false,
            allocated_vt: false,
        }
    }

    pub fn original_vt(&self) -> Option<u16> {
        self.original_vt
    }

    pub fn splash_vt(&self) -> Option<u16> {
        self.splash_vt
    }

    fn invalid_state(&self, operation: &'static str) -> DisplayError {
        DisplayError::InvalidState {
            operation,
            state: self.state,
        }
    }

    fn fail_open(&mut self, operation: &'static str, source: io::Error) -> DisplayError {
        let primary = DisplayError::backend(BACKEND_NAME, operation, source);
        let restoration = self.cleanup(DisplayState::FailedOpen, RestoreMode::Clear);
        DisplayError::with_restoration(primary, restoration)
    }

    fn switch_to(&mut self, number: u16, operation: &'static str) -> Result<(), DisplayError> {
        let activate_result = {
            let control = self
                .control
                .as_ref()
                .ok_or_else(|| self.invalid_state(operation))?;
            self.io.activate(control, number)
        };
        if let Err(error) = activate_result {
            return Err(self.fail_open(operation, error));
        }

        let wait_result = {
            let control = self
                .control
                .as_ref()
                .ok_or_else(|| self.invalid_state(operation))?;
            self.io.wait_active(control, number, VT_SWITCH_TIMEOUT)
        };
        if let Err(error) = wait_result {
            return Err(self.fail_open(operation, error));
        }
        Ok(())
    }

    fn write_splash(&mut self, bytes: &[u8], operation: &'static str) -> Result<(), DisplayError> {
        let result = match self.splash.as_mut() {
            Some(splash) => self.io.write_all(splash, bytes),
            None => return Err(self.invalid_state(operation)),
        };
        if let Err(error) = result {
            return Err(self.fail_open(operation, error));
        }
        let result = match self.splash.as_mut() {
            Some(splash) => self.io.flush(splash),
            None => return Err(self.invalid_state(operation)),
        };
        result.map_err(|error| self.fail_open(operation, error))
    }

    fn write_sensitive_splash(
        &mut self,
        bytes: &[u8],
        operation: &'static str,
    ) -> Result<(), DisplayError> {
        let result = match self.splash.as_mut() {
            Some(splash) => self.io.write_sensitive(splash, bytes),
            None => return Err(self.invalid_state(operation)),
        };
        result.map_err(|error| self.fail_open(operation, error))
    }

    fn cleanup(
        &mut self,
        final_state: DisplayState,
        restore_mode: RestoreMode,
    ) -> Result<(), DisplayError> {
        let mut first_error: Option<DisplayError> = None;

        if let Some(splash) = self.splash.as_mut() {
            if restore_mode == RestoreMode::Clear
                && let Err(error) = self.io.write_restore(splash, CLEAR_SPLASH)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "clear splash pixels", error),
                );
            }
            if self.cursor_saved || self.cursor_hidden {
                if let Err(error) = self.io.write_restore(splash, RESTORE_SCREEN_STATE) {
                    record_error(
                        &mut first_error,
                        DisplayError::backend(BACKEND_NAME, "restore cursor", error),
                    );
                } else if let Err(error) = self.io.flush(splash) {
                    record_error(
                        &mut first_error,
                        DisplayError::backend(BACKEND_NAME, "flush cursor restoration", error),
                    );
                }
            }
            if self.terminal_changed
                && let Some(original) = self.terminal_state.as_ref()
                && let Err(error) = self.io.restore_terminal(splash, original)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "restore termios", error),
                );
            }
            if self.keyboard_changed
                && let Some(mode) = self.original_keyboard_mode
                && let Err(error) = self.io.set_keyboard_mode(splash, mode)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "restore keyboard mode", error),
                );
            }
            if self.kd_changed
                && let Some(mode) = self.original_kd_mode
                && let Err(error) = self.io.set_kd_mode(splash, mode)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "restore KD mode", error),
                );
            }
        }

        // Close the splash descriptor before asking the kernel to deallocate a
        // VT obtained through VT_OPENQRY.
        self.splash.take();

        if let (Some(control), Some(original_vt)) = (self.control.as_ref(), self.original_vt) {
            if let Err(error) = self.io.activate(control, original_vt) {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "reactivate original VT", error),
                );
            } else if let Err(error) = self.io.wait_active(control, original_vt, VT_SWITCH_TIMEOUT)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "wait for original VT", error),
                );
            }

            if self.allocated_vt
                && let Some(splash_vt) = self.splash_vt
                && let Err(error) = self.io.disallocate(control, splash_vt)
            {
                record_error(
                    &mut first_error,
                    DisplayError::backend(BACKEND_NAME, "deallocate splash VT", error),
                );
            }
        }

        self.control.take();
        self.original_vt = None;
        self.splash_vt = None;
        self.dimensions = None;
        self.terminal_state = None;
        self.original_kd_mode = None;
        self.original_keyboard_mode = None;
        self.terminal_changed = false;
        self.kd_changed = false;
        self.keyboard_changed = false;
        self.cursor_saved = false;
        self.cursor_hidden = false;
        self.allocated_vt = false;
        self.state = final_state;

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl<I: VtIo> DisplayBackend for TextVtBackend<I> {
    fn state(&self) -> DisplayState {
        self.state
    }

    fn dimensions(&self) -> Option<Dimensions> {
        self.dimensions
    }

    fn acquire(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Hidden | DisplayState::Splash | DisplayState::Details => return Ok(()),
            DisplayState::Unacquired => {}
            _ => return Err(self.invalid_state("acquire")),
        }
        self.state = DisplayState::Acquiring;

        let result = (|| -> io::Result<()> {
            self.control = Some(self.io.open_control(Path::new(CONTROL_DEVICE))?);
            let control = self.control.as_ref().expect("control was just stored");
            let original_vt = self.io.active_vt(control)?;
            self.original_vt = Some(original_vt);

            let splash_vt = match self.config.selection {
                VtSelection::OpenQuery => {
                    self.allocated_vt = true;
                    self.io.open_query(control)?
                }
                VtSelection::Configured(number) => number,
            };
            validate_kernel_vt(splash_vt)?;
            if splash_vt == original_vt {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "splash VT must differ from the active boot VT",
                ));
            }
            self.splash_vt = Some(splash_vt);

            let path = PathBuf::from(format!("/dev/tty{splash_vt}"));
            self.splash = Some(self.io.open_vt(&path, splash_vt)?);
            let splash = self.splash.as_mut().expect("splash VT was just stored");
            self.dimensions = Some(self.io.dimensions(splash)?);
            self.terminal_state = Some(self.io.terminal_state(splash)?);
            self.original_kd_mode = Some(self.io.kd_mode(splash)?);
            self.original_keyboard_mode = Some(self.io.keyboard_mode(splash)?);

            // Linux exposes termios and KD state through ioctls, but it has no
            // truthful "get cursor visibility" ioctl.  Save the cursor
            // position in the console itself, track every visibility change
            // we make, and restore to a usable visible cursor.  Do not claim
            // that an already-hidden cursor on a configured VT was queried.
            self.io.write_all(splash, SAVE_CURSOR)?;
            self.io.flush(splash)?;
            self.cursor_saved = true;

            let terminal_state = self
                .terminal_state
                .as_ref()
                .expect("terminal state was just captured");
            self.io.set_raw_terminal(splash, terminal_state)?;
            self.terminal_changed = true;

            if self.original_kd_mode != Some(KD_TEXT) {
                self.io.set_kd_mode(splash, KD_TEXT)?;
                self.kd_changed = true;
            }
            if self.original_keyboard_mode != Some(K_UNICODE) {
                self.io.set_keyboard_mode(splash, K_UNICODE)?;
                self.keyboard_changed = true;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.state = DisplayState::Hidden;
                Ok(())
            }
            Err(error) => {
                let primary = DisplayError::backend(BACKEND_NAME, "acquire display", error);
                let restoration = self.cleanup(DisplayState::FailedOpen, RestoreMode::Clear);
                Err(DisplayError::with_restoration(primary, restoration))
            }
        }
    }

    fn show(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Splash => return Ok(()),
            DisplayState::Hidden | DisplayState::Details => {}
            _ => return Err(self.invalid_state("show")),
        }
        let splash_vt = self.splash_vt.ok_or_else(|| self.invalid_state("show"))?;
        self.switch_to(splash_vt, "activate splash VT")?;
        if !self.cursor_hidden {
            self.write_splash(HIDE_CURSOR_CLEAR, "prepare splash VT")?;
            self.cursor_hidden = true;
        }
        self.state = DisplayState::Splash;
        Ok(())
    }

    fn hide(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Hidden => return Ok(()),
            DisplayState::Details => {
                self.state = DisplayState::Hidden;
                return Ok(());
            }
            DisplayState::Splash => {}
            _ => return Err(self.invalid_state("hide")),
        }
        let original_vt = self.original_vt.ok_or_else(|| self.invalid_state("hide"))?;
        self.switch_to(original_vt, "activate original VT")?;
        self.state = DisplayState::Hidden;
        Ok(())
    }

    fn render(&mut self, scene: &Scene) -> Result<(), DisplayError> {
        if self.state != DisplayState::Splash {
            return Err(self.invalid_state("render"));
        }
        let dimensions = self
            .dimensions
            .ok_or_else(|| self.invalid_state("render"))?;
        if scene.dimensions() != dimensions {
            return Err(DisplayError::SizeMismatch {
                expected: dimensions,
                actual: scene.dimensions(),
            });
        }
        let bytes = encode_scene(scene);
        self.write_splash(&bytes, "render frame")
    }

    fn render_sensitive_text(
        &mut self,
        row: u16,
        column: u16,
        text: &str,
        style: Style,
    ) -> Result<(), DisplayError> {
        if self.state != DisplayState::Splash {
            return Err(self.invalid_state("render sensitive text"));
        }
        let dimensions = self
            .dimensions
            .ok_or_else(|| self.invalid_state("render sensitive text"))?;
        validate_sensitive_text(dimensions, row, column, text)?;

        // Cursor/style bytes contain no plaintext and may use the ordinary
        // logged/testable path. The borrowed secret slice itself crosses only
        // the dedicated non-retaining VtIo call.
        let mut prefix = Vec::with_capacity(40);
        write!(&mut prefix, "\x1b[{};{}H", row + 1, column + 1)
            .expect("writing to Vec cannot fail");
        write_style(&mut prefix, style);
        self.write_splash(&prefix, "position sensitive text")?;
        self.write_sensitive_splash(text.as_bytes(), "render sensitive text")?;
        self.write_splash(b"\x1b[0m", "finish sensitive text")
    }

    fn poll_input(&mut self, timeout: Duration) -> Result<Option<InputEvent>, DisplayError> {
        match self.state {
            DisplayState::Hidden => return Ok(None),
            DisplayState::Details => {
                // Never read the original boot console: doing so would race a
                // getty or the distro's stock password agent. VT_GETSTATE on
                // /dev/tty0 is observation-only. A deliberate kernel VT
                // switch back to our reserved VT becomes a non-byte event.
                let active = {
                    let control = self
                        .control
                        .as_ref()
                        .ok_or_else(|| self.invalid_state("observe active details VT"))?;
                    self.io.active_vt(control)
                };
                return match active {
                    Ok(number) if Some(number) == self.splash_vt => {
                        Ok(Some(InputEvent::ReturnToSplash))
                    }
                    Ok(_) => Ok(None),
                    Err(error) => Err(self.fail_open("observe active details VT", error)),
                };
            }
            DisplayState::Splash => {}
            _ => return Err(self.invalid_state("poll input")),
        }

        let measured_dimensions = match self.splash.as_ref() {
            Some(splash) => self.io.dimensions(splash),
            None => return Err(self.invalid_state("poll input")),
        };
        match measured_dimensions {
            Ok(dimensions) if self.dimensions != Some(dimensions) => {
                self.dimensions = Some(dimensions);
                return Ok(Some(InputEvent::Resized(dimensions)));
            }
            Ok(_) => {}
            Err(error) => return Err(self.fail_open("query splash dimensions", error)),
        }

        let result = match self.splash.as_mut() {
            Some(splash) => self.io.poll_read(splash, timeout),
            None => return Err(self.invalid_state("poll input")),
        };
        match result {
            Ok(Some(bytes)) if !bytes.is_empty() => Ok(Some(InputEvent::Bytes(bytes))),
            Ok(_) => Ok(None),
            Err(error) => Err(self.fail_open("poll splash input", error)),
        }
    }

    fn details(&mut self, visible: bool) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Hidden | DisplayState::Splash | DisplayState::Details => {}
            _ => return Err(self.invalid_state("change details visibility")),
        }
        if visible {
            if self.state == DisplayState::Details {
                return Ok(());
            }
            if self.state == DisplayState::Splash {
                let original = self
                    .original_vt
                    .ok_or_else(|| self.invalid_state("show details"))?;
                self.switch_to(original, "activate details VT")?;
            }
            self.state = DisplayState::Details;
        } else {
            if self.state == DisplayState::Splash {
                return Ok(());
            }
            let splash = self
                .splash_vt
                .ok_or_else(|| self.invalid_state("hide details"))?;
            self.switch_to(splash, "reactivate splash VT")?;
            if !self.cursor_hidden {
                self.write_splash(HIDE_CURSOR_CLEAR, "prepare splash VT")?;
                self.cursor_hidden = true;
            }
            self.state = DisplayState::Splash;
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<(), DisplayError> {
        self.restore_with_mode(RestoreMode::Clear)
    }

    fn restore_with_mode(&mut self, mode: RestoreMode) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Restored | DisplayState::FailedOpen => Ok(()),
            DisplayState::Unacquired => {
                self.state = DisplayState::Restored;
                Ok(())
            }
            _ => self.cleanup(DisplayState::Restored, mode),
        }
    }
}

impl<I: VtIo> Drop for TextVtBackend<I> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn record_error(slot: &mut Option<DisplayError>, error: DisplayError) {
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn encode_scene(scene: &Scene) -> Vec<u8> {
    let dimensions = scene.dimensions();
    let mut output = Vec::with_capacity(scene.cells().len().saturating_mul(4) + 64);
    output.extend_from_slice(b"\x1b[H\x1b[0m");
    let mut active_style = Style::default();

    for row in 0..dimensions.rows() {
        write!(&mut output, "\x1b[{};1H", row + 1).expect("writing to Vec cannot fail");
        for column in 0..dimensions.columns() {
            let cell = scene
                .get(column, row)
                .expect("scene dimensions and storage agree");
            if cell.style() != active_style {
                write_style(&mut output, cell.style());
                active_style = cell.style();
            }
            let mut bytes = [0_u8; 4];
            output.extend_from_slice(cell.glyph().encode_utf8(&mut bytes).as_bytes());
        }
    }
    output.extend_from_slice(b"\x1b[0m");
    output
}

fn write_style(output: &mut Vec<u8>, style: Style) {
    output.extend_from_slice(b"\x1b[0");
    if style.bold {
        output.extend_from_slice(b";1");
    }
    write!(
        output,
        ";{};{}m",
        foreground_code(style.foreground),
        background_code(style.background)
    )
    .expect("writing to Vec cannot fail");
}

fn foreground_code(color: Color) -> u8 {
    match color {
        Color::Default => 39,
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::White => 37,
        Color::BrightBlack => 90,
        Color::BrightRed => 91,
        Color::BrightGreen => 92,
        Color::BrightYellow => 93,
        Color::BrightBlue => 94,
        Color::BrightMagenta => 95,
        Color::BrightCyan => 96,
        Color::BrightWhite => 97,
    }
}

fn background_code(color: Color) -> u8 {
    match color {
        Color::Default => 49,
        Color::Black => 40,
        Color::Red => 41,
        Color::Green => 42,
        Color::Yellow => 43,
        Color::Blue => 44,
        Color::Magenta => 45,
        Color::Cyan => 46,
        Color::White => 47,
        Color::BrightBlack => 100,
        Color::BrightRed => 101,
        Color::BrightGreen => 102,
        Color::BrightYellow => 103,
        Color::BrightBlue => 104,
        Color::BrightMagenta => 105,
        Color::BrightCyan => 106,
        Color::BrightWhite => 107,
    }
}
