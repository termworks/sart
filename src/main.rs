use bootart::art::{Art, MAX_ART_BYTES};
use bootart::cli::{
    Cli, Command, DetailsAction, InstallAction, InstallPlanArgs, PasswordBrokerSelection,
    RuntimeArgs,
};
use bootart::display::Dimensions;
use bootart::display::text_vt::TextVtConfig;
use bootart::install::{
    AdapterRequest, AdapterSelection, AlternateRoot, FileStatusState, InstallError, Installer,
    MAX_INSTALL_FILE_BYTES, NoAdapterDiscovery, SupportPolicy, build_install_plan,
};
use bootart::password::{
    NATIVE_ASKPASS_CANCELLED_EXIT_CODE, NATIVE_ASKPASS_TRANSPORT_EXIT_CODE,
    NativeAskpassClientOutcome, PipeAskpassMetadata, claim_native_askpass_output,
    run_native_askpass_client,
};
use bootart::process::{PID1_REFUSAL_EXIT_CODE, run_after_pid1_guard};
use bootart::renderer::{RenderOptions, play_animation, render_final};
use bootart::splash::client::{ClientConfig, next_request_id, send_request};
use bootart::splash::daemon::{
    DISPLAY_RESTORATION_FAILED_EXIT_CODE, DaemonConfig, PasswordBroker, run as run_daemon,
    run_with_test_buffer,
};
use bootart::splash::engine::EngineConfig;
use bootart::splash::protocol::{Frame, Opcode};
use bootart::splash::runtime::RuntimePaths;
use bootart::terminal::StdoutTerminal;
use bootart::{DEFAULT_LOGO, SMALL_LOGO, signals};
use clap::Parser;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::exit;

fn main() {
    // bootart owns presentation state only. It must never replace the real init
    // system, even in a test image. The continuation cannot be invoked until
    // the pure PID guard passes, so argument parsing and all I/O remain behind
    // this boundary.
    if let Err(error) = run_after_pid1_guard(std::process::id(), run_bootart) {
        eprintln!("{error}");
        exit(PID1_REFUSAL_EXIT_CODE);
    }
}

