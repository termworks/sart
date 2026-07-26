//! Embedded mkinitcpio install and BusyBox runtime hooks.
//!
//! These lifecycle hooks do not implement a native password broker.

/// Build-time hook sourced by mkinitcpio.
pub const INSTALL_HOOK: &str = r#"#!/usr/bin/bash

build() {
    add_binary /usr/bin/bootart /usr/bin/bootart
    add_runscript
}

help() {
    cat <<'HELPEOF'
Adds the experimental Bootart splash daemon to early userspace.
The boot remains fail-open and bootart=0 or rd.bootart=0 disables the daemon.
HELPEOF
}
"#;

/// Runtime hook sourced by mkinitcpio's BusyBox init.
pub const RUNTIME_HOOK: &str = r#"#!/usr/bin/ash

run_earlyhook() {
    if [ ! -x /usr/bin/bootart ]; then
        return 0
    fi
    if /usr/bin/bootart ping >/dev/null 2>&1; then
        return 0
    fi
    /usr/bin/bootart daemon --mode boot </dev/null >/dev/null 2>&1 &
    return 0
}

run_cleanuphook() {
    if [ ! -x /usr/bin/bootart ]; then
        return 0
    fi
    bootart_new_root=/new_root
    if [ -d "$bootart_new_root" ]; then
        if ! /usr/bin/bootart update-root-fs "$bootart_new_root" >/dev/null 2>&1; then
            /usr/bin/bootart quit >/dev/null 2>&1 || :
        fi
    fi
    unset bootart_new_root
    return 0
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hook_uses_mkinitcpio_apis() {
        assert!(INSTALL_HOOK.contains("add_binary /usr/bin/bootart /usr/bin/bootart"));
        assert!(INSTALL_HOOK.contains("add_runscript"));
        assert!(!INSTALL_HOOK.contains("cp "));
    }

    #[test]
    fn runtime_hook_starts_a_child_and_hands_off() {
        assert!(RUNTIME_HOOK.contains("run_earlyhook()"));
        assert!(RUNTIME_HOOK.contains("run_cleanuphook()"));
        assert!(RUNTIME_HOOK.contains("daemon --mode boot"));
        assert!(RUNTIME_HOOK.contains("update-root-fs"));
        assert!(RUNTIME_HOOK.contains("if ! /usr/bin/bootart update-root-fs"));
        assert!(RUNTIME_HOOK.contains("/usr/bin/bootart quit"));
        assert!(!RUNTIME_HOOK.contains("exec /usr/bin/bootart"));
        assert!(!RUNTIME_HOOK.contains("systemctl"));
    }
}
