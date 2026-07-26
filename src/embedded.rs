//! Content that must be available before the real root filesystem is mounted.
//!
//! Production resources in this module are Rust string literals. Do not
//! replace them with compile-time file-inclusion macros: that would make an
//! external asset part of the production build input again.

/// Version of the embedded resource set.
///
/// This is independent of the daemon control-protocol version.  Increment it
/// when a materialized integration resource changes incompatibly.
pub const RESOURCE_SET_VERSION: u16 = 3;

/// Full-size art used when no valid user override is available.
pub const DEFAULT_ART: &str = r#"              ▄▄▄▄▄▄▄▄              
         ▄▄██████████████▄▄         
      ▄██████████████████████▄      
    ▄██████████████████████████▄    
  ▄█▀▄████████████████████████▄▀█▄  
 ▄█  ██████████████████████████  █▄ 
▄█▀ ▄██████████████████████████▄ ▀█▄
█▀  ████████████████████████████  ▀█
  ▄██████████████████████████████▄  
████████████████████████████████████
████████████████████████████████████
▀██▀  ▀▀████████████████████▀▀  ▀██▀
 ██       ▀██▀████████▀██▀       ██ 
  ██        ▀█ ██████ █▀        ██  
   ██▄        █ ████ █        ▄██   
    ███▄▄▄▄    █ ██ █    ▄▄▄▄███    
     ▀▀▀▀▀████▄██████▄████▀▀▀▀▀     
        █▄ █████▄██▄█████ ▄█        
        ██▄ ████████████ ▄██        
         ▀█████▀▄▄▄▄▀█████▀         
           ▀▀██████████▀▀           
              ▀██████▀              
"#;

/// Compact fallback art for displays that cannot fit [`DEFAULT_ART`].
pub const SMALL_ART: &str = r#" ___  ____ ___  ____ ___ 
|__] |  |  |  |__|  |  
|__] |__|  |  |  |  |  
"#;

/// Reviewable built-in defaults used when no future optional override is
/// present or valid. The runtime currently exposes these as typed defaults;
/// this manifest keeps the complete default configuration embedded and
/// inspectable without requiring an external file in the initramfs.
pub const DEFAULT_CONFIG: &str = r#"schema=bootart.config
version=1
runtime_dir=/run/bootart
mode=boot
password_broker=none
vt=open-query
frames_per_second=30
animation_cycle_ms=2500
seed=42
no_color=false
control_protocol=1
"#;

/// Identifies built-in art without exposing its storage details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtId {
    Default,
    Small,
}

impl ArtId {
    pub const ALL: &'static [Self] = &[Self::Default, Self::Small];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "art.default",
            Self::Small => "art.small",
        }
    }
}

macro_rules! define_template_ids {
    ($($variant:ident => $stable_name:literal),+ $(,)?) => {
        /// Stable, exhaustive identifier for integration content embedded in
        /// the ELF. `ALL` and the stable names are generated together so a new
        /// variant cannot silently disappear from resource audits.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum TemplateId {
            $($variant),+
        }

        impl TemplateId {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stable_name),+
                }
            }
        }
    };
}

define_template_ids! {
    SystemdStartUnit => "systemd.start-unit",
    SystemdShowUnit => "systemd.show-unit",
    SystemdSwitchRootUnit => "systemd.switch-root-unit",
    SystemdQuitUnit => "systemd.quit-unit",
    SystemdQuitWaitUnit => "systemd.quit-wait-unit",
    DracutSystemdModuleSetup => "dracut.systemd-module-setup",
    DracutClassicModuleSetup => "dracut.classic-module-setup",
    DracutClassicStartHook => "dracut.classic-start-hook",
    DracutClassicAskpassPatchHook => "dracut.classic-askpass-patch-hook",
    DracutClassicAskpassOverride => "dracut.classic-askpass-override",
    DracutClassicPrePivotHook => "dracut.classic-pre-pivot-hook",
    MkinitcpioInstallHook => "mkinitcpio.install-hook",
    MkinitcpioRuntimeHook => "mkinitcpio.runtime-hook",
    InitramfsToolsBuildHook => "initramfs-tools.build-hook",
    InitramfsToolsAskpassWrapper => "initramfs-tools.askpass-wrapper",
    InitramfsToolsEarlyHook => "initramfs-tools.early-hook",
    InitramfsToolsBottomHook => "initramfs-tools.bottom-hook",
    MkinitfsFeatureFiles => "mkinitfs.feature-files",
    MkinitfsRuntimeHook => "mkinitfs.runtime-hook",
    MkinitfsEarlyCallSnippet => "mkinitfs.early-call-snippet",
    MkinitfsHandoffCallSnippet => "mkinitfs.handoff-call-snippet",
    OpenRcSupervisorScript => "openrc.supervisor-script",
    OpenRcQuitScript => "openrc.quit-script",
}

