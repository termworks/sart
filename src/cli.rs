use crate::cmdline::PROC_CMDLINE;
use crate::display::text_vt::TextVtConfig;
use crate::splash::engine::{MAX_ANIMATION_CYCLE, MAX_FRAMES_PER_SECOND, MIN_ANIMATION_CYCLE};
use crate::splash::runtime::DEFAULT_RUNTIME_DIR;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(name = "bootart", version, about = "Minimal Linux ASCII boot animation", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Internal early-boot splash eligibility predicate
    #[command(name = "early-boot-enabled", hide = true)]
    EarlyBootEnabled(EarlyBootEnabledArgs),
    /// Run the foreground splash daemon
    Daemon(DaemonArgs),
    /// Show the splash view
    Show(RuntimeArgs),
    /// Hide the splash view
    Hide(RuntimeArgs),
    /// Set status text, or clear it when TEXT is omitted
    Status(OptionalTextArgs),
    /// Set boot progress from 0 through 100
    Progress(ProgressArgs),
    /// Show a message
    Message(TextArgs),
    /// Hide the current message, optionally only when it matches TEXT
    HideMessage(OptionalTextArgs),
    /// Control the detailed boot-output view
    Details(DetailsArgs),
    /// Temporarily deactivate splash presentation
    Deactivate(RuntimeArgs),
    /// Reactivate splash presentation
    Reactivate(RuntimeArgs),
    /// Change the presentation mode
    Mode(ModeArgs),
    /// Print daemon presentation state as JSON
    State(StateArgs),
    /// Ask the daemon to exit cleanly
    Quit(QuitArgs),
    /// Securely move the running daemon into an already-mounted real root
    UpdateRootFs(UpdateRootFsArgs),
    /// Check daemon connectivity and protocol compatibility
    Ping(RuntimeArgs),
    /// Inspect or request guarded installation operations for an alternate root
    Install(InstallArgs),
    /// Read-only inspection of host root (/) install plan
    HostPlan(HostPlanArgs),
    /// Request confirmed host root (/) installation; requires --confirm-host-apply
    HostApply(HostApplyArgs),
    /// Request confirmed host root (/) uninstallation; requires --confirm-host-uninstall
    HostUninstall(HostUninstallArgs),
    /// Internal same-ELF native broker capability probe
    #[command(name = "native-ready", hide = true)]
    NativeReady(RuntimeArgs),
    /// Internal same-ELF client for reviewed native initramfs pipe adapters
    #[command(name = "native-askpass", hide = true)]
    NativeAskpass(NativeAskpassArgs),
    /// Play animation once in terminal
    Play(PlayArgs),
    /// Interactive or infinite preview of animation
    Preview(PreviewArgs),
    /// Render final static state
    RenderFinal(RenderFinalArgs),
    /// Validate ASCII logo file
    Validate(ValidateArgs),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct EarlyBootEnabledArgs {
    /// Kernel command-line file; override only in an isolated test
    #[arg(long, hide = true, default_value = PROC_CMDLINE)]
    pub cmdline: PathBuf,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArgs {
    /// Runtime directory containing the daemon socket
    #[arg(long, global = true, default_value = DEFAULT_RUNTIME_DIR)]
    pub runtime_dir: PathBuf,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct StateArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// Select the stable machine-readable JSON representation
    #[arg(long, required = true)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct DaemonArgs {
    #[arg(long, value_enum, default_value_t = PresentationMode::Boot)]
    pub mode: PresentationMode,
    /// Experimental password-agent adapter; disabled unless explicitly selected
    #[arg(long, value_enum, default_value_t = PasswordBrokerSelection::None)]
    pub password_broker: PasswordBrokerSelection,
    #[arg(long, default_value = DEFAULT_RUNTIME_DIR)]
    pub runtime_dir: PathBuf,
    /// Kernel command-line file; override only in an isolated test
    #[arg(long, default_value = "/proc/cmdline")]
    pub cmdline: PathBuf,
    /// Dedicated Linux virtual terminal, for example /dev/tty7; omit to use VT_OPENQRY
    #[arg(long, value_name = "/dev/ttyN", value_parser = parse_linux_tty, conflicts_with = "test_buffer")]
    pub tty: Option<u16>,
    /// Persistent animation frame rate
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_FRAMES_PER_SECOND)))]
    pub fps: u16,
    /// Duration of one cyclic animation pass
    #[arg(
        long,
        default_value_t = 2500,
        value_parser = clap::value_parser!(u64).range(
            MIN_ANIMATION_CYCLE.as_millis() as u64..=MAX_ANIMATION_CYCLE.as_millis() as u64
        )
    )]
    pub cycle_ms: u64,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    #[arg(long)]
    pub no_color: bool,
    /// In-memory display for isolated subprocess tests; forbidden with /run/bootart
    #[arg(long, hide = true, conflicts_with = "tty")]
    pub test_buffer: bool,
}