fn run_bootart() {
    let cli = Cli::parse();

    let command = cli.command.unwrap_or({
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
        Command::Daemon(args) => {
            let config = DaemonConfig {
                runtime: RuntimePaths::new(args.runtime_dir),
                mode: args.mode.into(),
                password_broker: match args.password_broker {
                    PasswordBrokerSelection::None => PasswordBroker::None,
                    PasswordBrokerSelection::Systemd => PasswordBroker::Systemd,
                    PasswordBrokerSelection::Native => PasswordBroker::Native,
                },
                cmdline_path: args.cmdline,
                display: match args.tty {
                    Some(number) => TextVtConfig::configured(number)
                        .expect("CLI validation only accepts Linux VT numbers"),
                    None => TextVtConfig::open_query(),
                },
                engine: EngineConfig {
                    frames_per_second: args.fps,
                    animation_cycle: std::time::Duration::from_millis(args.cycle_ms),
                    seed: args.seed,
                    no_color: args.no_color,
                },
                ..DaemonConfig::default()
            };
            let result = if args.test_buffer {
                run_with_test_buffer(
                    &config,
                    Dimensions::new(80, 24).expect("fixed test dimensions are valid"),
                )
            } else {
                run_daemon(&config)
            };
            if let Err(error) = result {
                eprintln!("Daemon error: {error}");
                exit(if error.display_restoration_failed() {
                    DISPLAY_RESTORATION_FAILED_EXIT_CODE
                } else {
                    1
                });
            }
        }
        Command::Show(runtime) => {
            run_control(runtime, Frame::empty(Opcode::Show, next_request_id()))
        }
        Command::Hide(runtime) => {
            run_control(runtime, Frame::empty(Opcode::Hide, next_request_id()))
        }
        Command::Status(args) => run_control(
            args.runtime,
            Frame::text(
                Opcode::Status,
                next_request_id(),
                args.text.unwrap_or_default(),
            ),
        ),
        Command::Progress(args) => run_control(
            args.runtime,
            Frame::progress(next_request_id(), args.percent),
        ),
        Command::Message(args) => run_control(
            args.runtime,
            Frame::text(Opcode::Message, next_request_id(), args.text),
        ),
        Command::HideMessage(args) => run_control(
            args.runtime,
            Frame::text(
                Opcode::HideMessage,
                next_request_id(),
                args.text.unwrap_or_default(),
            ),
        ),
        Command::Details(args) => {
            let opcode = match args.action {
                DetailsAction::Show => Opcode::DetailsShow,
                DetailsAction::Hide => Opcode::DetailsHide,
                DetailsAction::Toggle => Opcode::DetailsToggle,
            };
            run_control(args.runtime, Frame::empty(opcode, next_request_id()));
        }
        Command::Deactivate(runtime) => {
            run_control(runtime, Frame::empty(Opcode::Deactivate, next_request_id()))
        }
        Command::Reactivate(runtime) => {
            run_control(runtime, Frame::empty(Opcode::Reactivate, next_request_id()))
        }
        Command::Mode(args) => run_control(
            args.runtime,
            Frame::mode(next_request_id(), args.mode.into()),
        ),
        Command::State(args) => {
            debug_assert!(args.json, "clap requires the JSON state selector");
            run_control(args.runtime, Frame::empty(Opcode::State, next_request_id()))
        }
        Command::Quit(args) => run_control(
            args.runtime,
            Frame::quit(next_request_id(), args.retain_splash),
        ),
        Command::UpdateRootFs(args) => {
            let Some(path) = args.path.to_str() else {
                eprintln!("Root path must be valid UTF-8");
                exit(2);
            };
            run_control(
                args.runtime,
                Frame::text(Opcode::UpdateRootFs, next_request_id(), path),
            );
        }
        Command::Ping(runtime) => {
            run_control(runtime, Frame::empty(Opcode::Ping, next_request_id()))
        }
        Command::Install(args) => run_install(args.action),
        Command::NativeReady(runtime) => {
            // The classic-initramfs adapter needs a silent, bounded capability
            // check. A generic ping is insufficient because the daemon can be
            // alive while its native listener or coordinator is unavailable.
            let ready = Frame::empty(Opcode::NativeReady, next_request_id())
                .ok()
                .and_then(|request| {
                    let config = ClientConfig::for_runtime(RuntimePaths::new(runtime.runtime_dir));
                    send_request(&config, &request).ok()
                })
                .is_some_and(|response| response.opcode() == Opcode::Ack);
            exit(if ready {
                0
            } else {
                NATIVE_ASKPASS_TRANSPORT_EXIT_CODE
            });
        }
        Command::NativeAskpass(args) => {
            // This internal client is deliberately silent. Delivery,
            // cancellation, and transport failure have distinct exit codes;
            // every path closes inherited fd 8 without putting a secret in
            // stdout, stderr, argv, or the environment.
            let adapter: bootart::password::NativeAdapter = args.adapter.into();
            let framing = adapter.secret_framing();
            let outcome = PipeAskpassMetadata::new(
                args.prompt,
                args.attempts,
                usize::from(args.maximum_secret_bytes),
                framing,
            )
            .ok()
            .and_then(|metadata| {
                claim_native_askpass_output()
                    .ok()
                    .map(|output| run_native_askpass_client(adapter, &metadata, output))
            })
            .unwrap_or(NativeAskpassClientOutcome::ConsoleFallback);
            match outcome {
                NativeAskpassClientOutcome::Delivered => exit(0),
                NativeAskpassClientOutcome::UserCancelled => {
                    exit(NATIVE_ASKPASS_CANCELLED_EXIT_CODE)
                }
                NativeAskpassClientOutcome::ConsoleFallback => {
                    exit(NATIVE_ASKPASS_TRANSPORT_EXIT_CODE)
                }
            }
        }
        Command::Play(args) => {
            if let Err(error) = (|| -> Result<(), String> {
                // Keep both guards inside this fallible scope. Returning the
                // error drops the terminal before main uses process::exit,
                // so an output failure cannot bypass cursor restoration.
                let _signal_guard = install_signal_handlers()?;
                let (art, small_art) = load_arts(args.asset.as_deref())?;
                let mut term = StdoutTerminal::with_override(args.cols, args.rows);
                let options = RenderOptions {
                    duration_ms: args.duration_ms,
                    fps: args.fps,
                    seed: args.seed,
                    no_color: args.no_color,
                    clear_first: args.clear_first,
                    leave_final: args.leave_final,
                };
                play_animation(&mut term, &art, small_art.as_ref(), options, 0)
                    .map_err(|error| format!("render error: {error}"))
            })() {
                eprintln!("Playback failed: {error}");
                exit(1);
            }
        }
        Command::RenderFinal(args) => {
            if let Err(error) = (|| -> Result<(), String> {
                let _signal_guard = install_signal_handlers()?;
                let (art, small_art) = load_arts(args.asset.as_deref())?;
                let mut term = StdoutTerminal::with_override(args.cols, args.rows);
                render_final(&mut term, &art, small_art.as_ref(), args.no_color)
                    .map_err(|error| format!("render error: {error}"))
            })() {
                eprintln!("Final render failed: {error}");
                exit(1);
            }
        }
        Command::Preview(args) => {
            if let Err(error) = (|| -> Result<(), String> {
                let _signal_guard = install_signal_handlers()?;
                let (art, small_art) = load_arts(args.asset.as_deref())?;
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
                        play_animation(&mut term, &art, small_art.as_ref(), options, iteration)
                            .map_err(|error| format!("render error: {error}"))?;
                        iteration = iteration.wrapping_add(1);
                    }
                    Ok(())
                } else {
                    play_animation(&mut term, &art, small_art.as_ref(), options, 0)
                        .map_err(|error| format!("render error: {error}"))
                }
            })() {
                eprintln!("Preview failed: {error}");
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
                    println!(
                        "Logo asset is valid! Dimensions: {}x{}",
                        art.width, art.height
                    );
                }
                Err(err) => {
                    eprintln!("Validation failed: {}", err);
                    exit(1);
                }
            }
        }
    }
}