/// How an installer would materialize an embedded template.  This is
/// declarative metadata only; this module never mutates a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateMaterialization {
    File {
        path: &'static str,
        mode: u32,
    },
    OpenRcService {
        path: &'static str,
        mode: u32,
        runlevel: &'static str,
    },
    ManagedSnippet {
        target: &'static str,
        insertion_point: &'static str,
    },
}

/// Embedded template metadata. Every current entry is deliberately unproven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateResource {
    pub id: TemplateId,
    pub materialization: TemplateMaterialization,
    pub contents: &'static str,
    pub experimental_unproven: bool,
}

/// Identifies any text resource compiled into the executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceId {
    Art(ArtId),
    Template(TemplateId),
    DefaultConfig,
}

impl ResourceId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Art(id) => id.as_str(),
            Self::Template(id) => id.as_str(),
            Self::DefaultConfig => "config.default",
        }
    }
}

/// Returns built-in art by its typed identifier.
pub const fn art(id: ArtId) -> &'static str {
    match id {
        ArtId::Default => DEFAULT_ART,
        ArtId::Small => SMALL_ART,
    }
}

/// Returns declarative metadata and embedded content for an integration
/// resource. Availability does not mean support; every entry remains marked
/// experimental/unproven until its exact VM lane passes.
pub const fn template_resource(id: TemplateId) -> TemplateResource {
    use crate::integration::{dracut, initramfs_tools, mkinitcpio, mkinitfs, openrc, systemd};

    let (materialization, contents) = match id {
        TemplateId::SystemdStartUnit => (
            TemplateMaterialization::File {
                path: "/usr/lib/systemd/system/bootart-start.service",
                mode: 0o644,
            },
            systemd::START_UNIT,
        ),
        TemplateId::SystemdShowUnit => (
            TemplateMaterialization::File {
                path: "/usr/lib/systemd/system/bootart-show.service",
                mode: 0o644,
            },
            systemd::SHOW_UNIT,
        ),
        TemplateId::SystemdSwitchRootUnit => (
            TemplateMaterialization::File {
                path: "/usr/lib/systemd/system/bootart-switch-root.service",
                mode: 0o644,
            },
            systemd::SWITCH_ROOT_UNIT,
        ),
        TemplateId::SystemdQuitUnit => (
            TemplateMaterialization::File {
                path: "/usr/lib/systemd/system/bootart-quit.service",
                mode: 0o644,
            },
            systemd::QUIT_UNIT,
        ),
        TemplateId::SystemdQuitWaitUnit => (
            TemplateMaterialization::File {
                path: "/usr/lib/systemd/system/bootart-quit-wait.service",
                mode: 0o644,
            },
            systemd::QUIT_WAIT_UNIT,
        ),
        TemplateId::DracutSystemdModuleSetup => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-systemd/module-setup.sh",
                mode: 0o755,
            },
            dracut::SYSTEMD_MODULE_SETUP,
        ),
        TemplateId::DracutClassicModuleSetup => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-classic/module-setup.sh",
                mode: 0o755,
            },
            dracut::CLASSIC_MODULE_SETUP,
        ),
        TemplateId::DracutClassicStartHook => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-classic/bootart-start.sh",
                mode: 0o755,
            },
            dracut::CLASSIC_START_HOOK,
        ),
        TemplateId::DracutClassicAskpassPatchHook => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-classic/bootart-askpass-patch.sh",
                mode: 0o755,
            },
            dracut::CLASSIC_ASKPASS_PATCH_HOOK,
        ),
        TemplateId::DracutClassicAskpassOverride => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-classic/bootart-askpass-lib.sh",
                mode: 0o644,
            },
            dracut::CLASSIC_ASKPASS_OVERRIDE,
        ),
        TemplateId::DracutClassicPrePivotHook => (
            TemplateMaterialization::File {
                path: "/usr/lib/dracut/modules.d/60bootart-classic/bootart-pre-pivot.sh",
                mode: 0o755,
            },
            dracut::CLASSIC_PRE_PIVOT_HOOK,
        ),
        TemplateId::MkinitcpioInstallHook => (
            TemplateMaterialization::File {
                path: "/usr/lib/initcpio/install/bootart",
                mode: 0o755,
            },
            mkinitcpio::INSTALL_HOOK,
        ),
        TemplateId::MkinitcpioRuntimeHook => (
            TemplateMaterialization::File {
                path: "/usr/lib/initcpio/hooks/bootart",
                mode: 0o755,
            },
            mkinitcpio::RUNTIME_HOOK,
        ),
        TemplateId::InitramfsToolsBuildHook => (
            TemplateMaterialization::File {
                path: "/usr/share/initramfs-tools/hooks/bootart",
                mode: 0o755,
            },
            initramfs_tools::BUILD_HOOK,
        ),
        TemplateId::InitramfsToolsAskpassWrapper => (
            TemplateMaterialization::File {
                path: "/usr/lib/bootart/initramfs-tools-askpass",
                mode: 0o755,
            },
            initramfs_tools::ASKPASS_WRAPPER,
        ),
        TemplateId::InitramfsToolsEarlyHook => (
            TemplateMaterialization::File {
                path: "/usr/share/initramfs-tools/scripts/init-top/bootart",
                mode: 0o755,
            },
            initramfs_tools::EARLY_HOOK,
        ),
        TemplateId::InitramfsToolsBottomHook => (
            TemplateMaterialization::File {
                path: "/usr/share/initramfs-tools/scripts/init-bottom/bootart",
                mode: 0o755,
            },
            initramfs_tools::BOTTOM_HOOK,
        ),
        TemplateId::MkinitfsFeatureFiles => (
            TemplateMaterialization::File {
                path: "/etc/mkinitfs/features.d/bootart.files",
                mode: 0o644,
            },
            mkinitfs::FEATURE_FILES,
        ),
        TemplateId::MkinitfsRuntimeHook => (
            TemplateMaterialization::File {
                path: "/usr/libexec/bootart/mkinitfs-runtime",
                mode: 0o755,
            },
            mkinitfs::RUNTIME_HOOK,
        ),
        TemplateId::MkinitfsEarlyCallSnippet => (
            TemplateMaterialization::ManagedSnippet {
                target: "/usr/share/mkinitfs/initramfs-init",
                insertion_point: "post-cmdline-and-runtime-mounts",
            },
            mkinitfs::EARLY_CALL_SNIPPET,
        ),
        TemplateId::MkinitfsHandoffCallSnippet => (
            TemplateMaterialization::ManagedSnippet {
                target: "/usr/share/mkinitfs/initramfs-init",
                insertion_point: "post-sysroot-mount-before-mount-move",
            },
            mkinitfs::HANDOFF_CALL_SNIPPET,
        ),
        TemplateId::OpenRcSupervisorScript => (
            TemplateMaterialization::OpenRcService {
                path: "/etc/init.d/bootart",
                mode: 0o755,
                runlevel: "boot",
            },
            openrc::SUPERVISOR_SCRIPT,
        ),
        TemplateId::OpenRcQuitScript => (
            TemplateMaterialization::OpenRcService {
                path: "/etc/init.d/bootart-quit",
                mode: 0o755,
                runlevel: "default",
            },
            openrc::QUIT_SCRIPT,
        ),
    };

    TemplateResource {
        id,
        materialization,
        contents,
        experimental_unproven: true,
    }
}

