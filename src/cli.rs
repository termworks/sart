use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(name = "bootart", version, about = "Minimal Linux ASCII boot animation", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Play animation once in terminal
    Play(PlayArgs),
    /// Interactive or infinite preview of animation
    Preview(PreviewArgs),
    /// Apply and register bootart to system initramfs
    Apply(ApplyArgs),
    /// Manage initramfs hooks (install, uninstall, status)
    Hook(HookArgs),
    /// Render final static state
    RenderFinal(RenderFinalArgs),
    /// Validate ASCII logo file
    Validate(ValidateArgs),
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
pub struct ApplyArgs {
    #[arg(long)]
    pub asset: Option<PathBuf>,
    #[arg(long, default_value_t = 2500)]
    pub duration_ms: u64,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct HookArgs {
    #[command(subcommand)]
    pub action: Option<HookAction>,
    #[arg(long)]
    pub asset: Option<PathBuf>,
    #[arg(long, default_value_t = 2500)]
    pub duration_ms: u64,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    /// Install bootart to system initramfs
    Install,
    /// Apply bootart to system initramfs (alias for install)
    Apply,
    /// Uninstall bootart from system initramfs
    Uninstall,
    /// Check initramfs hook status
    Status,
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
    fn test_parse_apply() {
        let cli = Cli::parse_from(["bootart", "apply"]);
        match cli.command {
            Some(Command::Apply(args)) => {
                assert_eq!(args.duration_ms, 2500);
                assert!(args.asset.is_none());
            }
            _ => panic!("Expected Apply command"),
        }
    }
}
