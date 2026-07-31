use bootart::art::{Art, MAX_ART_BYTES};
use bootart::cli::{
    Cli, Command, DetailsAction, InstallAction, InstallPlanArgs, PasswordBrokerSelection,
    RuntimeArgs,
};
#[cfg(feature = "installer-test-seams")]
use bootart::cli::{InstallApplyArgs, InstallMutationArgs};
use bootart::display::Dimensions;
use bootart::display::text_vt::TextVtConfig;
use bootart::install::AdapterDiscovery;
#[cfg(feature = "installer-test-seams")]
use bootart::install::NoAdapterDiscovery;
#[cfg(feature = "installer-test-seams")]
use bootart::install::running_bootart_elf_for_vm_tests;
use bootart::install::{
    AdapterRequest, AdapterSelection, ApplyOutcome, CommandRunner, DracutSystemdContract,
    FaultInjector, FileStatusState, ImageVerificationStatus, InitramfsToolsSystemdContract,
    Installer, ManifestInventoryStatus, MetadataSource, MkinitcpioSystemdContract,
    MkinitfsBootDeployOpenRcContract, MkinitfsOpenRcContract, RecoveryOutcome, StatusReport,
    SupportPolicy, build_self_install_plan, plan_dracut_systemd, plan_initramfs_tools_systemd,
    plan_mkinitcpio_systemd, plan_mkinitfs_boot_deploy_openrc, plan_mkinitfs_openrc,
};
use bootart::integration::AdapterKind;
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
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::exit;

