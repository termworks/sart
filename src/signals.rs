use std::sync::atomic::{AtomicBool, Ordering};
use std::{io, mem, ptr};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
const HANDLED_SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

extern "C" fn handle_signal(_sig: libc::c_int) {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Owns the process signal dispositions installed by
/// [`setup_signal_handlers`].
///
/// The guard must remain alive for the entire presentation/runtime boundary.
/// Dropping it restores the dispositions that were active before installation.
#[must_use = "dropping the signal guard immediately restores the previous handlers"]
pub struct SignalGuard {
    previous: [libc::sigaction; HANDLED_SIGNALS.len()],
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        // Restore in reverse installation order. There is no useful recovery
        // available from Drop, but every disposition is attempted even if one
        // restoration unexpectedly fails.
        for (signal, previous) in HANDLED_SIGNALS.iter().zip(self.previous.iter()).rev() {
            unsafe {
                libc::sigaction(*signal, previous, ptr::null_mut());
            }
        }
    }
}

pub fn setup_signal_handlers() -> io::Result<SignalGuard> {
    // Install every handler before a display or terminal is acquired. A
    // partial installation is rolled back before an error reaches the caller.
    unsafe {
        let mut sa: libc::sigaction = mem::zeroed();
        sa.sa_sigaction = handle_signal as *const () as usize;
        // Do not use SA_RESTART. A splash write to a stalled terminal must be
        // interruptible so SIGINT/SIGTERM can reach the restoration boundary
        // instead of re-entering the same blocking write indefinitely.
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);

        let mut previous: [libc::sigaction; HANDLED_SIGNALS.len()] = mem::zeroed();
        for (index, signal) in HANDLED_SIGNALS.iter().copied().enumerate() {
            if libc::sigaction(signal, &sa, &mut previous[index]) == -1 {
                let install_error = io::Error::last_os_error();
                for rollback in (0..index).rev() {
                    libc::sigaction(
                        HANDLED_SIGNALS[rollback],
                        &previous[rollback],
                        ptr::null_mut(),
                    );
                }
                return Err(install_error);
            }
        }

        Ok(SignalGuard { previous })
    }
}

pub fn should_stop() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

pub fn reset_stop_flag() {
    STOP_REQUESTED.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_flag() {
        reset_stop_flag();
        assert!(!should_stop());
        handle_signal(libc::SIGTERM);
        assert!(should_stop());
        reset_stop_flag();
        assert!(!should_stop());
    }
}
