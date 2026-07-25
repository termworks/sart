use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Play {
        duration_ms: u64,
        fps: u64,
        seed: u64,
        no_color: bool,
        clear_first: bool,
        leave_final: bool,
        asset: Option<PathBuf>,
        cols: Option<usize>,
        rows: Option<usize>,
    },
    RenderFinal {
        no_color: bool,
        asset: Option<PathBuf>,
        cols: Option<usize>,
        rows: Option<usize>,
    },
    Validate {
        asset: Option<PathBuf>,
    },
    Preview {
        loop_infinitely: bool,
        duration_ms: u64,
        fps: u64,
        seed: u64,
        no_color: bool,
        asset: Option<PathBuf>,
        cols: Option<usize>,
        rows: Option<usize>,
    },
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    MissingArgument(String),
    InvalidArgument(String),
    UnknownSubcommand(String),
    UnknownOption(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::MissingArgument(opt) => write!(f, "missing argument for parameter '{}'", opt),
            CliError::InvalidArgument(reason) => write!(f, "invalid argument: {}", reason),
            CliError::UnknownSubcommand(sub) => write!(f, "unknown command '{}'", sub),
            CliError::UnknownOption(opt) => write!(f, "unknown option '{}'", opt),
        }
    }
}

impl std::error::Error for CliError {}

pub fn parse_args<I, T>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();

    if args.is_empty() {
        return Ok(Command::Play {
            duration_ms: 2500,
            fps: 30,
            seed: 42,
            no_color: false,
            clear_first: true,
            leave_final: true,
            asset: None,
            cols: None,
            rows: None,
        });
    }

    // Skip program name / binary path (e.g. "bootart", "/init", etc.)
    let mut i = 0;
    if !args[0].starts_with('-') {
        i = 1;
    }

    if i >= args.len() {
        return Ok(Command::Play {
            duration_ms: 2500,
            fps: 30,
            seed: 42,
            no_color: false,
            clear_first: true,
            leave_final: true,
            asset: None,
            cols: None,
            rows: None,
        });
    }

    let sub = &args[i];
    match sub.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "-v" | "--version" | "version" => Ok(Command::Version),
        "play" => parse_play(&args[i + 1..]),
        "render-final" => parse_render_final(&args[i + 1..]),
        "validate" => parse_validate(&args[i + 1..]),
        "preview" => parse_preview(&args[i + 1..]),
        other => {
            if other.starts_with('-') {
                Err(CliError::UnknownOption(other.to_string()))
            } else {
                Err(CliError::UnknownSubcommand(other.to_string()))
            }
        }
    }
}

fn parse_play(args: &[String]) -> Result<Command, CliError> {
    let mut duration_ms = 2500;
    let mut fps = 30;
    let mut seed = 42;
    let mut no_color = false;
    let mut clear_first = false;
    let mut leave_final = false;
    let mut asset = None;
    let mut cols = None;
    let mut rows = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--duration-ms" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--duration-ms".to_string()));
                }
                duration_ms = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --duration-ms".to_string())
                })?;
            }
            "--fps" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--fps".to_string()));
                }
                fps = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --fps".to_string())
                })?;
            }
            "--seed" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--seed".to_string()));
                }
                seed = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --seed".to_string())
                })?;
            }
            "--no-color" => no_color = true,
            "--clear-first" => clear_first = true,
            "--leave-final" => leave_final = true,
            "--asset" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--asset".to_string()));
                }
                asset = Some(PathBuf::from(&args[i]));
            }
            "--cols" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--cols".to_string()));
                }
                cols = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --cols".to_string())
                })?);
            }
            "--rows" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--rows".to_string()));
                }
                rows = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --rows".to_string())
                })?);
            }
            opt => return Err(CliError::UnknownOption(opt.to_string())),
        }
        i += 1;
    }

    Ok(Command::Play {
        duration_ms,
        fps,
        seed,
        no_color,
        clear_first,
        leave_final,
        asset,
        cols,
        rows,
    })
}

fn parse_render_final(args: &[String]) -> Result<Command, CliError> {
    let mut no_color = false;
    let mut asset = None;
    let mut cols = None;
    let mut rows = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-color" => no_color = true,
            "--asset" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--asset".to_string()));
                }
                asset = Some(PathBuf::from(&args[i]));
            }
            "--cols" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--cols".to_string()));
                }
                cols = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --cols".to_string())
                })?);
            }
            "--rows" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--rows".to_string()));
                }
                rows = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --rows".to_string())
                })?);
            }
            opt => return Err(CliError::UnknownOption(opt.to_string())),
        }
        i += 1;
    }

    Ok(Command::RenderFinal {
        no_color,
        asset,
        cols,
        rows,
    })
}

fn parse_validate(args: &[String]) -> Result<Command, CliError> {
    let mut asset = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--asset" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--asset".to_string()));
                }
                asset = Some(PathBuf::from(&args[i]));
            }
            opt => return Err(CliError::UnknownOption(opt.to_string())),
        }
        i += 1;
    }

    Ok(Command::Validate { asset })
}

fn parse_preview(args: &[String]) -> Result<Command, CliError> {
    let mut loop_infinitely = false;
    let mut duration_ms = 2500;
    let mut fps = 30;
    let mut seed = 42;
    let mut no_color = false;
    let mut asset = None;
    let mut cols = None;
    let mut rows = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--loop" => loop_infinitely = true,
            "--duration-ms" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--duration-ms".to_string()));
                }
                duration_ms = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --duration-ms".to_string())
                })?;
            }
            "--fps" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--fps".to_string()));
                }
                fps = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --fps".to_string())
                })?;
            }
            "--seed" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--seed".to_string()));
                }
                seed = args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --seed".to_string())
                })?;
            }
            "--no-color" => no_color = true,
            "--asset" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--asset".to_string()));
                }
                asset = Some(PathBuf::from(&args[i]));
            }
            "--cols" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--cols".to_string()));
                }
                cols = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --cols".to_string())
                })?);
            }
            "--rows" => {
                i += 1;
                if i >= args.len() {
                    return Err(CliError::MissingArgument("--rows".to_string()));
                }
                rows = Some(args[i].parse().map_err(|_| {
                    CliError::InvalidArgument("invalid integer for --rows".to_string())
                })?);
            }
            opt => return Err(CliError::UnknownOption(opt.to_string())),
        }
        i += 1;
    }

    Ok(Command::Preview {
        loop_infinitely,
        duration_ms,
        fps,
        seed,
        no_color,
        asset,
        cols,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_play_defaults() {
        let cmd = parse_args(vec!["bootart", "play"]).unwrap();
        match cmd {
            Command::Play {
                duration_ms,
                fps,
                seed,
                no_color,
                clear_first,
                leave_final,
                asset,
                ..
            } => {
                assert_eq!(duration_ms, 2500);
                assert_eq!(fps, 30);
                assert_eq!(seed, 42);
                assert!(!no_color);
                assert!(!clear_first);
                assert!(!leave_final);
                assert!(asset.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_flags() {
        let cmd = parse_args(vec![
            "bootart",
            "play",
            "--duration-ms",
            "1200",
            "--fps",
            "60",
            "--clear-first",
            "--leave-final",
            "--no-color",
        ])
        .unwrap();
        match cmd {
            Command::Play {
                duration_ms,
                fps,
                clear_first,
                leave_final,
                no_color,
                ..
            } => {
                assert_eq!(duration_ms, 1200);
                assert_eq!(fps, 60);
                assert!(clear_first);
                assert!(leave_final);
                assert!(no_color);
            }
            _ => panic!("Expected Play command"),
        }
    }
}