const UDEV_CONTROL_SOCKET: &str = "/run/udev/control";
const VT_CONTROL_DEVICE: &str = "/dev/tty0";

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
        Command::EarlyBootEnabled(args) => {
            // Runtime hooks need a silent same-ELF decision before acquiring a
            // VT or intercepting a password path. Unreadable cmdline state is
            // a nonzero result so the stock boot path remains untouched.
            exit(if bootart::cmdline::early_boot_enabled_at(&args.cmdline) {
                0
            } else {
                1
            });
        }
        Command::ConsoleFallbackNeeded(args) => {
            // systemd runs this as ExecCondition after ordering the daemon
            // start job. Exit 1 only after an authenticated Ping/Pong so the
            // stock agent is skipped; every timeout or protocol failure exits
            // 0 and therefore fails open to the distro console agent.
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(args.wait_ms);
            let runtime = RuntimePaths::new(args.runtime.runtime_dir);
            loop {
                let request = Frame::empty(Opcode::Ping, next_request_id());
                let healthy = request.ok().is_some_and(|request| {
                    let mut config = ClientConfig::for_runtime(runtime.clone());
                    config.timeout = std::time::Duration::from_millis(100);
                    send_request(&config, &request)
                        .ok()
                        .is_some_and(|response| response.opcode() == Opcode::Pong)
                });
                if healthy {
                    exit(1);
                }
                if std::time::Instant::now() >= deadline {
                    exit(0);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Command::VtReady(args) => {
            // Do not touch a VT while initramfs device management may still
            // recreate its nodes. The wait is bounded and observes only fixed
            // kernel/udev paths; failure leaves the stock console agent in
            // control rather than racing it from a late Bootart start.
            exit(if wait_for_vt_readiness(args.wait_ms) {
                0
            } else {
                1
            });
        }
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
            // Delivery, cancellation, and transport failure have distinct
            // exit codes. Bounded setup/transport diagnostics may go to
            // stderr, but prompt and credential bytes never do; every path
            // closes inherited fd 8 without putting a secret in stdout,
            // stderr, argv, or the environment.
            let adapter: bootart::password::NativeAdapter = args.adapter.into();
            let framing = adapter.secret_framing();
            let outcome = match PipeAskpassMetadata::new(
                args.prompt,
                args.attempts,
                usize::from(args.maximum_secret_bytes),
                framing,
            ) {
                Ok(metadata) => match claim_native_askpass_output() {
                    Ok(output) => run_native_askpass_client(adapter, &metadata, output),
                    Err(error) => {
                        eprintln!(
                            "bootart native askpass unavailable: claim inherited output: {error}"
                        );
                        NativeAskpassClientOutcome::ConsoleFallback
                    }
                },
                Err(error) => {
                    eprintln!("bootart native askpass unavailable: invalid metadata: {error}");
                    NativeAskpassClientOutcome::ConsoleFallback
                }
            };
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

fn wait_for_vt_readiness(wait_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        if fixed_vt_paths_are_ready() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn fixed_vt_paths_are_ready() -> bool {
    std::fs::symlink_metadata(VT_CONTROL_DEVICE)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_char_device())
        && std::fs::symlink_metadata(UDEV_CONTROL_SOCKET)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_socket())
}

fn run_install(action: InstallAction) {
    let result = match action {
        InstallAction::Plan(args) => render_install_plan(args),
        InstallAction::Status(args) => run_install_status(args),
        InstallAction::Apply(args) => run_install_apply(args),
        InstallAction::Recover(args) => run_install_recover(args),
        InstallAction::Uninstall(args) => run_install_uninstall(args),
    };

    match result {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("Installer error: {error}");
            exit(1);
        }
    }
}

#[cfg(not(feature = "installer-test-seams"))]
fn render_install_plan(args: InstallPlanArgs) -> Result<String, String> {
    let installer =
        Installer::production_live_root_read_only().map_err(|error| error.to_string())?;
    let contract = exact_install_contract(&installer)?;
    let selection = exact_install_selection(installer.root(), &contract)?;
    match &contract {
        ExactInstallContract::Dracut(contract) => {
            render_exact_live_plan(&installer, selection, contract, args.json)
        }
        ExactInstallContract::InitramfsTools(contract) => {
            render_exact_initramfs_tools_live_plan(&installer, selection, contract, args.json)
        }
        ExactInstallContract::Mkinitcpio(contract) => {
            render_exact_mkinitcpio_live_plan(&installer, selection, contract, args.json)
        }
        ExactInstallContract::MkinitfsOpenRc(contract) => {
            render_exact_mkinitfs_openrc_live_plan(&installer, selection, contract, args.json)
        }
        ExactInstallContract::MkinitfsBootDeployOpenRc(contract) => {
            render_exact_mkinitfs_boot_deploy_openrc_live_plan(
                &installer, selection, contract, args.json,
            )
        }
    }
}

#[cfg(feature = "installer-test-seams")]
fn render_install_plan(args: InstallPlanArgs) -> Result<String, String> {
    if args.selection.root != Path::new("/") {
        let installer =
            Installer::production(&args.selection.root).map_err(|error| error.to_string())?;
        let selection = AdapterSelection::resolve(
            installer.root(),
            AdapterRequest::Explicit(args.selection.initramfs_adapter.into()),
            AdapterRequest::Explicit(args.selection.real_root_adapter.into()),
            SupportPolicy::AllowExplicitExperimental,
            &NoAdapterDiscovery,
        )
        .map_err(|error| error.to_string())?;
        let plan = build_self_install_plan(installer.root(), selection)
            .and_then(|plan| installer.preflight_fresh_install_plan(plan))
            .map_err(|error| error.to_string())?;
        return Ok(if args.json {
            format!("{}\n", plan.render_machine_json())
        } else {
            plan.render_human()
        });
    }
    let installer =
        Installer::production_live_root_read_only().map_err(|error| error.to_string())?;
    let initramfs = args.selection.initramfs_adapter.into();
    let selection = AdapterSelection::resolve(
        installer.root(),
        AdapterRequest::Explicit(initramfs),
        AdapterRequest::Explicit(args.selection.real_root_adapter.into()),
        SupportPolicy::AllowExplicitExperimental,
        &NoAdapterDiscovery,
    )
    .map_err(|error| error.to_string())?;
    match initramfs {
        bootart::integration::AdapterId::DracutSystemd => {
            let contract = exact_dracut_systemd_contract(&installer)?;
            render_exact_live_plan(&installer, selection, &contract, args.json)
        }
        bootart::integration::AdapterId::InitramfsToolsBusybox => {
            let contract = exact_initramfs_tools_systemd_contract(&installer)?;
            render_exact_initramfs_tools_live_plan(&installer, selection, &contract, args.json)
        }
        bootart::integration::AdapterId::MkinitcpioBusybox => {
            let contract = exact_mkinitcpio_systemd_contract(&installer)?;
            render_exact_mkinitcpio_live_plan(&installer, selection, &contract, args.json)
        }
        bootart::integration::AdapterId::MkinitfsBusybox => {
            let contract = exact_mkinitfs_openrc_contract(&installer)?;
            render_exact_mkinitfs_openrc_live_plan(&installer, selection, &contract, args.json)
        }
        bootart::integration::AdapterId::MkinitfsBootDeploy => {
            let contract = exact_mkinitfs_boot_deploy_openrc_contract(&installer)?;
            render_exact_mkinitfs_boot_deploy_openrc_live_plan(
                &installer, selection, &contract, args.json,
            )
        }
        _ => Err("VM-test live planning has no image transaction for this adapter yet".into()),
    }
}

fn render_exact_live_plan(
    installer: &Installer,
    selection: AdapterSelection,
    contract: &DracutSystemdContract,
    json: bool,
) -> Result<String, String> {
    let plan = build_self_install_plan(installer.root(), selection)
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .map_err(|error| error.to_string())?;
    Ok(if json {
        format!(
            "{}\n",
            plan.render_dracut_systemd_production_json(contract)
                .map_err(|error| error.to_string())?
        )
    } else {
        plan.render_dracut_systemd_production_human(contract)
            .map_err(|error| error.to_string())?
    })
}

fn render_exact_initramfs_tools_live_plan(
    installer: &Installer,
    selection: AdapterSelection,
    contract: &InitramfsToolsSystemdContract,
    json: bool,
) -> Result<String, String> {
    let plan = build_self_install_plan(installer.root(), selection)
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .map_err(|error| error.to_string())?;
    Ok(if json {
        format!(
            "{}\n",
            plan.render_initramfs_tools_systemd_json(contract)
                .map_err(|error| error.to_string())?
        )
    } else {
        plan.render_initramfs_tools_systemd_human(contract)
            .map_err(|error| error.to_string())?
    })
}

fn render_exact_mkinitfs_openrc_live_plan(
    installer: &Installer,
    selection: AdapterSelection,
    contract: &MkinitfsOpenRcContract,
    json: bool,
) -> Result<String, String> {
    let plan = build_self_install_plan(installer.root(), selection)
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .map_err(|error| error.to_string())?;
    Ok(if json {
        format!(
            "{}\n",
            plan.render_mkinitfs_openrc_json(contract)
                .map_err(|error| error.to_string())?
        )
    } else {
        plan.render_mkinitfs_openrc_human(contract)
            .map_err(|error| error.to_string())?
    })
}

fn render_exact_mkinitcpio_live_plan(
    installer: &Installer,
    selection: AdapterSelection,
    contract: &MkinitcpioSystemdContract,
    json: bool,
) -> Result<String, String> {
    let plan = build_self_install_plan(installer.root(), selection)
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .map_err(|error| error.to_string())?;
    Ok(if json {
        format!(
            "{}\n",
            plan.render_mkinitcpio_systemd_json(contract)
                .map_err(|error| error.to_string())?
        )
    } else {
        plan.render_mkinitcpio_systemd_human(contract)
            .map_err(|error| error.to_string())?
    })
}

fn render_exact_mkinitfs_boot_deploy_openrc_live_plan(
    installer: &Installer,
    selection: AdapterSelection,
    contract: &MkinitfsBootDeployOpenRcContract,
    json: bool,
) -> Result<String, String> {
    let plan = build_self_install_plan(installer.root(), selection)
        .and_then(|plan| installer.preflight_fresh_install_plan(plan))
        .map_err(|error| error.to_string())?;
    Ok(if json {
        format!(
            "{}\n",
            plan.render_mkinitfs_boot_deploy_openrc_json(contract)
                .map_err(|error| error.to_string())?
        )
    } else {
        plan.render_mkinitfs_boot_deploy_openrc_human(contract)
            .map_err(|error| error.to_string())?
    })
}

#[cfg(not(feature = "installer-test-seams"))]
fn run_install_status(_args: bootart::cli::InstallStatusArgs) -> Result<String, String> {
    let status = Installer::production_live_root_read_only()
        .and_then(|installer| installer.status())
        .map_err(|error| error.to_string())?;
    Ok(render_install_status(Path::new("/"), status))
}

#[cfg(feature = "installer-test-seams")]
fn run_install_status(args: bootart::cli::InstallStatusArgs) -> Result<String, String> {
    let status = if args.root == Path::new("/") {
        Installer::production_live_root_read_only().and_then(|installer| installer.status())
    } else {
        Installer::production(&args.root).and_then(|installer| installer.status())
    }
    .map_err(|error| error.to_string())?;
    Ok(render_install_status(&args.root, status))
}

fn render_install_status(root: &Path, status: StatusReport) -> String {
    let mut output = format!(
        "bootart install status\nroot: {}\ninstalled: {}\n",
        root.display(),
        status.installed
    );
    match status.provenance {
        Some(provenance) => output.push_str(&format!(
            "provenance: installed-plan-version={} current-plan-version={} installed-resource-set-version={} current-resource-set-version={} version-current={}\n",
            provenance.installed_plan_version,
            provenance.current_plan_version,
            provenance.installed_resource_set_version,
            provenance.current_resource_set_version,
            provenance.is_version_current(),
        )),
        None => output.push_str("provenance: not-installed\n"),
    }
    output.push_str(match status.inventory {
        ManifestInventoryStatus::NotInstalled => "inventory: not-installed\n",
        ManifestInventoryStatus::Complete => "inventory: complete\n",
        ManifestInventoryStatus::Partial => "inventory: partial\n",
    });
    match status.image_verification {
        ImageVerificationStatus::NotInstalled => {
            output.push_str("image-verification: not-installed\n")
        }
        ImageVerificationStatus::Unresolved { blocker } => output.push_str(&format!(
            "image-verification: unresolved blocker={blocker}\n"
        )),
        ImageVerificationStatus::Verified {
            active_digest,
            known_good_digest,
            bootart_digest,
        } => output.push_str(&format!(
            "image-verification: verified active-sha256={active_digest} known-good-sha256={known_good_digest} bootart-sha256={bootart_digest}\n"
        )),
        ImageVerificationStatus::Modified { paths } => output.push_str(&format!(
            "image-verification: modified paths={}\n",
            paths.join(",")
        )),
    }
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
            FileStatusState::SymlinkTargetModified { actual } => {
                format!("symlink-target-modified actual-target={actual}")
            }
            FileStatusState::TypeModified { actual_kind } => {
                format!("type-modified actual-kind={actual_kind:?}")
            }
        };
        output.push_str(&format!(
            "  {} expected-mode={:04o} expected-sha256={} state={}\n",
            file.path, file.expected_mode, file.expected_digest, state
        ));
    }
    output
}

