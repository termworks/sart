use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
const RESET_ATTRIBUTES: &[u8] = b"\x1b[0m";
const MAX_PTY_OUTPUT_BYTES: usize = 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_TIMEOUT: Duration = Duration::from_secs(3);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const DRAIN_RETRY: Duration = Duration::from_millis(5);

struct Pty {
    master: File,
    stdout: File,
    stderr: File,
}

fn open_pty() -> Pty {
    // SAFETY: openpty initializes both descriptors and does not retain the
    // stack-owned winsize. Successful descriptors are wrapped before any
    // subsequent fallible operation; the failure path closes partial results.
    unsafe {
        let mut master_fd: libc::c_int = -1;
        let mut slave_fd: libc::c_int = -1;
        let mut size: libc::winsize = std::mem::zeroed();
        size.ws_col = 80;
        size.ws_row = 24;
        if libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &size,
        ) != 0
        {
            let error = io::Error::last_os_error();
            if master_fd >= 0 {
                libc::close(master_fd);
            }
            if slave_fd >= 0 && slave_fd != master_fd {
                libc::close(slave_fd);
            }
            panic!("openpty failed: {error}");
        }

        let master = File::from_raw_fd(master_fd);
        let stdout = File::from_raw_fd(slave_fd);
        set_close_on_exec(&master).expect("set PTY master close-on-exec");
        set_close_on_exec(&stdout).expect("set PTY slave close-on-exec");
        set_nonblocking(&master).expect("set PTY master nonblocking");
        let stderr = stdout.try_clone().expect("clone PTY slave");
        set_close_on_exec(&stderr).expect("set cloned PTY slave close-on-exec");

        Pty {
            master,
            stdout,
            stderr,
        }
    }
}

