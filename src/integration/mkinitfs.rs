//! Embedded Alpine mkinitfs resources.
//!
//! Stock mkinitfs feature files reference files but do not define a portable
//! runtime hook lifecycle.  The call-site snippets below therefore remain
//! experimental data for a future structural `initramfs-init` adapter; their
//! presence must never be reported as working mkinitfs support. No mkinitfs
//! password broker is present.

/// Feature manifest consumed by mkinitfs.
pub const FEATURE_FILES: &str = r#"/usr/bin/bootart
/usr/libexec/bootart/mkinitfs-runtime
"#;

/// BusyBox-compatible hook invoked only from reviewed insertion points in the
/// distro's real initramfs init script.
pub const RUNTIME_HOOK: &str = r#"#!/bin/sh

case "${1:-}" in
    start)
        if [ -x /usr/bin/bootart ] && \
            ! /usr/bin/bootart ping >/dev/null 2>&1
        then
            /usr/bin/bootart daemon --mode boot </dev/null >/dev/null 2>&1 &
        fi
        ;;
    handoff)
        bootart_new_root="${2:-/sysroot}"
        if [ -x /usr/bin/bootart ] && [ -d "$bootart_new_root" ]; then
            if ! /usr/bin/bootart update-root-fs "$bootart_new_root" \
                >/dev/null 2>&1; then
                /usr/bin/bootart quit >/dev/null 2>&1 || :
            fi
        fi
        unset bootart_new_root
        ;;
    *)
        ;;
esac
exit 0
"#;

/// Managed snippet for the reviewed post-cmdline, post-`/run` insertion point.
pub const EARLY_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-early-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime start || :
fi
# bootart:end mkinitfs-early-v1
"#;

/// Managed snippet for the reviewed point after `$sysroot` is mounted and
/// before Alpine moves initramfs mounts into it.
pub const HANDOFF_CALL_SNIPPET: &str = r#"# bootart:begin mkinitfs-handoff-v1
if [ -x /usr/libexec/bootart/mkinitfs-runtime ]; then
    /usr/libexec/bootart/mkinitfs-runtime handoff "$sysroot" || :
fi
# bootart:end mkinitfs-handoff-v1
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_contains_only_the_single_elf_and_its_data_hook() {
        let files: Vec<_> = FEATURE_FILES.lines().collect();
        assert_eq!(
            files,
            ["/usr/bin/bootart", "/usr/libexec/bootart/mkinitfs-runtime"]
        );
    }

    #[test]
    fn runtime_hook_never_becomes_init() {
        assert!(RUNTIME_HOOK.contains("daemon --mode boot"));
        assert!(RUNTIME_HOOK.contains("update-root-fs"));
        assert!(RUNTIME_HOOK.contains("if ! /usr/bin/bootart update-root-fs"));
        assert!(RUNTIME_HOOK.contains("/usr/bin/bootart quit"));
        assert!(!RUNTIME_HOOK.contains("exec /usr/bin/bootart"));
        assert!(RUNTIME_HOOK.ends_with("exit 0\n"));
    }

    #[test]
    fn managed_snippets_are_delimited_and_fail_open() {
        for snippet in [EARLY_CALL_SNIPPET, HANDOFF_CALL_SNIPPET] {
            assert!(snippet.contains("# bootart:begin"));
            assert!(snippet.contains("# bootart:end"));
            assert!(snippet.contains("|| :"));
        }
    }
}