enum ExactInstallContract {
    Dracut(DracutSystemdContract),
    InitramfsTools(InitramfsToolsSystemdContract),
    Mkinitcpio(MkinitcpioSystemdContract),
    MkinitfsOpenRc(MkinitfsOpenRcContract),
    MkinitfsBootDeployOpenRc(MkinitfsBootDeployOpenRcContract),
}

impl ExactInstallContract {
    fn adapter(&self) -> bootart::integration::AdapterId {
        match self {
            Self::Dracut(_) => bootart::integration::AdapterId::DracutSystemd,
            Self::InitramfsTools(_) => bootart::integration::AdapterId::InitramfsToolsBusybox,
            Self::Mkinitcpio(_) => bootart::integration::AdapterId::MkinitcpioBusybox,
            Self::MkinitfsOpenRc(_) => bootart::integration::AdapterId::MkinitfsBusybox,
            Self::MkinitfsBootDeployOpenRc(_) => {
                bootart::integration::AdapterId::MkinitfsBootDeploy
            }
        }
    }

    fn real_root_adapter(&self) -> bootart::integration::AdapterId {
        match self {
            Self::Dracut(_) | Self::InitramfsTools(_) | Self::Mkinitcpio(_) => {
                bootart::integration::AdapterId::SystemdRealRoot
            }
            Self::MkinitfsOpenRc(_) | Self::MkinitfsBootDeployOpenRc(_) => {
                bootart::integration::AdapterId::OpenRcRealRoot
            }
        }
    }

