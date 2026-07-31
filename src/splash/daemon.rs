use super::command::{CommandOutcome, handle_request_with_root_transition, is_mutating};
use super::engine::{Clock, EngineConfig, EngineError, SplashEngine, SystemClock};
use super::protocol::{Frame, Opcode, ProtocolError};
use super::root_transition::{
    DeferredRootTransition, LinuxSelfRootTransition, RootTransition, RootTransitionError,
};
use super::runtime::{RuntimeError, RuntimeOwner, RuntimePaths, peer_credentials};
use super::state::{Mode, SplashState, StateAction};
use crate::art::{Art, ValidationError};
use crate::display::buffer::BufferBackend;
use crate::display::text_vt::{TextVtBackend, TextVtConfig};
use crate::display::{Dimensions, DisplayBackend};
use crate::password::{
    LinuxProcessSecretPolicy, NativePromptCoordinator, ProcessSecretPolicy, PromptCoordinator,
    SystemdPromptCoordinator,
};
use crate::process::{Pid1Refused, ensure_current_process_not_pid1};
use crate::{DEFAULT_LOGO, SMALL_LOGO, cmdline, signals};
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_ACCEPT_INTERVAL: Duration = Duration::from_millis(10);
pub const DEFAULT_MAX_CONNECTIONS: usize = 16;
/// Reserved for an initramfs supervisor that must distinguish an ordinary,
/// fully restored daemon failure from ambiguous display ownership.
pub const DISPLAY_RESTORATION_FAILED_EXIT_CODE: i32 = 77;
const REQUEST_QUEUE_CAPACITY: usize = 32;
const MAX_ACCEPTS_PER_TICK: usize = 16;
const MAX_REQUESTS_PER_TICK: usize = 32;
const SYSTEMD_REBIND_INTERVAL: Duration = Duration::from_millis(250);
// The initramfs systemd can finish switch-root well before real-root systemd
// has recreated its absolute ask-password namespace. Keep animating and
// accepting the already-open control socket during that bounded gap; the
// normal real-root quit unit usually wins first. A genuinely stalled handoff
// still restores the VT after this deadline.
const SYSTEMD_REBIND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub runtime: RuntimePaths,
    pub mode: Mode,
    pub password_broker: PasswordBroker,
    pub cmdline_path: PathBuf,
    pub connection_timeout: Duration,
    pub accept_interval: Duration,
    pub max_connections: usize,
    pub display: TextVtConfig,
    pub engine: EngineConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            runtime: RuntimePaths::production(),
            mode: Mode::Boot,
            password_broker: PasswordBroker::None,
            cmdline_path: PathBuf::from(cmdline::PROC_CMDLINE),
            connection_timeout: DEFAULT_CONNECTION_TIMEOUT,
            accept_interval: DEFAULT_ACCEPT_INTERVAL,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            display: TextVtConfig::open_query(),
            engine: EngineConfig::default(),
        }
    }
}

/// Explicit password-agent selection.  The default is init-neutral and does
/// not inspect or assume the host init system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasswordBroker {
    #[default]
    None,
    /// Systemd password-agent adapter. End-to-end support is owned by the
    /// exact initramfs/real-root pair table, not this component selector.
    Systemd,
    /// Experimental init-neutral native adapter over a separate authenticated
    /// SOCK_SEQPACKET carrier. Exact adapters remain VM-unproven.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonOutcome {
    Disabled,
    Stopped,
}

/// Run the production foreground daemon with the Linux text-VT backend.
///
/// There is deliberately no fallback to stdout or the in-memory backend when
/// VT acquisition fails. Failure restores what was acquired and returns so the
/// real init system can continue booting.
pub fn run(config: &DaemonConfig) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
    let mut root_transition = LinuxSelfRootTransition::new();
    run_with_root_transition(config, &mut root_transition)
}

pub fn run_with_root_transition(
    config: &DaemonConfig,
    root_transition: &mut dyn RootTransition,
) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
    let backend = TextVtBackend::new(config.display);
    run_with_backend(config, backend, root_transition)
}

/// Explicit, hidden test lane used by subprocess protocol tests.
///
/// It is rejected for `/run/bootart`, so production can never silently select
/// an in-memory display because `/dev/tty0` is unavailable.
#[doc(hidden)]
pub fn run_with_test_buffer(
    config: &DaemonConfig,
    dimensions: Dimensions,
) -> Result<DaemonOutcome, DaemonError> {
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
    if config.runtime.is_production() {
        return Err(DaemonError::TestBackendOnProductionRuntime);
    }
    let mut root_transition = DeferredRootTransition;
    run_with_backend(config, BufferBackend::new(dimensions), &mut root_transition)
}