fn parse_linux_tty(value: &str) -> Result<u16, String> {
    let digits = value
        .strip_prefix("/dev/tty")
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| "TTY must have the exact form /dev/ttyN".to_owned())?;
    let number = digits
        .parse::<u16>()
        .map_err(|_| "Linux VT number is out of range".to_owned())?;
    if format!("/dev/tty{number}") != value {
        return Err("TTY must use its canonical /dev/ttyN path".to_owned());
    }
    TextVtConfig::configured(number)
        .map(|config| match config.selection() {
            crate::display::text_vt::VtSelection::Configured(number) => number,
            crate::display::text_vt::VtSelection::OpenQuery => unreachable!(),
        })
        .map_err(|error| error.to_string())
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct OptionalTextArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    pub text: Option<String>,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct TextArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    pub text: String,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ProgressArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
    pub percent: u8,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct DetailsArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    #[command(subcommand)]
    pub action: DetailsAction,
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailsAction {
    Show,
    Hide,
    Toggle,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ModeArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    #[arg(value_enum)]
    pub mode: PresentationMode,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct QuitArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    #[arg(long)]
    pub retain_splash: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct UpdateRootFsArgs {
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    pub path: PathBuf,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMode {
    Boot,
    Shutdown,
    Reboot,
    Update,
    Upgrade,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordBrokerSelection {
    None,
    Systemd,
    Native,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallArgs {
    #[command(subcommand)]
    pub action: InstallAction,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum InstallAction {
    /// Render a deterministic, read-only, non-actionable installation plan
    Plan(InstallPlanArgs),
    /// Inspect an existing alternate-root manifest without changing it
    Status(InstallStatusArgs),
    /// Request installation; currently hard-locked before filesystem access
    Apply(InstallApplyArgs),
    /// Request interrupted-transaction recovery; currently hard-locked
    Recover(InstallMutationArgs),
    /// Request removal; currently hard-locked before filesystem access
    Uninstall(InstallMutationArgs),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct HostPlanArgs {
    /// Render the host install plan in stable machine-readable JSON format
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct HostApplyArgs {
    /// Explicit human confirmation flag to apply changes to host root (/)
    #[arg(long, default_value_t = false)]
    pub confirm_host_apply: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct HostUninstallArgs {
    /// Explicit human confirmation flag to uninstall bootart from host root (/)
    #[arg(long, default_value_t = false)]
    pub confirm_host_uninstall: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallSelectionArgs {
    /// Existing, root-owned disposable guest root; the running host root is forbidden
    #[arg(long)]
    pub root: PathBuf,
    /// Exact initramfs/runtime adapter to preview
    #[arg(long, value_enum)]
    pub initramfs_adapter: InitramfsAdapterSelection,
    /// Exact real-root supervisor adapter to preview
    #[arg(long, value_enum)]
    pub real_root_adapter: RealRootAdapterSelection,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallPlanArgs {
    #[command(flatten)]
    pub selection: InstallSelectionArgs,
    /// Emit the stable machine-readable plan instead of human text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallStatusArgs {
    /// Existing, root-owned disposable guest root; the running host root is forbidden
    #[arg(long)]
    pub root: PathBuf,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallApplyArgs {
    #[command(flatten)]
    pub selection: InstallSelectionArgs,
    /// Explicit hostname acknowledgement reserved for the future host-use gate
    #[arg(long, value_parser = parse_nonempty_confirmation)]
    pub confirm_host: String,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct InstallMutationArgs {
    /// Existing, root-owned disposable guest root; the running host root is forbidden
    #[arg(long)]
    pub root: PathBuf,
    /// Explicit hostname acknowledgement reserved for the future host-use gate
    #[arg(long, value_parser = parse_nonempty_confirmation)]
    pub confirm_host: String,
}

fn parse_nonempty_confirmation(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.len() > 255
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("confirmation must be a nonempty single hostname token".to_owned());
    }
    Ok(value.to_owned())
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitramfsAdapterSelection {
    #[value(name = "dracut-systemd")]
    DracutSystemd,
    #[value(name = "dracut-classic")]
    DracutClassic,
    #[value(name = "initramfs-tools-busybox")]
    InitramfsToolsBusybox,
    #[value(name = "mkinitcpio-busybox")]
    MkinitcpioBusybox,
    #[value(name = "mkinitfs-busybox")]
    MkinitfsBusybox,
}

impl From<InitramfsAdapterSelection> for crate::integration::AdapterId {
    fn from(value: InitramfsAdapterSelection) -> Self {
        match value {
            InitramfsAdapterSelection::DracutSystemd => Self::DracutSystemd,
            InitramfsAdapterSelection::DracutClassic => Self::DracutClassic,
            InitramfsAdapterSelection::InitramfsToolsBusybox => Self::InitramfsToolsBusybox,
            InitramfsAdapterSelection::MkinitcpioBusybox => Self::MkinitcpioBusybox,
            InitramfsAdapterSelection::MkinitfsBusybox => Self::MkinitfsBusybox,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealRootAdapterSelection {
    #[value(name = "systemd")]
    Systemd,
    #[value(name = "openrc")]
    OpenRc,
}

impl From<RealRootAdapterSelection> for crate::integration::AdapterId {
    fn from(value: RealRootAdapterSelection) -> Self {
        match value {
            RealRootAdapterSelection::Systemd => Self::SystemdRealRoot,
            RealRootAdapterSelection::OpenRc => Self::OpenRcRealRoot,
        }
    }
}

/// Non-secret metadata for the hidden native askpass client. The secret output
/// is always fixed inherited fd 8; no output path, command, or secret-bearing
/// argument exists.
#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct NativeAskpassArgs {
    /// Exact native initramfs contract selecting request identity and framing
    #[arg(long, value_enum)]
    pub adapter: NativeAskpassAdapterSelection,
    #[arg(long)]
    pub prompt: String,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=64))]
    pub attempts: u16,
    #[arg(long, default_value_t = 1024, value_parser = clap::value_parser!(u16).range(1..=4096))]
    pub maximum_secret_bytes: u16,
}

/// Native password transports are exact adapter contracts, not a generic
/// non-systemd mode. Only adapters with a reviewed inherited-pipe integration
/// are accepted by the hidden client.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeAskpassAdapterSelection {
    #[value(name = "dracut-classic")]
    DracutClassic,
    #[value(name = "initramfs-tools-busybox")]
    InitramfsToolsBusybox,
}

impl From<NativeAskpassAdapterSelection> for crate::password::NativeAdapter {
    fn from(value: NativeAskpassAdapterSelection) -> Self {
        match value {
            NativeAskpassAdapterSelection::DracutClassic => Self::DracutClassic,
            NativeAskpassAdapterSelection::InitramfsToolsBusybox => Self::InitramfsToolsBusybox,
        }
    }
}

impl From<PresentationMode> for crate::splash::state::Mode {
    fn from(value: PresentationMode) -> Self {
        match value {
            PresentationMode::Boot => Self::Boot,
            PresentationMode::Shutdown => Self::Shutdown,
            PresentationMode::Reboot => Self::Reboot,
            PresentationMode::Update => Self::Update,
            PresentationMode::Upgrade => Self::Upgrade,
        }
    }
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct PlayArgs {
    #[arg(long, default_value_t = 2500)]
    pub duration_ms: u64,
    #[arg(long, default_value_t = 30)]
    pub fps: u64,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    #[arg(long)]
    pub no_color: bool,
    #[arg(long)]
    pub clear_first: bool,
    #[arg(long)]
    pub leave_final: bool,
    #[arg(long)]
    pub asset: Option<PathBuf>,
    #[arg(long)]
    pub cols: Option<usize>,
    #[arg(long)]
    pub rows: Option<usize>,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct PreviewArgs {
    #[arg(long)]
    pub loop_infinitely: bool,
    #[arg(long, default_value_t = 2500)]
    pub duration_ms: u64,
    #[arg(long, default_value_t = 30)]
    pub fps: u64,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    #[arg(long)]
    pub no_color: bool,
    #[arg(long)]
    pub asset: Option<PathBuf>,
    #[arg(long)]
    pub cols: Option<usize>,
    #[arg(long)]
    pub rows: Option<usize>,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct RenderFinalArgs {
    #[arg(long)]
    pub no_color: bool,
    #[arg(long)]
    pub asset: Option<PathBuf>,
    #[arg(long)]
    pub cols: Option<usize>,
    #[arg(long)]
    pub rows: Option<usize>,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ValidateArgs {
    #[arg(long)]
    pub asset: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_play_defaults() {
        let cli = Cli::parse_from(["bootart", "play"]);
        match cli.command {
            Some(Command::Play(args)) => {
                assert_eq!(args.duration_ms, 2500);
                assert_eq!(args.fps, 30);
                assert_eq!(args.seed, 42);
                assert!(!args.no_color);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn parses_daemon_and_control_runtime() {
        let daemon = Cli::parse_from([
            "bootart",
            "daemon",
            "--mode",
            "update",
            "--runtime-dir",
            "/tmp/bootart-test",
        ]);
        assert!(matches!(
            daemon.command,
            Some(Command::Daemon(DaemonArgs {
                mode: PresentationMode::Update,
                password_broker: PasswordBrokerSelection::None,
                ..
            }))
        ));

        let broker = Cli::parse_from(["bootart", "daemon", "--password-broker", "systemd"]);
        assert!(matches!(
            broker.command,
            Some(Command::Daemon(DaemonArgs {
                password_broker: PasswordBrokerSelection::Systemd,
                ..
            }))
        ));

        let native = Cli::parse_from(["bootart", "daemon", "--password-broker", "native"]);
        assert!(matches!(
            native.command,
            Some(Command::Daemon(DaemonArgs {
                password_broker: PasswordBrokerSelection::Native,
                ..
            }))
        ));

        let progress = Cli::parse_from([
            "bootart",
            "progress",
            "--runtime-dir",
            "/tmp/bootart-test",
            "42",
        ]);
        assert!(matches!(
            progress.command,
            Some(Command::Progress(ProgressArgs { percent: 42, .. }))
        ));

        let state = Cli::parse_from([
            "bootart",
            "state",
            "--json",
            "--runtime-dir",
            "/tmp/bootart-test",
        ]);
        assert!(matches!(
            state.command,
            Some(Command::State(StateArgs {
                json: true,
                runtime: RuntimeArgs { runtime_dir },
            })) if runtime_dir.as_path() == std::path::Path::new("/tmp/bootart-test")
        ));
        assert!(Cli::try_parse_from(["bootart", "state"]).is_err());
    }

    #[test]
    fn hidden_native_client_has_only_non_secret_metadata_and_fixed_fd() {
        let readiness = Cli::parse_from([
            "bootart",
            "native-ready",
            "--runtime-dir",
            "/tmp/bootart-test",
        ]);
        assert!(matches!(
            readiness.command,
            Some(Command::NativeReady(RuntimeArgs { runtime_dir }))
                if runtime_dir.as_path() == std::path::Path::new("/tmp/bootart-test")
        ));

        let parsed = Cli::parse_from([
            "bootart",
            "native-askpass",
            "--adapter",
            "dracut-classic",
            "--prompt",
            "Password (/dev/vda2)",
            "--attempts",
            "5",
        ]);
        assert!(matches!(
            parsed.command,
            Some(Command::NativeAskpass(NativeAskpassArgs {
                adapter: NativeAskpassAdapterSelection::DracutClassic,
                attempts: 5,
                maximum_secret_bytes: 1024,
                ..
            }))
        ));

        let initramfs_tools = Cli::parse_from([
            "bootart",
            "native-askpass",
            "--adapter",
            "initramfs-tools-busybox",
            "--prompt",
            "Please unlock disk cryptroot: ",
        ]);
        assert!(matches!(
            initramfs_tools.command,
            Some(Command::NativeAskpass(NativeAskpassArgs {
                adapter: NativeAskpassAdapterSelection::InitramfsToolsBusybox,
                attempts: 1,
                ..
            }))
        ));
        assert!(
            Cli::try_parse_from(["bootart", "native-askpass", "--prompt", "Password"]).is_err()
        );
    }

    #[test]
    fn hidden_early_boot_predicate_has_only_a_testable_cmdline_input() {
        let default = Cli::parse_from(["bootart", "early-boot-enabled"]);
        assert!(matches!(
            default.command,
            Some(Command::EarlyBootEnabled(EarlyBootEnabledArgs { cmdline }))
                if cmdline.as_path() == std::path::Path::new(PROC_CMDLINE)
        ));

        let overridden = Cli::parse_from([
            "bootart",
            "early-boot-enabled",
            "--cmdline",
            "/tmp/bootart-test-cmdline",
        ]);
        assert!(matches!(
            overridden.command,
            Some(Command::EarlyBootEnabled(EarlyBootEnabledArgs { cmdline }))
                if cmdline.as_path() == std::path::Path::new("/tmp/bootart-test-cmdline")
        ));
    }

    #[test]
    fn installer_surface_requires_an_exact_pair_and_confirmation() {
        let plan = Cli::parse_from([
            "bootart",
            "install",
            "plan",
            "--root",
            "/guest",
            "--initramfs-adapter",
            "dracut-classic",
            "--real-root-adapter",
            "openrc",
            "--json",
        ]);
        assert!(matches!(
            plan.command,
            Some(Command::Install(InstallArgs {
                action: InstallAction::Plan(InstallPlanArgs {
                    selection: InstallSelectionArgs {
                        initramfs_adapter: InitramfsAdapterSelection::DracutClassic,
                        real_root_adapter: RealRootAdapterSelection::OpenRc,
                        ..
                    },
                    json: true,
                })
            }))
        ));

        assert!(
            Cli::try_parse_from([
                "bootart",
                "install",
                "plan",
                "--root",
                "/guest",
                "--initramfs-adapter",
                "dracut-systemd",
                "--real-root-adapter",
                "systemd",
                "--bootart-elf",
                "/tmp/substitute",
            ])
            .is_err(),
            "production planning must not accept an alternate executable payload"
        );

        assert!(
            Cli::try_parse_from([
                "bootart",
                "install",
                "apply",
                "--root",
                "/guest",
                "--initramfs-adapter",
                "dracut-systemd",
                "--real-root-adapter",
                "systemd",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "bootart",
                "install",
                "uninstall",
                "--root",
                "/guest",
                "--confirm-host",
                "bad confirmation",
            ])
            .is_err()
        );
    }

    #[test]
    fn host_commands_parse_explicit_flags() {
        let plan = Cli::try_parse_from(["bootart", "host-plan", "--json"]).unwrap();
        assert!(matches!(
            plan.command,
            Some(Command::HostPlan(HostPlanArgs { json: true }))
        ));

        let apply = Cli::try_parse_from(["bootart", "host-apply", "--confirm-host-apply"]).unwrap();
        assert!(matches!(
            apply.command,
            Some(Command::HostApply(HostApplyArgs {
                confirm_host_apply: true
            }))
        ));

        let uninstall =
            Cli::try_parse_from(["bootart", "host-uninstall", "--confirm-host-uninstall"]).unwrap();
        assert!(matches!(
            uninstall.command,
            Some(Command::HostUninstall(HostUninstallArgs {
                confirm_host_uninstall: true
            }))
        ));
    }

    #[test]
    fn daemon_tty_path_is_strict_and_canonical() {
        let parsed = Cli::try_parse_from(["bootart", "daemon", "--tty", "/dev/tty7"])
            .expect("canonical Linux VT path should parse");
        assert!(matches!(
            parsed.command,
            Some(Command::Daemon(DaemonArgs { tty: Some(7), .. }))
        ));

        for invalid in [
            "tty7",
            "/dev/console",
            "/dev/tty0",
            "/dev/tty07",
            "/dev/tty64",
        ] {
            assert!(
                Cli::try_parse_from(["bootart", "daemon", "--tty", invalid]).is_err(),
                "{invalid} must be rejected"
            );
        }
    }
}