    fn roots_match(&self, root: &bootart::install::AlternateRoot) -> bool {
        match self {
            Self::Dracut(contract) => {
                root.as_path() == contract.generate.alternate_root
                    && root.as_path() == contract.update_grub.alternate_root
            }
            Self::InitramfsTools(contract) => {
                root.as_path() == contract.generate.alternate_root
                    && root.as_path() == contract.update_grub.alternate_root
            }
            Self::Mkinitcpio(contract) => {
                root.as_path() == contract.generate.alternate_root
                    && root.as_path() == contract.update_grub.alternate_root
            }
            Self::MkinitfsOpenRc(contract) => {
                root.as_path() == contract.generate.alternate_root
                    && root.as_path() == contract.update_extlinux.alternate_root
            }
            Self::MkinitfsBootDeployOpenRc(contract) => {
                root.as_path() == contract.generate.alternate_root
            }
        }
    }
}

struct VerifiedInstallDiscovery<'a> {
    contract: &'a ExactInstallContract,
}

impl AdapterDiscovery for VerifiedInstallDiscovery<'_> {
    fn candidates(
        &self,
        root: &bootart::install::AlternateRoot,
        kind: AdapterKind,
    ) -> Result<Vec<bootart::integration::AdapterId>, String> {
        if !self.contract.roots_match(root) {
            return Err("verified initramfs contract belongs to another root".into());
        }
        Ok(vec![match kind {
            AdapterKind::InitramfsRuntime => self.contract.adapter(),
            AdapterKind::RealRootSupervisor => self.contract.real_root_adapter(),
        }])
    }
}