fn run_with_backend<B: DisplayBackend>(
    config: &DaemonConfig,
    backend: B,
    root_transition: &mut dyn RootTransition,
) -> Result<DaemonOutcome, DaemonError> {
    // Keep the library entry point as strict as the binary entry point. This
    // check precedes command-line reads, signals, display, sockets, and files.
    ensure_current_process_not_pid1().map_err(DaemonError::Pid1)?;
    let emit_lifecycle_events = config.runtime.is_production();
    if emit_lifecycle_events {
        lifecycle_event("daemon-enter");
    }

    if cmdline::splash_disabled_at(&config.cmdline_path).map_err(|source| DaemonError::Cmdline {
        path: config.cmdline_path.clone(),
        source,
    })? {
        return Ok(DaemonOutcome::Disabled);
    }
    if config.max_connections == 0 {
        return Err(DaemonError::InvalidMaxConnections);
    }

    // This process-wide gate must precede display/input acquisition and
    // password-agent watcher construction. Failure aborts the presentation
    // before it can hide the distro's console recovery agent.
    let password_broker =
        prepare_password_broker(config.password_broker, &LinuxProcessSecretPolicy)?;

    let art = Art::parse(DEFAULT_LOGO).map_err(DaemonError::EmbeddedArt)?;
    let small_art = Art::parse(SMALL_LOGO).map_err(DaemonError::EmbeddedArt)?;

    signals::reset_stop_flag();
    // The daemon owns these dispositions for its entire presentation
    // boundary. Dropping this guard during normal/error unwinding restores
    // whatever handlers its initramfs supervisor installed beforehand.
    let _signal_guard = signals::setup_signal_handlers().map_err(DaemonError::Signals)?;

    let mut runtime =
        RuntimeOwner::acquire(config.runtime.clone()).map_err(DaemonError::Runtime)?;
    let listener = runtime.bind_listener().map_err(DaemonError::Runtime)?;
    listener
        .set_nonblocking(true)
        .map_err(DaemonError::ConfigureListener)?;
    let native_password_listener = if password_broker == PasswordBroker::Native {
        match runtime.bind_native_password_listener() {
            Ok(listener) => Some(listener),
            Err(_) => {
                eprintln!(
                    "bootart native password broker is unavailable; refusing splash display acquisition"
                );
                None
            }
        }
    } else {
        None
    };

    let mut prompt_coordinator: Option<Box<dyn PromptCoordinator>> = match password_broker {
        PasswordBroker::None => None,
        PasswordBroker::Systemd => match SystemdPromptCoordinator::open_system() {
            Ok(coordinator) => Some(Box::new(coordinator)),
            Err(_) => {
                eprintln!(
                    "bootart systemd password broker is unavailable; refusing splash display acquisition"
                );
                None
            }
        },
        PasswordBroker::Native => native_password_listener.map(|listener| {
            Box::new(NativePromptCoordinator::new(
                listener,
                runtime.required_client_uid(),
            )) as Box<dyn PromptCoordinator>
        }),
    };
    require_selected_password_coordinator(password_broker, prompt_coordinator.as_deref())?;

    // Do not acquire or hide a VT until the explicitly selected recovery-input
    // broker is live. If setup failed above, runtime ownership unwinds here and
    // the distro console agent remains visible.
    let mut state = SplashState::new(config.mode);
    state
        .apply(StateAction::MarkRunning)
        .expect("a new daemon state can always enter running");
    let mut engine = SplashEngine::new(backend, &art, Some(&small_art), config.engine)
        .map_err(DaemonError::Engine)?;
    engine.start(&mut state).map_err(DaemonError::Engine)?;
    if emit_lifecycle_events {
        lifecycle_event("display-acquired");
    }

    let clock = SystemClock::start();
    let active_connections = Arc::new(AtomicUsize::new(0));
    let (request_sender, request_receiver) = mpsc::sync_channel(REQUEST_QUEUE_CAPACITY);

    let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        event_loop(
            config,
            &runtime,
            &listener,
            &request_sender,
            &request_receiver,
            &active_connections,
            &mut state,
            &mut engine,
            password_broker,
            &mut prompt_coordinator,
            &clock,
            runtime.required_client_uid(),
            root_transition,
            emit_lifecycle_events,
        )
    }));

    // Daemon exit, panic, and display failure are local abandonment, not user
    // cancellation. Native clients observe endpoint closure and immediately
    // restore their stock console fallback.
    if let Some(coordinator) = prompt_coordinator.as_deref_mut() {
        coordinator.abandon(&mut state);
    }

    // Stop accepting and remove the authenticated runtime path before a quit
    // client receives its completion ACK. Accepted worker sockets remain valid
    // long enough to carry that final response.
    drop(request_sender);
    let exit = match loop_result {
        Ok(Ok(exit)) => exit,
        Ok(Err(error)) => {
            let _ = state.apply(StateAction::FailOpen);
            LoopExit {
                retain_splash: false,
                deferred_reply: None,
                error: Some(error),
            }
        }
        Err(_) => {
            let _ = state.apply(StateAction::FailOpen);
            LoopExit {
                retain_splash: false,
                deferred_reply: None,
                error: Some(DaemonError::PanicBoundary),
            }
        }
    };

    let restore_result = engine.shutdown(exit.retain_splash);
    if emit_lifecycle_events && restore_result.is_ok() {
        lifecycle_event("display-restored");
    }
    if !matches!(state.lifecycle(), super::state::Lifecycle::Quitting) {
        let _ = state.apply(StateAction::Quit);
    }
    let _ = state.apply(StateAction::MarkStopped);

    drop(listener);
    drop(runtime);

    if let Some(deferred) = exit.deferred_reply {
        let DeferredReply {
            sender,
            response: deferred_response,
            completion,
        } = deferred;
        let response = match &restore_result {
            Ok(()) => deferred_response,
            Err(error) => Frame::error(
                deferred_response.request_id(),
                format!("display restoration failed: {error}"),
            )
            .unwrap_or(deferred_response),
        };
        deliver_deferred_reply(sender, completion, response, config.connection_timeout);
    }

    match (exit.error, restore_result) {
        (Some(failure), Err(restoration)) => Err(DaemonError::FailureAndRestoration {
            failure: Box::new(failure),
            restoration,
        }),
        (Some(failure), Ok(())) => Err(failure),
        (None, Err(restoration)) => Err(DaemonError::Engine(restoration)),
        (None, Ok(())) => {
            if emit_lifecycle_events {
                lifecycle_event("daemon-exit");
            }
            Ok(DaemonOutcome::Stopped)
        }
    }
}

fn lifecycle_event(event: &'static str) {
    event_record("BOOTART_LIFECYCLE_V1", event);
}

fn password_event(event: &'static str) {
    event_record("BOOTART_PASSWORD_V1", event);
}

fn event_record(prefix: &'static str, event: &'static str) {
    // A formatted `eprintln!` may issue several writes. `/dev/kmsg` treats
    // each write as a separate record, which split the machine-readable VM
    // oracle at `event=`. Build one bounded line before touching stderr so a
    // normal kernel-log sink receives one complete record.
    let record = format!("{prefix}|event={event}|pid={}\n", std::process::id());
    let _ = io::stderr().lock().write_all(record.as_bytes());
}

