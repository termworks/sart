use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

pub const EMBEDDED_INIT_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bootart-init"));

pub enum DistroInitramfs {
    InitramfsTools,
    Mkinitcpio,
    Dracut,
    Unknown,
}

pub fn detect_distro() -> DistroInitramfs {
    if Path::new("/usr/sbin/update-initramfs").exists() || Command::new("which").arg("update-initramfs").output().map(|o| o.status.success()).unwrap_or(false) {
        DistroInitramfs::InitramfsTools
    } else if Path::new("/usr/bin/mkinitcpio").exists() || Command::new("which").arg("mkinitcpio").output().map(|o| o.status.success()).unwrap_or(false) {
        DistroInitramfs::Mkinitcpio
    } else if Path::new("/usr/bin/dracut").exists() || Command::new("which").arg("dracut").output().map(|o| o.status.success()).unwrap_or(false) {
        DistroInitramfs::Dracut
    } else {
        DistroInitramfs::Unknown
    }
}

pub fn install_hooks(custom_asset: Option<&Path>) -> Result<(), String> {
    println!("==> Bootart Self-Installer");

    // 1. Write the embedded static init binary to /usr/local/bin/bootart-init
    let target_init_bin = Path::new("/usr/local/bin/bootart-init");
    println!("==> Extracting embedded static init binary to {:?}...", target_init_bin);
    fs::write(target_init_bin, EMBEDDED_INIT_BIN).map_err(|e| format!("Failed to write embedded binary to {:?}: {} (run with sudo)", target_init_bin, e))?;
    fs::set_permissions(target_init_bin, fs::Permissions::from_mode(0o755)).map_err(|e| format!("Failed to set permissions on {:?}: {}", target_init_bin, e))?;

    // Also copy current management binary to /usr/local/bin/bootart
    if let Ok(current_exe) = std::env::current_exe() {
        let target_cli_bin = Path::new("/usr/local/bin/bootart");
        println!("==> Installing CLI binary to {:?}...", target_cli_bin);
        let _ = fs::copy(current_exe, target_cli_bin);
        let _ = fs::set_permissions(target_cli_bin, fs::Permissions::from_mode(0o755));
    }

    // 2. Install logo asset
    let asset_dir = Path::new("/etc/bootart");
    fs::create_dir_all(asset_dir).map_err(|e| format!("Failed to create asset directory {:?}: {}", asset_dir, e))?;
    let asset_path = asset_dir.join("logo.txt");

    if let Some(custom) = custom_asset {
        println!("==> Installing custom logo asset from {:?}...", custom);
        fs::copy(custom, &asset_path).map_err(|e| format!("Failed to copy custom asset {:?}: {}", custom, e))?;
    } else {
        println!("==> Installing default logo asset to {:?}...", asset_path);
        let default_logo = include_str!("../assets/logo.txt");
        fs::write(&asset_path, default_logo).map_err(|e| format!("Failed to write logo asset: {}", e))?;
    }

    // 3. Detect Distro Initramfs Framework & Install Hooks
    match detect_distro() {
        DistroInitramfs::InitramfsTools => {
            println!("==> Detected initramfs-tools (Debian/Ubuntu/Mint)");
            let hook_dir = Path::new("/etc/initramfs-tools/scripts/init-top");
            fs::create_dir_all(hook_dir).map_err(|e| format!("Failed to create hook directory {:?}: {}", hook_dir, e))?;
            let hook_file = hook_dir.join("bootart");

            let hook_script = r#"#!/bin/sh
PREREQ=""
prereqs() { echo "$PREREQ"; }
case $1 in prereqs) prereqs; exit 0;; esac

if [ -x /usr/local/bin/bootart-init ]; then
    /usr/local/bin/bootart-init
elif [ -x /bin/bootart-init ]; then
    /bin/bootart-init
elif [ -x /bin/bootart ]; then
    /bin/bootart play
fi
"#;
            fs::write(&hook_file, hook_script).map_err(|e| format!("Failed to write hook script {:?}: {}", hook_file, e))?;
            fs::set_permissions(&hook_file, fs::Permissions::from_mode(0o755)).map_err(|e| format!("Failed to set permissions on {:?}: {}", hook_file, e))?;

            println!("==> Updating initramfs image...");
            let status = Command::new("update-initramfs").arg("-u").status().map_err(|e| format!("Failed to execute update-initramfs: {}", e))?;
            if !status.success() {
                return Err("update-initramfs returned non-zero exit code".to_string());
            }
        }
        DistroInitramfs::Mkinitcpio => {
            println!("==> Detected mkinitcpio (Arch Linux/Manjaro)");
            let hook_dir = Path::new("/etc/initcpio/hooks");
            let install_dir = Path::new("/etc/initcpio/install");
            fs::create_dir_all(hook_dir).map_err(|e| format!("Failed to create hook directory {:?}: {}", hook_dir, e))?;
            fs::create_dir_all(install_dir).map_err(|e| format!("Failed to create install directory {:?}: {}", install_dir, e))?;

            let install_script = r#"build() {
    add_binary /usr/local/bin/bootart-init /bin/bootart-init
    add_file /etc/bootart/logo.txt /etc/bootart/logo.txt
    add_runscript
}
"#;
            let hook_script = r#"run_hook() {
    /bin/bootart-init
}
"#;
            fs::write(install_dir.join("bootart"), install_script).map_err(|e| format!("Failed to write mkinitcpio install script: {}", e))?;
            fs::write(hook_dir.join("bootart"), hook_script).map_err(|e| format!("Failed to write mkinitcpio hook script: {}", e))?;

            let conf_path = Path::new("/etc/mkinitcpio.conf");
            if conf_path.exists() {
                let conf_content = fs::read_to_string(conf_path).unwrap_or_default();
                if !conf_content.contains("bootart") {
                    println!("==> Adding bootart to HOOKS in /etc/mkinitcpio.conf...");
                    let new_conf = conf_content.replace("HOOKS=(", "HOOKS=(bootart ");
                    fs::write(conf_path, new_conf).map_err(|e| format!("Failed to update /etc/mkinitcpio.conf: {}", e))?;
                }
            }

            println!("==> Rebuilding initcpio images...");
            let status = Command::new("mkinitcpio").arg("-P").status().map_err(|e| format!("Failed to execute mkinitcpio: {}", e))?;
            if !status.success() {
                return Err("mkinitcpio returned non-zero exit code".to_string());
            }
        }
        DistroInitramfs::Dracut => {
            println!("==> Detected dracut (Fedora/RHEL/CentOS)");
            let mod_dir = Path::new("/usr/lib/dracut/modules.d/99bootart");
            fs::create_dir_all(mod_dir).map_err(|e| format!("Failed to create dracut module directory {:?}: {}", mod_dir, e))?;

            let module_setup = r#"#!/bin/bash
check() { return 0; }
depends() { return 0; }
install() {
    inst /usr/local/bin/bootart-init /bin/bootart-init
    inst /etc/bootart/logo.txt /etc/bootart/logo.txt
    inst_hook pre-pivot 99 "${moddir}/bootart-run.sh"
}
"#;
            let bootart_run = r#"#!/bin/sh
/bin/bootart-init
"#;
            fs::write(mod_dir.join("module-setup.sh"), module_setup).map_err(|e| format!("Failed to write module-setup.sh: {}", e))?;
            fs::set_permissions(mod_dir.join("module-setup.sh"), fs::Permissions::from_mode(0o755)).ok();
            fs::write(mod_dir.join("bootart-run.sh"), bootart_run).map_err(|e| format!("Failed to write bootart-run.sh: {}", e))?;
            fs::set_permissions(mod_dir.join("bootart-run.sh"), fs::Permissions::from_mode(0o755)).ok();

            println!("==> Rebuilding dracut initramfs...");
            let status = Command::new("dracut").arg("--force").status().map_err(|e| format!("Failed to execute dracut: {}", e))?;
            if !status.success() {
                return Err("dracut returned non-zero exit code".to_string());
            }
        }
        DistroInitramfs::Unknown => {
            println!("==> Binary installed to {:?}. (No known initramfs generator detected).", target_init_bin);
        }
    }

    println!("==> Bootart hook installation completed successfully!");
    Ok(())
}