fn exact_dracut_systemd_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<DracutSystemdContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let facts = installer
        .collect_dracut_systemd_facts()
        .map_err(|error| error.to_string())?;
    plan_dracut_systemd(&facts).map_err(|error| error.to_string())
}

fn exact_initramfs_tools_systemd_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<InitramfsToolsSystemdContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let facts = installer
        .collect_initramfs_tools_systemd_facts()
        .map_err(|error| error.to_string())?;
    plan_initramfs_tools_systemd(&facts).map_err(|error| error.to_string())
}

fn exact_mkinitcpio_systemd_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<MkinitcpioSystemdContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let facts = installer
        .collect_mkinitcpio_systemd_facts()
        .map_err(|error| error.to_string())?;
    plan_mkinitcpio_systemd(&facts).map_err(|error| error.to_string())
}

fn exact_mkinitfs_openrc_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<MkinitfsOpenRcContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let facts = installer
        .collect_mkinitfs_openrc_facts()
        .map_err(|error| error.to_string())?;
    plan_mkinitfs_openrc(&facts).map_err(|error| error.to_string())
}

fn exact_mkinitfs_boot_deploy_openrc_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<MkinitfsBootDeployOpenRcContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let facts = installer
        .collect_mkinitfs_boot_deploy_openrc_facts()
        .map_err(|error| error.to_string())?;
    plan_mkinitfs_boot_deploy_openrc(&facts).map_err(|error| error.to_string())
}

