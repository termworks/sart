use bootart::art::Art;
use bootart::cli::{Cli, Command, HookAction};
use bootart::renderer::{play_animation, render_final, RenderOptions};
use bootart::terminal::StdoutTerminal;
use bootart::{signals, DEFAULT_LOGO, SMALL_LOGO};
use clap::Parser;
use std::fs;
use std::path::Path;
use std::process::exit;

fn main() {
    let cli = Cli::parse();

    let command = cli.command.unwrap_or_else(|| {
        bootart::cli::Command::Play(bootart::cli::PlayArgs {
            duration_ms: 2500,
            fps: 30,
            seed: 42,
            no_color: false,
            clear_first: true,
            leave_final: true,
            asset: None,
            cols: None,
            rows: None,
        })
    });

    match command {
        Command::Apply(args) => {
            if let Err(e) = bootart::hook::install_hooks(args.asset.as_deref()) {
                eprintln!("Error applying hooks: {}", e);
                exit(1);
            }
        }
        Command::Hook(args) => match args.action.unwrap_or(HookAction::Status) {
            HookAction::Install | HookAction::Apply => {
                if let Err(e) = bootart::hook::install_hooks(args.asset.as_deref()) {
                    eprintln!("Error installing hooks: {}", e);
                    exit(1);
                }
            }
            HookAction::Uninstall => {
                if let Err(e) = bootart::hook::uninstall_hooks() {
                    eprintln!("Error uninstalling hooks: {}", e);
                    exit(1);
                }
            }
            HookAction::Status => {
                bootart::hook::status_hooks();
            }
        },
        Command::Play(args) => {
            let (art, small_art) = match load_arts(args.asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(args.cols, args.rows);
            let options = RenderOptions {
                duration_ms: args.duration_ms,
                fps: args.fps,
                seed: args.seed,
                no_color: args.no_color,
                clear_first: args.clear_first,
                leave_final: args.leave_final,
            };
            if let Err(e) = play_animation(&mut term, &art, small_art.as_ref(), options, 0) {
                eprintln!("Render error: {}", e);
                exit(1);
            }
        }
        Command::RenderFinal(args) => {
            let (art, small_art) = match load_arts(args.asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(args.cols, args.rows);
            if let Err(e) = render_final(&mut term, &art, small_art.as_ref(), args.no_color) {
                eprintln!("Render error: {}", e);
                exit(1);
            }
        }
        Command::Preview(args) => {
            let (art, small_art) = match load_arts(args.asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(args.cols, args.rows);
            let options = RenderOptions {
                duration_ms: args.duration_ms,
                fps: args.fps,
                seed: args.seed,
                no_color: args.no_color,
                clear_first: true,
                leave_final: true,
            };

            if args.loop_infinitely {
                let mut iteration = 0;
                while !signals::should_stop() {
                    if let Err(e) = play_animation(&mut term, &art, small_art.as_ref(), options, iteration) {
                        eprintln!("Render error: {}", e);
                        exit(1);
                    }
                    iteration += 1;
                }
            } else if let Err(e) = play_animation(&mut term, &art, small_art.as_ref(), options, 0) {
                eprintln!("Render error: {}", e);
                exit(1);
            }
        }
        Command::Validate(args) => {
            let path = args.asset.as_deref();
            let content = match load_asset_str(path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Validation error: {}", e);
                    exit(1);
                }
            };
            match Art::parse(&content) {
                Ok(art) => {
                    println!("Logo asset is valid! Dimensions: {}x{}", art.width, art.height);
                }
                Err(err) => {
                    eprintln!("Validation failed: {}", err);
                    exit(1);
                }
            }
        }
    }

    if std::process::id() == 1 {
        unsafe {
            libc::reboot(libc::RB_POWER_OFF);
            libc::reboot(libc::RB_HALT_SYSTEM);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}

fn load_asset_str(path: Option<&Path>) -> Result<String, String> {
    match path {
        Some(p) => fs::read_to_string(p).map_err(|e| format!("failed to read asset {:?}: {}", p, e)),
        None => Ok(DEFAULT_LOGO.to_string()),
    }
}

fn load_arts(path: Option<&Path>) -> Result<(Art, Option<Art>), String> {
    let main_str = load_asset_str(path)?;
    let art = Art::parse(&main_str).map_err(|e| format!("invalid logo art: {}", e))?;

    let small_art = if path.is_none() {
        Art::parse(SMALL_LOGO).ok()
    } else {
        None
    };

    Ok((art, small_art))
}
