use std::fmt;

pub const PID1_REFUSAL_EXIT_CODE: i32 = 126;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pid1Refused;

impl fmt::Display for Pid1Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "bootart refuses to run as PID 1; start it as a child of the real init system",
        )
    }
}

impl std::error::Error for Pid1Refused {}

pub fn ensure_not_pid1(pid: u32) -> Result<(), Pid1Refused> {
    if pid == 1 { Err(Pid1Refused) } else { Ok(()) }
}

pub fn ensure_current_process_not_pid1() -> Result<(), Pid1Refused> {
    ensure_not_pid1(std::process::id())
}

/// Cross the binary entry boundary only after the supplied process identity is
/// known not to be PID 1. Keeping the continuation behind `FnOnce` makes the
/// no-parser/no-I/O refusal order behaviorally testable without launching the
/// product executable as PID 1.
pub fn run_after_pid1_guard<T>(
    pid: u32,
    continuation: impl FnOnce() -> T,
) -> Result<T, Pid1Refused> {
    ensure_not_pid1(pid)?;
    Ok(continuation())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_one_is_rejected() {
        assert_eq!(ensure_not_pid1(1), Err(Pid1Refused));
        assert_eq!(PID1_REFUSAL_EXIT_CODE, 126);
    }

    #[test]
    fn ordinary_processes_are_accepted() {
        assert!(ensure_not_pid1(2).is_ok());
        assert!(ensure_not_pid1(u32::MAX).is_ok());
    }

    #[test]
    fn pid_one_never_invokes_the_entry_continuation() {
        let invoked = std::cell::Cell::new(false);
        let result = run_after_pid1_guard(1, || invoked.set(true));
        assert_eq!(result, Err(Pid1Refused));
        assert!(!invoked.get());

        assert_eq!(run_after_pid1_guard(2, || 42), Ok(42));
    }
}