fn exact_install_contract<M, C, F>(
    installer: &Installer<M, C, F>,
) -> Result<ExactInstallContract, String>
where
    M: MetadataSource,
    C: CommandRunner,
    F: FaultInjector,
{
    let dracut = exact_dracut_systemd_contract(installer);
    let initramfs_tools = exact_initramfs_tools_systemd_contract(installer);
    let mkinitcpio = exact_mkinitcpio_systemd_contract(installer);
    let mkinitfs_openrc = exact_mkinitfs_openrc_contract(installer);
    let mkinitfs_boot_deploy_openrc = exact_mkinitfs_boot_deploy_openrc_contract(installer);
    let complete = usize::from(dracut.is_ok())
        + usize::from(initramfs_tools.is_ok())
        + usize::from(mkinitcpio.is_ok())
        + usize::from(mkinitfs_openrc.is_ok())
        + usize::from(mkinitfs_boot_deploy_openrc.is_ok());
    if complete > 1 {
        return Err(
            "multiple complete initramfs capability contracts were detected; refusing an ambiguous mutation"
                .into(),
        );
    }
    if complete == 0 {
        return Err(format!(
            "no complete initramfs capability contract was detected; dracut-systemd: {}; initramfs-tools-systemd: {}; mkinitcpio-systemd: {}; mkinitfs-openrc: {}; mkinitfs-boot-deploy-openrc: {}",
            dracut.as_ref().unwrap_err(),
            initramfs_tools.as_ref().unwrap_err(),
            mkinitcpio.as_ref().unwrap_err(),
            mkinitfs_openrc.as_ref().unwrap_err(),
            mkinitfs_boot_deploy_openrc.as_ref().unwrap_err(),
        ));
    }
    if let Ok(contract) = dracut {
        Ok(ExactInstallContract::Dracut(contract))
    } else if let Ok(contract) = initramfs_tools {
        Ok(ExactInstallContract::InitramfsTools(contract))
    } else if let Ok(contract) = mkinitcpio {
        Ok(ExactInstallContract::Mkinitcpio(contract))
    } else if let Ok(contract) = mkinitfs_openrc {
        Ok(ExactInstallContract::MkinitfsOpenRc(contract))
    } else if let Ok(contract) = mkinitfs_boot_deploy_openrc {
        Ok(ExactInstallContract::MkinitfsBootDeployOpenRc(contract))
    } else {
        unreachable!("exactly one complete contract was counted")
    }
}

fn exact_install_selection(
    root: &bootart::install::AlternateRoot,
    contract: &ExactInstallContract,
) -> Result<AdapterSelection, String> {
    AdapterSelection::resolve(
        root,
        AdapterRequest::Discover,
        AdapterRequest::Discover,
        SupportPolicy::ProvenOnly,
        &VerifiedInstallDiscovery { contract },
    )
    .map_err(|error| error.to_string())
}

fn render_apply_outcome(outcome: ApplyOutcome) -> String {
    match outcome {
        ApplyOutcome::Installed => "bootart install apply: installed\n".into(),
        ApplyOutcome::AlreadyCurrent => "bootart install apply: already-current\n".into(),
    }
}

fn render_recovery_outcome(outcome: RecoveryOutcome) -> String {
    match outcome {
        RecoveryOutcome::NothingToRecover => "bootart install recover: nothing-to-recover\n",
        RecoveryOutcome::RolledBack => "bootart install recover: rolled-back\n",
        RecoveryOutcome::RolledBackWithPreservedDirectories => {
            "bootart install recover: rolled-back-with-preserved-directories\n"
        }
        RecoveryOutcome::CompletedCommitCleaned => {
            "bootart install recover: completed-commit-cleaned\n"
        }
    }
    .into()
}

fn render_uninstall_report(report: bootart::install::UninstallReport) -> String {
    format!(
        "bootart install uninstall: removed={} restored={} preserved-modified={} preserved-directories={}\n",
        report.removed.len(),
        report.restored.len(),
        report.preserved_modified.len(),
        report.preserved_directories.len(),
    )
}