fn run_install(action: InstallAction) {
    let result = match action {
        InstallAction::Plan(args) => render_install_plan(args),
        InstallAction::Status(args) => (|| {
            let installer = Installer::production(&args.root).map_err(|error| error.to_string())?;
            let status = installer.status().map_err(|error| error.to_string())?;
            let mut output = format!(
                "bootart install status\nroot: {}\ninstalled: {}\n",
                args.root.display(),
                status.installed
            );
            for file in status.files {
                let state = match file.state {
                    FileStatusState::Exact => "exact".to_owned(),
                    FileStatusState::Missing => "missing".to_owned(),
                    FileStatusState::ContentModified { actual } => {
                        format!("content-modified actual-sha256={actual}")
                    }
                    FileStatusState::ModeModified { actual } => {
                        format!("mode-modified actual-mode={actual:04o}")
                    }
                    FileStatusState::ContentAndModeModified {
                        actual_digest,
                        actual_mode,
                    } => format!(
                        "content-and-mode-modified actual-sha256={actual_digest} actual-mode={actual_mode:04o}"
                    ),
                };
                output.push_str(&format!(
                    "  {} expected-mode={:04o} expected-sha256={} state={}\n",
                    file.path, file.expected_mode, file.expected_digest, state
                ));
            }
            Ok(output)
        })(),
        // These variants are intentionally rejected before inspecting their
        // root, confirmation token, adapter, or ELF paths. The CLI exists so
        // the safety boundary is testable without exposing a hidden mutation
        // route in the default/release binary.
        InstallAction::Apply(_) | InstallAction::Recover(_) | InstallAction::Uninstall(_) => {
            Err(InstallError::MutationLocked.to_string())
        }
    };

    match result {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("Installer error: {error}");
            exit(1);
        }
    }
}

fn render_install_plan(args: InstallPlanArgs) -> Result<String, String> {
    let root =
        AlternateRoot::production(&args.selection.root).map_err(|error| error.to_string())?;
    let selection = AdapterSelection::resolve(
        &root,
        AdapterRequest::Explicit(args.selection.initramfs_adapter.into()),
        AdapterRequest::Explicit(args.selection.real_root_adapter.into()),
        SupportPolicy::AllowExplicitExperimental,
        &NoAdapterDiscovery,
    )
    .map_err(|error| error.to_string())?;
    let elf = read_bounded_file(
        &args.selection.bootart_elf,
        MAX_INSTALL_FILE_BYTES,
        "bootart ELF",
    )?;
    let plan = build_install_plan(&root, selection, &elf).map_err(|error| error.to_string())?;
    Ok(if args.json {
        format!("{}\n", plan.render_machine_json())
    } else {
        plan.render_human()
    })
}

fn read_bounded_file(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {label} {path:?}: {error}"))?;
    let initial_capacity = usize::try_from(maximum.min(64 * 1024)).unwrap_or(64 * 1024);
    let mut bytes = Vec::with_capacity(initial_capacity);
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label} {path:?}: {error}"))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "{label} {path:?} exceeds the {maximum}-byte safety limit"
        ));
    }
    Ok(bytes)
}

fn run_control(
    runtime: RuntimeArgs,
    request: Result<Frame, bootart::splash::protocol::ProtocolError>,
) {
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            eprintln!("Invalid control request: {error}");
            exit(2);
        }
    };
    let config = ClientConfig::for_runtime(RuntimePaths::new(runtime.runtime_dir));
    let response = match send_request(&config, &request) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("Control error: {error}");
            exit(1);
        }
    };

    match response.opcode() {
        Opcode::Ack => {}
        Opcode::Pong => println!("pong"),
        Opcode::StateResult => match response.payload_text() {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("Invalid state response: {error}");
                exit(1);
            }
        },
        Opcode::Error => {
            let message = response
                .payload_text()
                .unwrap_or("daemon rejected the request");
            eprintln!("Daemon rejected request: {message}");
            exit(1);
        }
        opcode => {
            eprintln!("Unexpected daemon response: {opcode:?}");
            exit(1);
        }
    }
}

fn install_signal_handlers() -> Result<signals::SignalGuard, String> {
    signals::reset_stop_flag();
    signals::setup_signal_handlers()
        .map_err(|error| format!("cannot install terminal-restoration signal handlers: {error}"))
}

fn load_asset_str(path: Option<&Path>) -> Result<String, String> {
    match path {
        Some(path) => {
            let file = File::open(path)
                .map_err(|error| format!("failed to open asset {path:?}: {error}"))?;
            let mut bytes = Vec::with_capacity(MAX_ART_BYTES.min(64 * 1024));
            file.take((MAX_ART_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("failed to read asset {path:?}: {error}"))?;
            if bytes.len() > MAX_ART_BYTES {
                return Err(format!(
                    "asset {path:?} exceeds the {MAX_ART_BYTES}-byte safety limit"
                ));
            }
            String::from_utf8(bytes)
                .map_err(|error| format!("asset {path:?} is not valid UTF-8: {error}"))
        }
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
