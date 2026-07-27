use std::fs;
use std::io;
use std::path::Path;

pub const PROC_CMDLINE: &str = "/proc/cmdline";

/// Return true only for the documented, exact splash-disable tokens.
///
/// Values such as `bootart=01`, `xbootart=0`, or `bootart=0x` must not
/// accidentally disable the daemon.
pub fn splash_disabled(cmdline: &str) -> bool {
    cmdline
        .split_ascii_whitespace()
        .any(|token| matches!(token, "bootart=0" | "rd.bootart=0"))
}

pub fn splash_disabled_at(path: &Path) -> io::Result<bool> {
    fs::read_to_string(path).map(|cmdline| splash_disabled(&cmdline))
}

/// Return whether an early-boot adapter may acquire the splash display.
///
/// An unreadable command line is deliberately treated the same as an explicit
/// disable token. The boot must continue through its stock presentation and
/// password path rather than letting an adapter guess that Bootart is enabled.
pub fn early_boot_enabled_at(path: &Path) -> bool {
    matches!(splash_disabled_at(path), Ok(false))
}

pub fn splash_disabled_for_current_boot() -> io::Result<bool> {
    splash_disabled_at(Path::new(PROC_CMDLINE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_disable_tokens_are_recognized() {
        assert!(splash_disabled("quiet bootart=0 root=/dev/vda"));
        assert!(splash_disabled("rd.bootart=0\n"));
    }

    #[test]
    fn similar_tokens_do_not_disable() {
        for cmdline in [
            "",
            "bootart=1",
            "bootart=01",
            "xbootart=0",
            "bootart=0x",
            "bootart =0",
        ] {
            assert!(
                !splash_disabled(cmdline),
                "unexpected match for {cmdline:?}"
            );
        }
    }

    #[test]
    fn early_boot_predicate_fails_open_to_the_stock_boot_path() {
        assert!(early_boot_enabled_at(Path::new("/dev/null")));
        assert!(!early_boot_enabled_at(Path::new(
            "/bootart-test-path-that-must-not-exist/cmdline",
        )));
    }
}