fn set_close_on_exec(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn send_signal(&mut self, signal: libc::c_int) {
        let exited = self
            .child
            .as_mut()
            .expect("child is still owned")
            .try_wait()
            .expect("poll child before signal");
        if let Some(status) = exited {
            let _ = self.child.take();
            panic!("bootart exited before signal delivery: {status}");
        }
        let child = self.child.as_ref().expect("child is still owned");

        // SAFETY: the PID belongs to the live, unreaped child. A PID cannot be
        // reused before this Child is reaped.
        assert_eq!(
            unsafe { libc::kill(child.id() as libc::pid_t, signal) },
            0,
            "send signal {signal} to bootart: {}",
            io::Error::last_os_error()
        );
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self
                .child
                .as_mut()
                .expect("child is still owned")
                .try_wait()
                .expect("poll child");
            if let Some(status) = status {
                self.child = None;
                return status;
            }
            if Instant::now() >= deadline {
                let mut child = self.child.take().expect("child is still owned");
                let kill_error = child.kill().err();
                let wait_error = child.wait().err();
                panic!(
                    "bootart did not exit within {timeout:?}; kill error: {kill_error:?}; wait error: {wait_error:?}"
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn spawn_on_pty(arguments: &[&str], pty: Pty) -> (ChildGuard, File) {
    let child = Command::new(env!("CARGO_BIN_EXE_bootart"))
        .args(arguments)
        .stdout(Stdio::from(pty.stdout))
        .stderr(Stdio::from(pty.stderr))
        .spawn()
        .expect("failed to spawn bootart in PTY");
    (ChildGuard::new(child), pty.master)
}

struct PtyOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

struct PtyDrain {
    ready: mpsc::Receiver<()>,
    result: mpsc::Receiver<io::Result<PtyOutput>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PtyDrain {
    fn wait_until_cursor_hidden(&self, timeout: Duration) {
        match self.ready.recv_timeout(timeout) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("bootart did not hide the cursor within {timeout:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("PTY closed before bootart hid the cursor")
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.stop.store(true, Ordering::Release);
        let output = match self.result.recv_timeout(DRAIN_TIMEOUT) {
            Ok(result) => result.expect("read PTY output"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("PTY drain did not finish within {DRAIN_TIMEOUT:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("PTY drain exited without returning output")
            }
        };
        self.join_completed();
        assert!(!output.truncated, "PTY output exceeded bounded capture");
        output.bytes
    }

    fn join_completed(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("PTY drain thread panicked");
        }
    }
}

impl Drop for PtyDrain {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        match self.result.recv_timeout(DRAIN_TIMEOUT) {
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => self.join_completed(),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The descriptor is nonblocking and the thread observes stop
                // between reads. Detach rather than making panic cleanup itself
                // unbounded if the host violates those guarantees.
                let _ = self.handle.take();
            }
        }
    }
}

fn drain_pty(mut master: File) -> PtyDrain {
    let (ready_sender, ready) = mpsc::sync_channel(1);
    let (result_sender, result) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("bootart-pty-drain".to_string())
        .spawn(move || {
            let mut output = Vec::new();
            let mut truncated = false;
            let mut readiness_tail = Vec::new();
            let mut ready_sent = false;
            let mut buffer = [0_u8; 4096];

            loop {
                let mut read_any = false;
                loop {
                    match master.read(&mut buffer) {
                        Ok(0) => {
                            let _ = result_sender.send(Ok(PtyOutput {
                                bytes: output,
                                truncated,
                            }));
                            return;
                        }
                        Ok(count) => {
                            read_any = true;
                            if !ready_sent
                                && sequence_seen(&mut readiness_tail, &buffer[..count], HIDE_CURSOR)
                            {
                                ready_sent = true;
                                let _ = ready_sender.send(());
                            }
                            let remaining = MAX_PTY_OUTPUT_BYTES.saturating_sub(output.len());
                            let retained = count.min(remaining);
                            output.extend_from_slice(&buffer[..retained]);
                            truncated |= retained != count;
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                            let _ = result_sender.send(Ok(PtyOutput {
                                bytes: output,
                                truncated,
                            }));
                            return;
                        }
                        Err(error) => {
                            let _ = result_sender.send(Err(error));
                            return;
                        }
                    }
                }

                if thread_stop.load(Ordering::Acquire) && !read_any {
                    let _ = result_sender.send(Ok(PtyOutput {
                        bytes: output,
                        truncated,
                    }));
                    return;
                }
                thread::sleep(DRAIN_RETRY);
            }
        })
        .expect("spawn PTY drain thread");

    PtyDrain {
        ready,
        result,
        stop,
        handle: Some(handle),
    }
}

fn sequence_seen(tail: &mut Vec<u8>, bytes: &[u8], needle: &[u8]) -> bool {
    let mut combined = Vec::with_capacity(tail.len() + bytes.len());
    combined.extend_from_slice(tail);
    combined.extend_from_slice(bytes);
    let found = combined
        .windows(needle.len())
        .any(|window| window == needle);

    let retained = combined.len().min(needle.len().saturating_sub(1));
    tail.clear();
    tail.extend_from_slice(&combined[combined.len() - retained..]);
    found
}

fn last_position(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn assert_cursor_restored(output: &[u8]) {
    let last_hide = last_position(output, HIDE_CURSOR).expect("PTY output should hide the cursor");
    let last_show =
        last_position(output, SHOW_CURSOR).expect("PTY output should restore a visible cursor");
    let last_reset = last_position(output, RESET_ATTRIBUTES)
        .expect("PTY output should reset terminal attributes");

    assert!(
        last_show > last_hide,
        "the final cursor transition must restore visibility"
    );
    assert!(
        last_reset > last_hide,
        "terminal attributes must be reset after the final cursor hide"
    );
}

#[test]
fn normal_exit_restores_cursor_and_attributes() {
    let (mut child, master) = spawn_on_pty(&["render-final"], open_pty());
    let drain = drain_pty(master);
    let status = child.wait_bounded(CHILD_TIMEOUT);
    let output = drain.finish();
    assert!(status.success(), "bootart exited with error in PTY");
    assert_cursor_restored(&output);
}

fn signal_exit_restores_terminal(signal: libc::c_int) {
    let (mut child, master) = spawn_on_pty(
        &[
            "preview",
            "--loop-infinitely",
            "--duration-ms",
            "1000",
            "--fps",
            "30",
            "--no-color",
            "--cols",
            "80",
            "--rows",
            "24",
        ],
        open_pty(),
    );
    let drain = drain_pty(master);
    drain.wait_until_cursor_hidden(READY_TIMEOUT);
    child.send_signal(signal);
    let status = child.wait_bounded(CHILD_TIMEOUT);
    let output = drain.finish();
    assert!(status.success(), "handled signal should exit cleanly");
    assert_cursor_restored(&output);
}

#[test]
fn sigint_restores_cursor_and_attributes() {
    signal_exit_restores_terminal(libc::SIGINT);
}

#[test]
fn sigterm_restores_cursor_and_attributes() {
    signal_exit_restores_terminal(libc::SIGTERM);
}

#[test]
fn sighup_restores_cursor_and_attributes() {
    signal_exit_restores_terminal(libc::SIGHUP);
}