#[cfg(not(feature = "installer-test-seams"))]
fn run_install_apply(args: bootart::cli::InstallApplyArgs) -> Result<String, String> {
    let mut installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    let contract = exact_install_contract(&installer)?;
    let selection = exact_install_selection(installer.root(), &contract)?;
    let already_installed = installer
        .status()
        .map_err(|error| error.to_string())?
        .installed;
    let plan =
        build_self_install_plan(installer.root(), selection).map_err(|error| error.to_string())?;
    let plan = if already_installed {
        plan
    } else {
        installer
            .preflight_fresh_install_plan(plan)
            .map_err(|error| error.to_string())?
    };
    match &contract {
        ExactInstallContract::Dracut(contract) => installer.apply_dracut_systemd(&plan, contract),
        ExactInstallContract::InitramfsTools(contract) => {
            installer.apply_initramfs_tools_systemd(&plan, contract)
        }
        ExactInstallContract::Mkinitcpio(contract) => {
            installer.apply_mkinitcpio_systemd(&plan, contract)
        }
        ExactInstallContract::MkinitfsOpenRc(contract) => {
            installer.apply_mkinitfs_openrc(&plan, contract)
        }
        ExactInstallContract::MkinitfsBootDeployOpenRc(contract) => {
            installer.apply_mkinitfs_boot_deploy_openrc(&plan, contract)
        }
    }
    .map(render_apply_outcome)
    .map_err(|error| error.to_string())
}

#[cfg(not(feature = "installer-test-seams"))]
fn run_install_recover(args: bootart::cli::InstallMutationArgs) -> Result<String, String> {
    let installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    installer
        .recover()
        .map(render_recovery_outcome)
        .map_err(|error| error.to_string())
}

#[cfg(not(feature = "installer-test-seams"))]
fn run_install_uninstall(args: bootart::cli::InstallMutationArgs) -> Result<String, String> {
    let mut installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    let contract = exact_install_contract(&installer)?;
    exact_install_selection(installer.root(), &contract)?;
    let report = match contract {
        ExactInstallContract::Dracut(_) => installer.uninstall_dracut_systemd(),
        ExactInstallContract::InitramfsTools(_)
        | ExactInstallContract::Mkinitcpio(_)
        | ExactInstallContract::MkinitfsOpenRc(_)
        | ExactInstallContract::MkinitfsBootDeployOpenRc(_) => installer.uninstall(),
    };
    report
        .map(render_uninstall_report)
        .map_err(|error| error.to_string())
}