/// Resolves embedded template text by typed identifier.
pub const fn template(id: TemplateId) -> Option<&'static str> {
    Some(template_resource(id).contents)
}

/// Resolves a typed embedded resource.
pub const fn resource(id: ResourceId) -> Option<&'static str> {
    match id {
        ResourceId::Art(id) => Some(art(id)),
        ResourceId::Template(id) => template(id),
        ResourceId::DefaultConfig => Some(DEFAULT_CONFIG),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_resources_are_available_by_typed_id() {
        for &id in ArtId::ALL {
            assert!(!id.as_str().is_empty());
            assert_eq!(resource(ResourceId::Art(id)), Some(art(id)));
        }
    }

    #[test]
    fn default_configuration_is_embedded_and_matches_typed_runtime_defaults() {
        use crate::splash::engine::{DEFAULT_ANIMATION_CYCLE, DEFAULT_FRAMES_PER_SECOND};
        use crate::splash::protocol::PROTOCOL_VERSION;
        use crate::splash::runtime::DEFAULT_RUNTIME_DIR;

        assert_eq!(resource(ResourceId::DefaultConfig), Some(DEFAULT_CONFIG));
        assert!(DEFAULT_CONFIG.ends_with('\n'));
        assert!(DEFAULT_CONFIG.contains(&format!("runtime_dir={DEFAULT_RUNTIME_DIR}\n")));
        assert!(
            DEFAULT_CONFIG.contains(&format!("frames_per_second={DEFAULT_FRAMES_PER_SECOND}\n"))
        );
        assert!(DEFAULT_CONFIG.contains(&format!(
            "animation_cycle_ms={}\n",
            DEFAULT_ANIMATION_CYCLE.as_millis()
        )));
        assert!(DEFAULT_CONFIG.contains(&format!("control_protocol={PROTOCOL_VERSION}\n")));
        for required in [
            "mode=boot\n",
            "password_broker=none\n",
            "vt=open-query\n",
            "seed=42\n",
            "no_color=false\n",
        ] {
            assert!(DEFAULT_CONFIG.contains(required));
        }
    }

    #[test]
    fn templates_resolve_to_nonempty_reviewable_content() {
        for &id in TemplateId::ALL {
            assert!(!id.as_str().is_empty());
            let embedded = template_resource(id);
            assert_eq!(embedded.id, id);
            assert!(embedded.experimental_unproven);
            assert!(!embedded.contents.is_empty());
            assert!(embedded.contents.ends_with('\n'));
            assert!(!embedded.contents.contains('\0'));
            assert!(!embedded.contents.contains('\r'));
            assert_eq!(template(id), Some(embedded.contents));
            assert_eq!(resource(ResourceId::Template(id)), Some(embedded.contents));
        }
    }

    #[test]
    fn resource_ids_and_file_destinations_are_unique_and_safe() {
        use std::collections::BTreeSet;

        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for &id in TemplateId::ALL {
            assert!(ids.insert(id.as_str()));
            let embedded = template_resource(id);
            match embedded.materialization {
                TemplateMaterialization::File { path, mode }
                | TemplateMaterialization::OpenRcService {
                    path,
                    mode,
                    runlevel: _,
                } => {
                    assert!(path.starts_with('/'));
                    assert!(!path.split('/').any(|component| component == ".."));
                    assert!(matches!(mode, 0o644 | 0o755));
                    assert!(paths.insert(path));
                    if mode == 0o755 {
                        assert!(embedded.contents.starts_with("#!"));
                    }
                }
                TemplateMaterialization::ManagedSnippet {
                    target,
                    insertion_point,
                } => {
                    assert!(target.starts_with('/'));
                    assert!(!insertion_point.is_empty());
                    assert!(embedded.contents.contains("# bootart:begin"));
                    assert!(embedded.contents.contains("# bootart:end"));
                }
            }
        }
        assert_eq!(ids.len(), TemplateId::ALL.len());
    }

    #[test]
    fn openrc_activation_has_an_explicit_boot_complete_point() {
        assert!(matches!(
            template_resource(TemplateId::OpenRcSupervisorScript).materialization,
            TemplateMaterialization::OpenRcService {
                path: "/etc/init.d/bootart",
                runlevel: "boot",
                ..
            }
        ));
        assert!(matches!(
            template_resource(TemplateId::OpenRcQuitScript).materialization,
            TemplateMaterialization::OpenRcService {
                path: "/etc/init.d/bootart-quit",
                runlevel: "default",
                ..
            }
        ));
    }

    #[test]
    fn integration_data_never_materializes_a_pid1_or_sibling_elf() {
        for &id in TemplateId::ALL {
            let embedded = template_resource(id);
            match embedded.materialization {
                TemplateMaterialization::File { path, .. }
                | TemplateMaterialization::OpenRcService { path, .. } => {
                    assert_ne!(path, "/init");
                    assert!(!path.ends_with(concat!("/bootart-", "init")));
                }
                TemplateMaterialization::ManagedSnippet { target, .. } => {
                    assert_ne!(target, "/init");
                }
            }
            assert!(!embedded.contents.starts_with("\u{7f}ELF"));
            assert!(
                !embedded
                    .contents
                    .lines()
                    .any(|line| { line.trim_start().starts_with("exec /usr/bin/bootart") })
            );
            assert!(!embedded.contents.contains(concat!("bootart-", "init")));
        }
    }
}