fn emit_prompt_transition(was_active: bool, is_active: bool) {
    match (was_active, is_active) {
        (false, true) => password_event("prompt-open"),
        (true, false) => password_event("prompt-close"),
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn event_loop<B: DisplayBackend>(
    config: &DaemonConfig,
    runtime: &RuntimeOwner,
    listener: &UnixListener,
    request_sender: &SyncSender<PendingRequest>,
    request_receiver: &Receiver<PendingRequest>,
    active_connections: &Arc<AtomicUsize>,
    state: &mut SplashState,
    engine: &mut SplashEngine<'_, B>,
    password_broker: PasswordBroker,
    prompt_coordinator: &mut Option<Box<dyn PromptCoordinator>>,
    clock: &SystemClock,
    required_mutation_uid: u32,
    root_transition: &mut dyn RootTransition,
    emit_lifecycle_events: bool,
) -> Result<LoopExit, DaemonError> {
    let mut next_systemd_rebind = Duration::ZERO;
    let mut systemd_rebind_started = None;
    loop {
        if signals::should_stop() {
            state
                .apply(StateAction::Quit)
                .expect("a running daemon can always enter quitting");
            return Ok(LoopExit::normal(false, None));
        }

        accept_ready(
            listener,
            request_sender,
            active_connections,
            config.max_connections,
            config.connection_timeout,
        )?;

        let root_stage_before = state.root_stage();
        let request_exit = process_requests(
            request_receiver,
            state,
            required_mutation_uid,
            root_transition,
            password_broker,
            prompt_coordinator,
        )?;
        if emit_lifecycle_events
            && root_stage_before != super::state::RootStage::RealRoot
            && state.root_stage() == super::state::RootStage::RealRoot
        {
            lifecycle_event("root-handoff");
        }
        if let Some(exit) = request_exit {
            return Ok(exit);
        }

        let rebind_now = clock.elapsed();
        let runtime_entries_reachable = password_broker == PasswordBroker::Systemd
            && state.root_stage() == super::state::RootStage::RealRoot
            && rebind_now >= next_systemd_rebind
            && runtime.owned_entries_reachable();
        maybe_rebind_systemd_coordinator(
            password_broker,
            state,
            runtime_entries_reachable,
            rebind_now,
            &mut next_systemd_rebind,
            prompt_coordinator,
            || {
                SystemdPromptCoordinator::open_system()
                    .ok()
                    .map(|coordinator| Box::new(coordinator) as Box<dyn PromptCoordinator>)
            },
        );

        let coordinator_enabled = prompt_coordinator
            .as_deref()
            .is_some_and(PromptCoordinator::enabled);
        match password_broker_runtime_action(
            password_broker,
            state.root_stage(),
            coordinator_enabled,
            rebind_now,
            &mut systemd_rebind_started,
        )? {
            PasswordBrokerRuntimeAction::Poll => {
                let prompt_was_active = state.view().prompt().is_some();
                if let Some(coordinator) = prompt_coordinator.as_deref_mut() {
                    coordinator.poll(state);
                    if !coordinator.enabled() {
                        return Err(DaemonError::PasswordBrokerUnavailable {
                            stage: "runtime coordinator failed",
                        });
                    }
                    engine
                        .tick_with_prompt(state, clock, coordinator)
                        .map_err(DaemonError::Engine)?;
                } else {
                    engine.tick(state, clock).map_err(DaemonError::Engine)?;
                }
                if emit_lifecycle_events {
                    emit_prompt_transition(prompt_was_active, state.view().prompt().is_some());
                }
            }
            PasswordBrokerRuntimeAction::WaitForSystemdRebind => {
                engine.tick(state, clock).map_err(DaemonError::Engine)?;
            }
            PasswordBrokerRuntimeAction::RetireNativeAfterHandoff => {
                if let Some(mut coordinator) = prompt_coordinator.take() {
                    coordinator.abandon(state);
                }
                engine.tick(state, clock).map_err(DaemonError::Engine)?;
            }
        }

        let until_frame = engine.time_until_next_frame(clock.elapsed());
        let sleep = config.accept_interval.min(until_frame);
        if sleep.is_zero() {
            std::thread::yield_now();
        } else {
            std::thread::sleep(sleep);
        }
    }
}

fn prepare_password_broker(
    requested: PasswordBroker,
    policy: &dyn ProcessSecretPolicy,
) -> Result<PasswordBroker, DaemonError> {
    if requested == PasswordBroker::None {
        return Ok(PasswordBroker::None);
    }
    match policy.protect_process() {
        Ok(()) => Ok(requested),
        Err(_) => Err(DaemonError::PasswordBrokerUnavailable {
            stage: "process dump protection failed before display acquisition",
        }),
    }
}

fn require_selected_password_coordinator(
    password_broker: PasswordBroker,
    coordinator: Option<&dyn PromptCoordinator>,
) -> Result<(), DaemonError> {
    if password_broker == PasswordBroker::None
        || coordinator.is_some_and(PromptCoordinator::enabled)
    {
        Ok(())
    } else {
        Err(DaemonError::PasswordBrokerUnavailable {
            stage: "coordinator setup failed before display acquisition",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasswordBrokerRuntimeAction {
    Poll,
    WaitForSystemdRebind,
    RetireNativeAfterHandoff,
}

fn password_broker_runtime_action(
    password_broker: PasswordBroker,
    root_stage: super::state::RootStage,
    coordinator_enabled: bool,
    now: Duration,
    systemd_rebind_started: &mut Option<Duration>,
) -> Result<PasswordBrokerRuntimeAction, DaemonError> {
    use super::state::RootStage;

    match (password_broker, root_stage, coordinator_enabled) {
        (PasswordBroker::None, _, _) => Ok(PasswordBrokerRuntimeAction::Poll),
        (PasswordBroker::Native, RootStage::RealRoot, _) => {
            Ok(PasswordBrokerRuntimeAction::RetireNativeAfterHandoff)
        }
        (PasswordBroker::Native, _, true) | (PasswordBroker::Systemd, _, true) => {
            *systemd_rebind_started = None;
            Ok(PasswordBrokerRuntimeAction::Poll)
        }
        (PasswordBroker::Systemd, RootStage::RealRoot, false) => {
            let started = systemd_rebind_started.get_or_insert(now);
            if now.saturating_sub(*started) >= SYSTEMD_REBIND_TIMEOUT {
                Err(DaemonError::PasswordBrokerUnavailable {
                    stage: "systemd coordinator rebind deadline expired",
                })
            } else {
                Ok(PasswordBrokerRuntimeAction::WaitForSystemdRebind)
            }
        }
        (PasswordBroker::Native | PasswordBroker::Systemd, _, false) => {
            Err(DaemonError::PasswordBrokerUnavailable {
                stage: "runtime coordinator became unavailable",
            })
        }
    }
}

/// Reopen systemd's absolute-path request namespace only after the original
/// `/run/bootart` dentries are reachable from the daemon's post-chroot
/// namespace. This proves that the initramfs `/run` mount has been moved into
/// the real root; reopening sooner could watch an unrelated empty directory.
#[allow(clippy::too_many_arguments)]
fn maybe_rebind_systemd_coordinator<F>(
    password_broker: PasswordBroker,
    state: &mut SplashState,
    runtime_entries_reachable: bool,
    now: Duration,
    next_attempt: &mut Duration,
    coordinator: &mut Option<Box<dyn PromptCoordinator>>,
    mut open: F,
) where
    F: FnMut() -> Option<Box<dyn PromptCoordinator>>,
{
    if password_broker != PasswordBroker::Systemd
        || state.root_stage() != super::state::RootStage::RealRoot
        || coordinator
            .as_deref()
            .is_some_and(PromptCoordinator::enabled)
        || now < *next_attempt
    {
        return;
    }

    *next_attempt = now.saturating_add(SYSTEMD_REBIND_INTERVAL);
    if !runtime_entries_reachable {
        return;
    }
    if let Some(rebound) = open() {
        *coordinator = Some(rebound);
    }
}

fn accept_ready(
    listener: &UnixListener,
    request_sender: &SyncSender<PendingRequest>,
    active_connections: &Arc<AtomicUsize>,
    max_connections: usize,
    timeout: Duration,
) -> Result<(), DaemonError> {
    for _ in 0..MAX_ACCEPTS_PER_TICK {
        match listener.accept() {
            Ok((stream, _address)) => {
                if !reserve_connection(active_connections, max_connections) {
                    // Dropping immediately bounds both memory and worker count.
                    drop(stream);
                    continue;
                }
                let slot = ConnectionSlot(Arc::clone(active_connections));
                let sender = request_sender.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("bootart-control".into())
                    .spawn(move || {
                        let _slot = slot;
                        if let Err(error) = connection_worker(stream, &sender, timeout) {
                            eprintln!("bootart rejected a control connection: {error}");
                        }
                    })
                {
                    // The moved closure (and its slot guard) is dropped on a
                    // spawn error, releasing the reservation.
                    eprintln!("bootart could not start a bounded control worker: {error}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(DaemonError::Accept(error)),
        }
    }
    Ok(())
}

fn reserve_connection(active: &AtomicUsize, maximum: usize) -> bool {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < maximum).then_some(current + 1)
        })
        .is_ok()
}

struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PendingRequest {
    request: Frame,
    peer_uid: u32,
    reply: SyncSender<Frame>,
    completion: Receiver<()>,
}

fn connection_worker(
    mut stream: UnixStream,
    sender: &SyncSender<PendingRequest>,
    timeout: Duration,
) -> Result<(), ConnectionError> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(ConnectionError::Configure)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(ConnectionError::Configure)?;

    let credentials = peer_credentials(stream.as_raw_fd()).map_err(ConnectionError::Credentials)?;
    let request = Frame::read_exact_message(&mut stream).map_err(ConnectionError::Protocol)?;
    let request_id = request.request_id();
    let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
    let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
    let pending = PendingRequest {
        request,
        peer_uid: credentials.uid,
        reply: reply_sender,
        completion: completion_receiver,
    };

    match sender.try_send(pending) {
        Ok(()) => {}
        Err(TrySendError::Full(pending)) => {
            let response =
                Frame::error(pending.request.request_id(), "daemon request queue is full")
                    .map_err(ConnectionError::Protocol)?;
            response
                .write_to(&mut stream)
                .map_err(ConnectionError::Protocol)?;
            return Ok(());
        }
        Err(TrySendError::Disconnected(_)) => return Err(ConnectionError::DaemonStopped),
    }

    let response = reply_receiver
        .recv_timeout(timeout)
        .map_err(|_| ConnectionError::ResponseTimeout(timeout))?;
    if response.request_id() != request_id {
        return Err(ConnectionError::MismatchedResponse);
    }
    let write_result = response
        .write_to(&mut stream)
        .map_err(ConnectionError::Protocol);
    let _ = completion_sender.send(());
    write_result
}

fn process_requests(
    receiver: &Receiver<PendingRequest>,
    state: &mut SplashState,
    required_mutation_uid: u32,
    root_transition: &mut dyn RootTransition,
    password_broker: PasswordBroker,
    prompt_coordinator: &mut Option<Box<dyn PromptCoordinator>>,
) -> Result<Option<LoopExit>, DaemonError> {
    for _ in 0..MAX_REQUESTS_PER_TICK {
        let pending = match receiver.try_recv() {
            Ok(pending) => pending,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(None),
        };

        if pending.request.opcode() == Opcode::NativeReady {
            // A live control socket (Ping) does not prove that the separate
            // native carrier was bound or that its coordinator remains
            // enabled. Answer only from the prepared runtime capability.
            let ready = password_broker == PasswordBroker::Native
                && prompt_coordinator
                    .as_deref()
                    .is_some_and(PromptCoordinator::enabled);
            let response = if ready {
                Frame::ack(pending.request.request_id())
            } else {
                Frame::error(
                    pending.request.request_id(),
                    "native password broker is unavailable",
                )
                .map_err(DaemonError::Protocol)?
            };
            let _ = pending.reply.send(response);
            continue;
        }

        let outcome =
            if is_mutating(pending.request.opcode()) && pending.peer_uid != required_mutation_uid {
                CommandOutcome {
                    response: Frame::error(
                        pending.request.request_id(),
                        format!(
                            "UID {} is not authorized; mutating commands require UID {}",
                            pending.peer_uid, required_mutation_uid
                        ),
                    )
                    .map_err(DaemonError::Protocol)?,
                    should_quit: false,
                    retain_splash: false,
                    fatal_root_transition: None,
                }
            } else {
                if pending.request.opcode() == Opcode::UpdateRootFs
                    && state.root_stage() != super::state::RootStage::RealRoot
                    && state.view().prompt().is_none()
                    && let Some(coordinator) = prompt_coordinator.as_deref_mut()
                {
                    // Stop accepting initramfs prompt requests before chroot
                    // can strand their namespace. Native endpoint closure
                    // wakes the adapter into its bounded console fallback.
                    // An active prompt is rejected by the state machine in the
                    // request handler and therefore is deliberately not
                    // abandoned here.
                    coordinator.abandon(state);
                }
                handle_request_with_root_transition(state, &pending.request, root_transition)
                    .map_err(DaemonError::Protocol)?
            };

        if let Some(error) = outcome.fatal_root_transition {
            return Ok(Some(LoopExit::fatal_root_transition(
                error,
                DeferredReply {
                    sender: pending.reply,
                    response: outcome.response,
                    completion: pending.completion,
                },
            )));
        }
        if outcome.should_quit {
            return Ok(Some(LoopExit::normal(
                outcome.retain_splash,
                Some(DeferredReply {
                    sender: pending.reply,
                    response: outcome.response,
                    completion: pending.completion,
                }),
            )));
        }
        let _ = pending.reply.send(outcome.response);
    }
    Ok(None)
}

struct DeferredReply {
    sender: SyncSender<Frame>,
    response: Frame,
    completion: Receiver<()>,
}

fn deliver_deferred_reply(
    sender: SyncSender<Frame>,
    completion: Receiver<()>,
    response: Frame,
    timeout: Duration,
) {
    if sender.send(response).is_ok() {
        // A detached worker owns the accepted socket. Do not let the main
        // process return (and terminate all threads) until that worker has
        // finished its bounded response write; otherwise quit ACKs race
        // process exit and clients intermittently observe an empty frame.
        let _ = completion.recv_timeout(timeout);
    }
}

struct LoopExit {
    retain_splash: bool,
    deferred_reply: Option<DeferredReply>,
    error: Option<DaemonError>,
}

impl LoopExit {
    fn normal(retain_splash: bool, deferred_reply: Option<DeferredReply>) -> Self {
        Self {
            retain_splash,
            deferred_reply,
            error: None,
        }
    }

    fn fatal_root_transition(error: RootTransitionError, deferred_reply: DeferredReply) -> Self {
        Self {
            retain_splash: false,
            deferred_reply: Some(deferred_reply),
            error: Some(DaemonError::RootTransition(error)),
        }
    }
}

#[derive(Debug)]
pub enum DaemonError {
    Pid1(Pid1Refused),
    Cmdline {
        path: PathBuf,
        source: io::Error,
    },
    EmbeddedArt(ValidationError),
    Signals(io::Error),
    Runtime(RuntimeError),
    ConfigureListener(io::Error),
    Accept(io::Error),
    Protocol(ProtocolError),
    Engine(EngineError),
    FailureAndRestoration {
        failure: Box<DaemonError>,
        restoration: EngineError,
    },
    RootTransition(RootTransitionError),
    PasswordBrokerUnavailable {
        stage: &'static str,
    },
    InvalidMaxConnections,
    TestBackendOnProductionRuntime,
    PanicBoundary,
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pid1(error) => error.fmt(formatter),
            Self::Cmdline { path, source } => write!(
                formatter,
                "failed to read kernel command line {}: {source}",
                path.display()
            ),
            Self::EmbeddedArt(error) => {
                write!(formatter, "embedded splash art is invalid: {error}")
            }
            Self::Signals(error) => write!(
                formatter,
                "failed to install terminal-restoration signal handlers: {error}"
            ),
            Self::Runtime(error) => error.fmt(formatter),
            Self::ConfigureListener(error) => {
                write!(formatter, "failed to configure control listener: {error}")
            }
            Self::Accept(error) => write!(formatter, "control listener failed: {error}"),
            Self::Protocol(error) => error.fmt(formatter),
            Self::Engine(error) => error.fmt(formatter),
            Self::FailureAndRestoration {
                failure,
                restoration,
            } => write!(
                formatter,
                "{failure}; cleanup after that failure also failed: {restoration}"
            ),
            Self::RootTransition(error) => {
                write!(formatter, "fatal real-root transition failure: {error}")
            }
            Self::PasswordBrokerUnavailable { stage } => write!(
                formatter,
                "password broker unavailable ({stage}); splash display was not retained"
            ),
            Self::InvalidMaxConnections => {
                formatter.write_str("maximum control connection count must be non-zero")
            }
            Self::TestBackendOnProductionRuntime => formatter.write_str(
                "the in-memory test display is forbidden with the production runtime path",
            ),
            Self::PanicBoundary => formatter.write_str(
                "daemon event loop panicked; display and runtime restoration were attempted",
            ),
        }
    }
}

impl Error for DaemonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Pid1(error) => Some(error),
            Self::Cmdline { source, .. }
            | Self::Signals(source)
            | Self::ConfigureListener(source)
            | Self::Accept(source) => Some(source),
            Self::EmbeddedArt(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::FailureAndRestoration { restoration, .. } => Some(restoration),
            Self::RootTransition(error) => Some(error),
            Self::PasswordBrokerUnavailable { .. } => None,
            Self::InvalidMaxConnections
            | Self::TestBackendOnProductionRuntime
            | Self::PanicBoundary => None,
        }
    }
}

impl DaemonError {
    /// True when the daemon could not prove that it restored/released display
    /// ownership. Initramfs adapters use this distinction to avoid racing a
    /// stock console password prompt against an ambiguously owned VT.
    pub fn display_restoration_failed(&self) -> bool {
        match self {
            Self::Engine(error) => error.restoration_failed(),
            Self::FailureAndRestoration { .. } => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
enum ConnectionError {
    Configure(io::Error),
    Credentials(io::Error),
    Protocol(ProtocolError),
    DaemonStopped,
    ResponseTimeout(Duration),
    MismatchedResponse,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configure(error) => write!(formatter, "socket configuration failed: {error}"),
            Self::Credentials(error) => write!(formatter, "peer authentication failed: {error}"),
            Self::Protocol(error) => error.fmt(formatter),
            Self::DaemonStopped => formatter.write_str("daemon stopped before request dispatch"),
            Self::ResponseTimeout(timeout) => {
                write!(formatter, "daemon response exceeded {timeout:?}")
            }
            Self::MismatchedResponse => {
                formatter.write_str("daemon worker received a mismatched response")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splash::protocol::Opcode;
    use crate::splash::root_transition::SystemFailure;
    use std::cell::Cell;
    use std::path::Path;
    use std::rc::Rc;

    struct FakeSecretPolicy {
        calls: Cell<usize>,
        fail: bool,
    }

    impl ProcessSecretPolicy for FakeSecretPolicy {
        fn protect_process(&self) -> io::Result<()> {
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                Err(io::Error::other("injected policy failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn broker_is_explicit_and_dump_policy_failure_refuses_presentation() {
        let unused = FakeSecretPolicy {
            calls: Cell::new(0),
            fail: true,
        };
        assert!(matches!(
            prepare_password_broker(PasswordBroker::None, &unused),
            Ok(PasswordBroker::None)
        ));
        assert_eq!(unused.calls.get(), 0);

        let accepted = FakeSecretPolicy {
            calls: Cell::new(0),
            fail: false,
        };
        assert!(matches!(
            prepare_password_broker(PasswordBroker::Systemd, &accepted),
            Ok(PasswordBroker::Systemd)
        ));
        assert_eq!(accepted.calls.get(), 1);

        let rejected = FakeSecretPolicy {
            calls: Cell::new(0),
            fail: true,
        };
        assert!(matches!(
            prepare_password_broker(PasswordBroker::Systemd, &rejected),
            Err(DaemonError::PasswordBrokerUnavailable {
                stage: "process dump protection failed before display acquisition"
            })
        ));
        assert_eq!(rejected.calls.get(), 1);

        let native = FakeSecretPolicy {
            calls: Cell::new(0),
            fail: false,
        };
        assert!(matches!(
            prepare_password_broker(PasswordBroker::Native, &native),
            Ok(PasswordBroker::Native)
        ));
        assert_eq!(native.calls.get(), 1);
    }

    #[test]
    fn selected_broker_requires_a_live_coordinator_before_display_acquisition() {
        assert!(require_selected_password_coordinator(PasswordBroker::None, None).is_ok());
        for broker in [PasswordBroker::Systemd, PasswordBroker::Native] {
            assert!(matches!(
                require_selected_password_coordinator(broker, None),
                Err(DaemonError::PasswordBrokerUnavailable {
                    stage: "coordinator setup failed before display acquisition"
                })
            ));
        }
    }

    #[test]
    fn runtime_broker_failure_and_systemd_rebind_wait_are_fail_open_and_bounded() {
        use super::super::state::RootStage;

        let mut started = None;
        assert!(matches!(
            password_broker_runtime_action(
                PasswordBroker::Systemd,
                RootStage::Initramfs,
                false,
                Duration::ZERO,
                &mut started,
            ),
            Err(DaemonError::PasswordBrokerUnavailable {
                stage: "runtime coordinator became unavailable"
            })
        ));

        assert_eq!(
            password_broker_runtime_action(
                PasswordBroker::Systemd,
                RootStage::RealRoot,
                false,
                Duration::from_secs(10),
                &mut started,
            )
            .unwrap(),
            PasswordBrokerRuntimeAction::WaitForSystemdRebind
        );
        assert_eq!(started, Some(Duration::from_secs(10)));
        assert_eq!(
            password_broker_runtime_action(
                PasswordBroker::Systemd,
                RootStage::RealRoot,
                false,
                Duration::from_secs(39),
                &mut started,
            )
            .unwrap(),
            PasswordBrokerRuntimeAction::WaitForSystemdRebind
        );
        assert!(matches!(
            password_broker_runtime_action(
                PasswordBroker::Systemd,
                RootStage::RealRoot,
                false,
                Duration::from_secs(40),
                &mut started,
            ),
            Err(DaemonError::PasswordBrokerUnavailable {
                stage: "systemd coordinator rebind deadline expired"
            })
        ));

        assert_eq!(
            password_broker_runtime_action(
                PasswordBroker::Systemd,
                RootStage::RealRoot,
                true,
                Duration::from_secs(40),
                &mut started,
            )
            .unwrap(),
            PasswordBrokerRuntimeAction::Poll
        );
        assert_eq!(started, None);
        assert_eq!(
            password_broker_runtime_action(
                PasswordBroker::Native,
                RootStage::RealRoot,
                false,
                Duration::ZERO,
                &mut started,
            )
            .unwrap(),
            PasswordBrokerRuntimeAction::RetireNativeAfterHandoff
        );
    }

    #[test]
    fn restoration_ambiguity_has_a_distinct_adapter_exit_class() {
        let ordinary =
            DaemonError::Engine(EngineError::Display(crate::display::DisplayError::backend(
                "test",
                "render",
                io::Error::other("injected render failure"),
            )));
        assert!(!ordinary.display_restoration_failed());

        let ambiguous = DaemonError::Engine(EngineError::Restoration(
            crate::display::DisplayError::backend(
                "test",
                "restore",
                io::Error::other("injected restore failure"),
            ),
        ));
        assert!(ambiguous.display_restoration_failed());
        assert_eq!(DISPLAY_RESTORATION_FAILED_EXIT_CODE, 77);
    }

    struct AbandonProbe(Rc<Cell<usize>>, bool);

    impl PromptCoordinator for AbandonProbe {
        fn poll(&mut self, _state: &mut SplashState) {}
        fn handle_input(&mut self, _state: &mut SplashState, bytes: &mut [u8]) {
            bytes.fill(0);
        }
        fn feedback(&self) -> Option<crate::password::InputFeedback> {
            None
        }
        fn with_visible_text(&self, _render: &mut dyn FnMut(&str)) {}
        fn abandon(&mut self, _state: &mut SplashState) {
            self.0.set(self.0.get() + 1);
        }
        fn enabled(&self) -> bool {
            self.1
        }
    }

    fn native_readiness_response(
        password_broker: PasswordBroker,
        coordinator_enabled: Option<bool>,
    ) -> Opcode {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::empty(Opcode::NativeReady, 71).unwrap(),
                peer_uid: 2000,
                reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();
        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        let mut prompt = coordinator_enabled.map(|enabled| {
            Box::new(AbandonProbe(Rc::new(Cell::new(0)), enabled)) as Box<dyn PromptCoordinator>
        });
        let mut transition = DeferredRootTransition;

        assert!(
            process_requests(
                &receiver,
                &mut state,
                1000,
                &mut transition,
                password_broker,
                &mut prompt,
            )
            .unwrap()
            .is_none()
        );
        reply_receiver.recv().unwrap().opcode()
    }

    #[test]
    fn native_readiness_requires_the_prepared_native_broker_and_enabled_coordinator() {
        assert_eq!(
            native_readiness_response(PasswordBroker::Native, Some(true)),
            Opcode::Ack
        );
        assert_eq!(
            native_readiness_response(PasswordBroker::Native, Some(false)),
            Opcode::Error
        );
        assert_eq!(
            native_readiness_response(PasswordBroker::Native, None),
            Opcode::Error
        );
        assert_eq!(
            native_readiness_response(PasswordBroker::Systemd, Some(true)),
            Opcode::Error
        );
    }

    #[test]
    fn systemd_rebind_waits_for_real_root_runtime_identity_and_is_bounded() {
        fn attempt(
            state: &mut SplashState,
            coordinator: &mut Option<Box<dyn PromptCoordinator>>,
            next_attempt: &mut Duration,
            calls: &Rc<Cell<usize>>,
            runtime_visible: bool,
            now: Duration,
            succeeds: bool,
        ) {
            let calls = Rc::clone(calls);
            maybe_rebind_systemd_coordinator(
                PasswordBroker::Systemd,
                state,
                runtime_visible,
                now,
                next_attempt,
                coordinator,
                move || {
                    calls.set(calls.get() + 1);
                    succeeds.then(|| {
                        Box::new(AbandonProbe(Rc::new(Cell::new(0)), true))
                            as Box<dyn PromptCoordinator>
                    })
                },
            );
        }

        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        state
            .apply(StateAction::SetRootStage(
                super::super::state::RootStage::Switching,
            ))
            .unwrap();
        state
            .apply(StateAction::SetRootStage(
                super::super::state::RootStage::RealRoot,
            ))
            .unwrap();

        let old_abandons = Rc::new(Cell::new(0));
        let mut coordinator: Option<Box<dyn PromptCoordinator>> =
            Some(Box::new(AbandonProbe(Rc::clone(&old_abandons), false)));
        let calls = Rc::new(Cell::new(0));
        let mut next_attempt = Duration::ZERO;

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            false,
            Duration::ZERO,
            true,
        );
        assert_eq!(calls.get(), 0, "an unrelated /run must not be watched");

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            true,
            Duration::from_millis(249),
            false,
        );
        assert_eq!(calls.get(), 0, "runtime identity probes must be paced");

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            true,
            Duration::from_millis(250),
            false,
        );
        assert_eq!(calls.get(), 1);
        assert!(!coordinator.as_deref().unwrap().enabled());

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            true,
            Duration::from_millis(499),
            true,
        );
        assert_eq!(calls.get(), 1, "reopen attempts must be paced");

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            true,
            Duration::from_millis(500),
            true,
        );
        assert_eq!(calls.get(), 2);
        assert!(coordinator.as_deref().unwrap().enabled());

        attempt(
            &mut state,
            &mut coordinator,
            &mut next_attempt,
            &calls,
            true,
            Duration::from_secs(1),
            true,
        );
        assert_eq!(calls.get(), 2, "an enabled coordinator is not replaced");
    }

    struct TransitionAfterAbandon {
        abandon_calls: Rc<Cell<usize>>,
        transition_calls: usize,
    }

    impl RootTransition for TransitionAfterAbandon {
        fn transition(
            &mut self,
            _new_root: &Path,
        ) -> Result<(), super::super::root_transition::RootTransitionError> {
            assert_eq!(self.abandon_calls.get(), 1);
            self.transition_calls += 1;
            Ok(())
        }
    }

    struct FailingRootTransition {
        rollback_incomplete: bool,
    }

    impl RootTransition for FailingRootTransition {
        fn transition(&mut self, _new_root: &Path) -> Result<(), RootTransitionError> {
            Err(RootTransitionError::TransitionFailed {
                failure: SystemFailure {
                    operation: "change directory to new root",
                    kind: io::ErrorKind::Other,
                    errno: None,
                },
                rollback_failures: self
                    .rollback_incomplete
                    .then_some(SystemFailure {
                        operation: "restore old root",
                        kind: io::ErrorKind::Other,
                        errno: None,
                    })
                    .into_iter()
                    .collect(),
            })
        }
    }

    #[test]
    fn root_handoff_abandons_absolute_reply_namespace_before_transition() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::text(Opcode::UpdateRootFs, 18, "/sysroot").unwrap(),
                peer_uid: 1000,
                reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();
        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        let abandon_calls = Rc::new(Cell::new(0));
        let mut prompt: Option<Box<dyn PromptCoordinator>> =
            Some(Box::new(AbandonProbe(Rc::clone(&abandon_calls), true)));
        let mut transition = TransitionAfterAbandon {
            abandon_calls: Rc::clone(&abandon_calls),
            transition_calls: 0,
        };

        assert!(
            process_requests(
                &receiver,
                &mut state,
                1000,
                &mut transition,
                PasswordBroker::Native,
                &mut prompt,
            )
            .unwrap()
            .is_none()
        );

        assert_eq!(reply_receiver.recv().unwrap().opcode(), Opcode::Ack);
        assert_eq!(abandon_calls.get(), 1);
        assert_eq!(transition.transition_calls, 1);
    }

    #[test]
    fn root_transition_failure_stops_dispatch_and_defers_error_until_cleanup() {
        assert_root_transition_failure_stops(false);
        assert_root_transition_failure_stops(true);
    }

    #[test]
    fn prompt_racing_root_handoff_stops_and_defers_error_until_cleanup() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::text(Opcode::UpdateRootFs, 73, "/sysroot").unwrap(),
                peer_uid: 1000,
                reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();
        let (later_reply, later_reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::empty(Opcode::Ping, 74).unwrap(),
                peer_uid: 1000,
                reply: later_reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();

        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        state
            .apply(StateAction::BeginPrompt(
                super::super::state::PromptMetadata::new(51, "Non-disk boot prompt").unwrap(),
            ))
            .unwrap();
        let abandon_calls = Rc::new(Cell::new(0));
        let mut prompt: Option<Box<dyn PromptCoordinator>> =
            Some(Box::new(AbandonProbe(Rc::clone(&abandon_calls), true)));
        let mut transition = TransitionAfterAbandon {
            abandon_calls: Rc::clone(&abandon_calls),
            transition_calls: 0,
        };

        let exit = process_requests(
            &receiver,
            &mut state,
            1000,
            &mut transition,
            PasswordBroker::Systemd,
            &mut prompt,
        )
        .unwrap()
        .expect("handoff rejection must stop the event loop");

        assert_eq!(
            state.lifecycle(),
            super::super::state::Lifecycle::FailedOpen
        );
        assert_eq!(
            state.root_stage(),
            super::super::state::RootStage::Initramfs
        );
        assert!(state.view().prompt().is_none());
        assert_eq!(abandon_calls.get(), 0);
        assert_eq!(transition.transition_calls, 0);
        assert!(!exit.retain_splash);
        assert!(exit.error.is_none());
        assert!(matches!(
            reply_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));
        assert!(matches!(
            later_reply_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));

        let deferred = exit
            .deferred_reply
            .expect("handoff rejection response must wait for cleanup");
        assert_eq!(deferred.response.opcode(), Opcode::Error);
        deferred.sender.send(deferred.response).unwrap();
        assert_eq!(reply_receiver.recv().unwrap().opcode(), Opcode::Error);
    }

    fn assert_root_transition_failure_stops(rollback_incomplete: bool) {
        let (sender, receiver) = mpsc::sync_channel(2);
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::text(Opcode::UpdateRootFs, 19, "/sysroot").unwrap(),
                peer_uid: 1000,
                reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();
        let (later_reply, later_reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::empty(Opcode::Ping, 20).unwrap(),
                peer_uid: 1000,
                reply: later_reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();

        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        let abandon_calls = Rc::new(Cell::new(0));
        let mut prompt: Option<Box<dyn PromptCoordinator>> =
            Some(Box::new(AbandonProbe(Rc::clone(&abandon_calls), true)));
        let mut transition = FailingRootTransition {
            rollback_incomplete,
        };

        let exit = process_requests(
            &receiver,
            &mut state,
            1000,
            &mut transition,
            PasswordBroker::Native,
            &mut prompt,
        )
        .unwrap()
        .expect("every failed root transition must stop the event loop");

        assert_eq!(
            state.lifecycle(),
            super::super::state::Lifecycle::FailedOpen
        );
        assert_eq!(
            state.root_stage(),
            super::super::state::RootStage::Switching
        );
        assert_eq!(abandon_calls.get(), 1);
        assert!(!exit.retain_splash);
        assert!(matches!(
            exit.error.as_ref(),
            Some(DaemonError::RootTransition(error))
                if error.rollback_incomplete() == rollback_incomplete
        ));
        assert!(matches!(
            reply_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));
        assert!(matches!(
            later_reply_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));

        let deferred = exit
            .deferred_reply
            .expect("fatal response must wait for display/runtime cleanup");
        assert_eq!(deferred.response.opcode(), Opcode::Error);
        deferred.sender.send(deferred.response).unwrap();
        assert_eq!(reply_receiver.recv().unwrap().opcode(), Opcode::Error);
    }

    #[test]
    fn connection_reservations_are_strictly_bounded() {
        let active = AtomicUsize::new(0);
        assert!(reserve_connection(&active, 2));
        assert!(reserve_connection(&active, 2));
        assert!(!reserve_connection(&active, 2));
        assert_eq!(active.load(Ordering::Acquire), 2);
    }

    #[test]
    fn quit_ack_is_deferred_until_the_cleanup_boundary_releases_it() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        sender
            .send(PendingRequest {
                request: Frame::quit(17, true).unwrap(),
                peer_uid: 1000,
                reply,
                completion: mpsc::sync_channel(1).1,
            })
            .unwrap();
        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        let mut transition = DeferredRootTransition;

        let exit = process_requests(
            &receiver,
            &mut state,
            1000,
            &mut transition,
            PasswordBroker::None,
            &mut None,
        )
        .unwrap()
        .expect("quit must stop the event loop");
        assert!(exit.retain_splash);
        assert!(matches!(
            reply_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));

        let deferred = exit.deferred_reply.unwrap();
        deferred.sender.send(deferred.response).unwrap();
        assert_eq!(reply_receiver.recv().unwrap().opcode(), Opcode::Ack);
    }

    #[test]
    fn deferred_reply_waits_until_the_socket_worker_finishes_writing() {
        let (reply, reply_receiver) = mpsc::sync_channel(1);
        let (completion, completion_receiver) = mpsc::sync_channel(1);
        let (returned, returned_receiver) = mpsc::sync_channel(1);
        let deferred = DeferredReply {
            sender: reply,
            response: Frame::ack(91),
            completion: completion_receiver,
        };

        let worker = std::thread::spawn(move || {
            deliver_deferred_reply(
                deferred.sender,
                deferred.completion,
                Frame::ack(91),
                Duration::from_secs(1),
            );
            returned.send(()).unwrap();
        });

        assert_eq!(
            reply_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .opcode(),
            Opcode::Ack
        );
        assert!(matches!(
            returned_receiver.try_recv(),
            Err(TryRecvError::Empty)
        ));
        completion.send(()).unwrap();
        returned_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        worker.join().unwrap();
    }
}