#[cfg(feature = "installer-test-seams")]
fn run_install_apply(args: InstallApplyArgs) -> Result<String, String> {
    if args.selection.root != Path::new("/") {
        return Err("VM-test mutation requires the exact live root".into());
    }
    let mut installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    let initramfs = args.selection.initramfs_adapter.into();
    let selection = AdapterSelection::resolve(
        installer.root(),
        AdapterRequest::Explicit(initramfs),
        AdapterRequest::Explicit(args.selection.real_root_adapter.into()),
        SupportPolicy::AllowExplicitExperimental,
        &NoAdapterDiscovery,
    )
    .map_err(|error| error.to_string())?;
    let implemented_pair = matches!(
        (selection.initramfs(), selection.real_root()),
        (
            bootart::integration::AdapterId::DracutSystemd
                | bootart::integration::AdapterId::InitramfsToolsBusybox
                | bootart::integration::AdapterId::MkinitcpioBusybox,
            bootart::integration::AdapterId::SystemdRealRoot,
        ) | (
            bootart::integration::AdapterId::MkinitfsBusybox
                | bootart::integration::AdapterId::MkinitfsBootDeploy,
            bootart::integration::AdapterId::OpenRcRealRoot,
        )
    );
    if !implemented_pair {
        return Err("VM-test mutation permits only implemented initramfs transactions".into());
    }
    let already_installed = installer
        .status()
        .map_err(|error| error.to_string())?
        .installed;
    let plan =
        build_self_install_plan(installer.root(), selection).map_err(|error| error.to_string())?;
    let plan = if already_installed {
        plan
    } else {
        installer
            .preflight_fresh_install_plan(plan)
            .map_err(|error| error.to_string())?
    };
    let bootart = running_bootart_elf_for_vm_tests().map_err(|error| error.to_string())?;
    let outcome = match initramfs {
        bootart::integration::AdapterId::DracutSystemd => {
            let contract = exact_dracut_systemd_contract(&installer)?;
            match args.interrupt_at_checkpoint {
                Some(checkpoint) => installer.apply_dracut_systemd_interrupted_for_tests(
                    &plan, &contract, &bootart, checkpoint,
                ),
                None => installer.apply_dracut_systemd_for_tests(&plan, &contract, &bootart),
            }
        }
        bootart::integration::AdapterId::InitramfsToolsBusybox => {
            let contract = exact_initramfs_tools_systemd_contract(&installer)?;
            match args.interrupt_at_checkpoint {
                Some(checkpoint) => installer.apply_initramfs_tools_systemd_interrupted_for_tests(
                    &plan, &contract, &bootart, checkpoint,
                ),
                None => {
                    installer.apply_initramfs_tools_systemd_for_tests(&plan, &contract, &bootart)
                }
            }
        }
        bootart::integration::AdapterId::MkinitcpioBusybox => {
            let contract = exact_mkinitcpio_systemd_contract(&installer)?;
            match args.interrupt_at_checkpoint {
                Some(checkpoint) => installer.apply_mkinitcpio_systemd_interrupted_for_tests(
                    &plan, &contract, &bootart, checkpoint,
                ),
                None => installer.apply_mkinitcpio_systemd_for_tests(&plan, &contract, &bootart),
            }
        }
        bootart::integration::AdapterId::MkinitfsBusybox => {
            let contract = exact_mkinitfs_openrc_contract(&installer)?;
            match args.interrupt_at_checkpoint {
                Some(checkpoint) => installer.apply_mkinitfs_openrc_interrupted_for_tests(
                    &plan, &contract, &bootart, checkpoint,
                ),
                None => installer.apply_mkinitfs_openrc_for_tests(&plan, &contract, &bootart),
            }
        }
        bootart::integration::AdapterId::MkinitfsBootDeploy => {
            let contract = exact_mkinitfs_boot_deploy_openrc_contract(&installer)?;
            match args.interrupt_at_checkpoint {
                Some(checkpoint) => installer
                    .apply_mkinitfs_boot_deploy_openrc_interrupted_for_tests(
                        &plan, &contract, &bootart, checkpoint,
                    ),
                None => installer
                    .apply_mkinitfs_boot_deploy_openrc_for_tests(&plan, &contract, &bootart),
            }
        }
        _ => unreachable!("selection was bounded above"),
    }
    .map_err(|error| error.to_string())?;
    Ok(render_apply_outcome(outcome))
}

#[cfg(feature = "installer-test-seams")]
fn run_install_recover(args: InstallMutationArgs) -> Result<String, String> {
    if args.root != Path::new("/") {
        return Err("VM-test mutation requires the exact live root".into());
    }
    let installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    installer
        .recover()
        .map(render_recovery_outcome)
        .map_err(|error| error.to_string())
}

#[cfg(feature = "installer-test-seams")]
fn run_install_uninstall(args: InstallMutationArgs) -> Result<String, String> {
    if args.root != Path::new("/") {
        return Err("VM-test mutation requires the exact live root".into());
    }
    let mut installer = Installer::production_live_root_mutating(&args.confirm_host)
        .map_err(|error| error.to_string())?;
    let contract = exact_install_contract(&installer)?;
    exact_install_selection(installer.root(), &contract)?;
    let report = match contract {
        ExactInstallContract::Dracut(_) => installer.uninstall_dracut_systemd_for_tests(),
        ExactInstallContract::InitramfsTools(_)
        | ExactInstallContract::Mkinitcpio(_)
        | ExactInstallContract::MkinitfsOpenRc(_)
        | ExactInstallContract::MkinitfsBootDeployOpenRc(_) => installer.uninstall(),
    };
    report
        .map(render_uninstall_report)
        .map_err(|error| error.to_string())
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