pub fn uninstall_hooks() -> Result<(), String> {
    println!("==> Uninstalling Bootart Hooks...");

    let target_init_bin = Path::new("/usr/local/bin/bootart-init");
    if target_init_bin.exists() {
        println!("==> Removing {:?}", target_init_bin);
        fs::remove_file(target_init_bin).ok();
    }

    let init_top = Path::new("/etc/initramfs-tools/scripts/init-top/bootart");
    if init_top.exists() {
        println!("==> Removing {:?}", init_top);
        fs::remove_file(init_top).ok();
        Command::new("update-initramfs").arg("-u").status().ok();
    }

    let mk_install = Path::new("/etc/initcpio/install/bootart");
    let mk_hook = Path::new("/etc/initcpio/hooks/bootart");
    if mk_install.exists() || mk_hook.exists() {
        println!("==> Removing mkinitcpio hooks...");
        fs::remove_file(mk_install).ok();
        fs::remove_file(mk_hook).ok();
        Command::new("mkinitcpio").arg("-P").status().ok();
    }

    let dracut_mod = Path::new("/usr/lib/dracut/modules.d/99bootart");
    if dracut_mod.exists() {
        println!("==> Removing dracut module {:?}...", dracut_mod);
        fs::remove_dir_all(dracut_mod).ok();
        Command::new("dracut").arg("--force").status().ok();
    }

    println!("==> Bootart uninstalled successfully.");
    Ok(())
}

pub fn status_hooks() {
    println!("==> Bootart Installation Status:");
    let target_bin = Path::new("/usr/local/bin/bootart-init");
    println!("  Embedded Static Init Binary (/usr/local/bin/bootart-init): {}", if target_bin.exists() { "Installed" } else { "Not installed" });

    let logo_asset = Path::new("/etc/bootart/logo.txt");
    println!("  Logo Asset (/etc/bootart/logo.txt): {}", if logo_asset.exists() { "Present" } else { "Not present" });

    match detect_distro() {
        DistroInitramfs::InitramfsTools => {
            let hook = Path::new("/etc/initramfs-tools/scripts/init-top/bootart");
            println!("  Distro: Debian/Ubuntu (initramfs-tools)");
            println!("  Hook ({:?}): {}", hook, if hook.exists() { "Active" } else { "Not installed" });
        }
        DistroInitramfs::Mkinitcpio => {
            let hook = Path::new("/etc/initcpio/install/bootart");
            println!("  Distro: Arch Linux (mkinitcpio)");
            println!("  Hook ({:?}): {}", hook, if hook.exists() { "Active" } else { "Not installed" });
        }
        DistroInitramfs::Dracut => {
            let hook = Path::new("/usr/lib/dracut/modules.d/99bootart");
            println!("  Distro: Fedora/RHEL (dracut)");
            println!("  Hook ({:?}): {}", hook, if hook.exists() { "Active" } else { "Not installed" });
        }
        DistroInitramfs::Unknown => {
            println!("  Distro: Generic / Unknown");
        }
    }
}
