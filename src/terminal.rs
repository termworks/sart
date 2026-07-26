use crate::signals;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
const RESET_AND_SHOW_CURSOR: &[u8] = b"\x1b[0m\x1b[?25h";
const FRAME_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const RESTORE_WRITE_TIMEOUT: Duration = Duration::from_millis(100);
const POLL_SLICE: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: usize,
    pub height: usize,
}

impl TerminalSize {
    pub const DEFAULT: Self = Self {
        width: 80,
        height: 24,
    };
}

pub trait TerminalOutput {
    fn dimensions(&self) -> io::Result<TerminalSize>;
    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
}

pub struct StdoutTerminal {
    output: Option<File>,
    override_size: Option<TerminalSize>,
    needs_restore: bool,
}

impl StdoutTerminal {
    pub fn new() -> Self {
        Self {
            output: None,
            override_size: None,
            needs_restore: false,
        }
    }

    pub fn with_override(cols: Option<usize>, rows: Option<usize>) -> Self {
        let override_size = match (cols, rows) {
            (Some(c), Some(r)) if c > 0 && r > 0 => Some(TerminalSize {
                width: c,
                height: r,
            }),
            _ => None,
        };
        Self {
            output: None,
            override_size,
            needs_restore: false,
        }
    }

    fn output_fd(&mut self) -> io::Result<RawFd> {
        if self.output.is_none() {
            self.output = Some(open_nonblocking_stdout()?);
        }
        Ok(self
            .output
            .as_ref()
            .expect("output was initialized above")
            .as_raw_fd())
    }

    fn write_bounded(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
        stop_aware: bool,
    ) -> io::Result<()> {
        let fd = self.output_fd()?;
        write_all_bounded(fd, bytes, timeout, stop_aware)
    }

    fn restore_terminal(&mut self) -> io::Result<()> {
        let result = self.write_bounded(RESET_AND_SHOW_CURSOR, RESTORE_WRITE_TIMEOUT, false);
        if result.is_ok() {
            self.needs_restore = false;
        }
        result
    }
}

impl Drop for StdoutTerminal {
    fn drop(&mut self) {
        if self.needs_restore {
            let _ = self.restore_terminal();
        }
    }
}

impl Default for StdoutTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalOutput for StdoutTerminal {
    fn dimensions(&self) -> io::Result<TerminalSize> {
        if let Some(size) = self.override_size {
            return Ok(size);
        }

        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            let res = libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws);
            if res == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                return Ok(TerminalSize {
                    width: ws.ws_col as usize,
                    height: ws.ws_row as usize,
                });
            }
        }

        Ok(TerminalSize::DEFAULT)
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        let contains_hide = bytes
            .windows(HIDE_CURSOR.len())
            .any(|window| window == HIDE_CURSOR);
        let final_cursor_state = cursor_state_after(bytes);
        if contains_hide {
            self.needs_restore = true;
        }

        let is_restoration = matches!(final_cursor_state, Some(false));
        let timeout = if is_restoration {
            RESTORE_WRITE_TIMEOUT
        } else {
            FRAME_WRITE_TIMEOUT
        };
        let result = self.write_bounded(bytes, timeout, !is_restoration);

        match result {
            Ok(()) => {
                if let Some(needs_restore) = final_cursor_state {
                    self.needs_restore = needs_restore;
                }
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted && signals::should_stop() => {
                if self.needs_restore {
                    self.restore_terminal()
                } else {
                    Ok(())
                }
            }
            Err(error) => {
                if self.needs_restore {
                    let _ = self.restore_terminal();
                }
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // StdoutTerminal writes directly to an unbuffered descriptor.
        Ok(())
    }
}

/// Reopen stdout through procfs so `O_NONBLOCK` belongs to a new open-file
/// description. Mutating descriptor 1 in place would also change the parent
/// shell or supervisor's inherited open-file description and could leave it
/// nonblocking if bootart were killed.
fn open_nonblocking_stdout() -> io::Result<File> {
    let status_flags = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_GETFL) };
    if status_flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if status_flags & libc::O_ACCMODE == libc::O_RDONLY {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "stdout is not writable",
        ));
    }

    let mut options = OpenOptions::new();
    options.write(true).custom_flags(
        libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY | (status_flags & libc::O_APPEND),
    );
    let output = options.open(format!("/proc/self/fd/{}", libc::STDOUT_FILENO))?;

    // Preserve the current position for redirected regular files. Terminals,
    // pipes, and sockets report ESPIPE and need no position transfer.
    let offset = unsafe { libc::lseek(libc::STDOUT_FILENO, 0, libc::SEEK_CUR) };
    if offset >= 0 && unsafe { libc::lseek(output.as_raw_fd(), offset, libc::SEEK_SET) } == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(output)
}

fn cursor_state_after(bytes: &[u8]) -> Option<bool> {
    let mut needs_restore = None;
    for window in bytes.windows(HIDE_CURSOR.len()) {
        if window == HIDE_CURSOR {
            needs_restore = Some(true);
        } else if window == SHOW_CURSOR {
            needs_restore = Some(false);
        }
    }
    needs_restore
}

pub(crate) fn write_all_bounded(
    fd: RawFd,
    mut bytes: &[u8],
    timeout: Duration,
    stop_aware: bool,
) -> io::Result<()> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "terminal deadline overflow"))?;

    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal write deadline expired",
            ));
        }
        if stop_aware && signals::should_stop() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "terminal write interrupted by shutdown signal",
            ));
        }

        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
            continue;
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "terminal write returned zero",
            ));
        }

        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if !matches!(
            error.raw_os_error(),
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
        ) {
            return Err(error);
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal write deadline expired",
            ));
        }
        let remaining = deadline - now;
        let wait = remaining.min(POLL_SLICE).as_millis().max(1) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, wait) };
        if ready == -1 {
            let poll_error = io::Error::last_os_error();
            if poll_error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(poll_error);
        }
        if ready > 0 && descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        if ready > 0
            && descriptor.revents & (libc::POLLERR | libc::POLLHUP) != 0
            && descriptor.revents & libc::POLLOUT == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output closed while waiting to write",
            ));
        }
    }

    Ok(())
}

pub struct BufferTerminal {
    pub buffer: Vec<u8>,
    pub size: TerminalSize,
}

impl BufferTerminal {
    pub fn new(size: TerminalSize) -> Self {
        Self {
            buffer: Vec::new(),
            size,
        }
    }

    pub fn contents_as_string(&self) -> String {
        String::from_utf8_lossy(&self.buffer).to_string()
    }
}

impl TerminalOutput for BufferTerminal {
    fn dimensions(&self) -> io::Result<TerminalSize> {
        Ok(self.size)
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_terminal() {
        let size = TerminalSize {
            width: 40,
            height: 10,
        };
        let mut term = BufferTerminal::new(size);
        assert_eq!(term.dimensions().unwrap(), size);

        term.write_frame(b"hello").unwrap();
        term.flush().unwrap();
        assert_eq!(term.contents_as_string(), "hello");
    }

    #[test]
    fn test_stdout_terminal_override() {
        let term = StdoutTerminal::with_override(Some(120), Some(40));
        assert_eq!(
            term.dimensions().unwrap(),
            TerminalSize {
                width: 120,
                height: 40
            }
        );
    }
}
