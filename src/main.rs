use std::fs;
use std::path::Path;
use std::process::exit;

use bootart::art::Art;
use bootart::cli::{parse_args, Command};
use bootart::renderer::{play_animation, render_final, RenderOptions};
use bootart::signals;
use bootart::terminal::StdoutTerminal;
use bootart::{DEFAULT_LOGO, SMALL_LOGO};

fn main() {
    signals::setup_signal_handlers();

    let command = match parse_args(std::env::args()) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("Error: {}", err);
            eprintln!("Run 'bootart --help' for usage information.");
            exit(1);
        }
    };

    match command {
        Command::Help => {
            print_help();
            exit(0);
        }
        Command::Version => {
            println!("bootart version 0.1.0");
            exit(0);
        }
        Command::Validate { asset } => {
            let content = match load_asset_str(asset.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Validation failed: {}", e);
                    exit(1);
                }
            };
            match Art::parse(&content) {
                Ok(art) => {
                    println!(
                        "Validation successful: {} columns x {} rows ({} visible lines)",
                        art.width,
                        art.height,
                        art.lines.len()
                    );
                    exit(0);
                }
                Err(err) => {
                    eprintln!("Validation failed: {}", err);
                    exit(1);
                }
            }
        }
        Command::Play {
            duration_ms,
            fps,
            seed,
            no_color,
            clear_first,
            leave_final,
            asset,
            cols,
            rows,
        } => {
            let (art, small_art) = match load_arts(asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(cols, rows);
            let options = RenderOptions {
                duration_ms,
                fps,
                seed,
                no_color,
                clear_first,
                leave_final,
            };
            if let Err(e) = play_animation(&mut term, &art, small_art.as_ref(), options, 0) {
                eprintln!("Render error: {}", e);
                exit(1);
            }
        }
        Command::RenderFinal {
            no_color,
            asset,
            cols,
            rows,
        } => {
            let (art, small_art) = match load_arts(asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(cols, rows);
            if let Err(e) = render_final(&mut term, &art, small_art.as_ref(), no_color) {
                eprintln!("Render error: {}", e);
                exit(1);
            }
        }
        Command::Preview {
            loop_infinitely,
            duration_ms,
            fps,
            seed,
            no_color,
            asset,
            cols,
            rows,
        } => {
            let (art, small_art) = match load_arts(asset.as_deref()) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Error loading logo asset: {}", e);
                    exit(1);
                }
            };
            let mut term = StdoutTerminal::with_override(cols, rows);
            let options = RenderOptions {
                duration_ms,
                fps,
                seed,
                no_color,
                clear_first: true,
                leave_final: true,
            };

            if loop_infinitely {
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

fn print_help() {
    println!(
        r#"bootart - Minimal Linux ASCII boot animation

USAGE:
    bootart <COMMAND> [OPTIONS]

COMMANDS:
    play            Run boot animation pass and exit
    render-final    Render final static logo frame without animation
    validate        Validate specified or embedded logo asset
    preview         Run animation preview (supports interactive looping)

PLAY / PREVIEW OPTIONS:
    --duration-ms <ms>  Animation duration in milliseconds (default: 2500)
    --fps <fps>         Target frames per second (default: 30)
    --seed <seed>       Deterministic integer seed (default: 42)
    --no-color          Disable ANSI color output
    --clear-first       Clear terminal screen before starting animation
    --leave-final       Keep final frame rendered after exit
    --asset <path>      Override embedded ASCII logo file
    --cols <cols>       Override detected terminal columns
    --rows <rows>       Override detected terminal rows
    --loop              Loop animation continuously (preview mode only)

FLAGS:
    -h, --help       Show this help message
    -v, --version    Show version information
"#
    );
}
