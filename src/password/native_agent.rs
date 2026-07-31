//! Experimental native password broker.
//!
//! Non-secret prompt metadata and exactly one private credential responder are
//! transferred atomically over a dedicated root-authenticated
//! `AF_UNIX/SOCK_SEQPACKET` carrier. Secret bytes never enter this carrier, the
//! general control socket, argv, the environment, stdout, or presentation
//! state.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::splash::runtime::{
    NativePasswordListener, RuntimePaths, peer_credentials, unix_address,
};
use crate::splash::state::{PromptMetadata, PromptOutcome, SplashState, StateAction};

use super::coordinator::PromptCoordinator;
use super::credential::{
    LinuxCredentialPeerAuthenticator, NativeCredentialError, NativeCredentialResponder,
    native_credential_pair, receive_responder_packet, send_responder_packet,
};
use super::input::{InputFeedback, InputOutcome, PromptInput};
use super::pipe_askpass::{
    InheritedSecretPipe, PipeAskpassDisposition, PipeAskpassError, PipeAskpassMetadata,
    PipeSecretFraming,
};
use super::secure::{LinuxProcessSecretPolicy, ProcessSecretPolicy};

const REQUEST_MAGIC: [u8; 4] = *b"BNAP";
const REQUEST_VERSION: u8 = 1;
const REQUEST_OPCODE: u8 = 1;
const REQUEST_HEADER_BYTES: usize = 52;
const FLAG_ECHO: u8 = 1 << 0;
const FLAG_SILENT: u8 = 1 << 1;
const KNOWN_FLAGS: u8 = FLAG_ECHO | FLAG_SILENT;
const MAX_PROMPT_BYTES: usize = 1024;
const MAX_PENDING_CARRIERS: usize = 32;
const MAX_PROMPT_REQUESTS: usize = 32;
const MAX_RETIRED_IDENTITIES: usize = 64;
const MAX_ACCEPTS_PER_POLL: usize = 8;
const MAX_HANDSHAKES_PER_POLL: usize = 16;
const HANDSHAKE_TIMEOUT_MICROS: u64 = 2_000_000;
const MAX_REQUEST_LIFETIME_MICROS: u64 = 5 * 60 * 1_000_000;
const LIVENESS_INTERVAL_MICROS: u64 = 100_000;
const PROC_STAT_MAX_BYTES: u64 = 4096;

pub const NATIVE_ASKPASS_OUTPUT_FD: RawFd = 8;
pub const NATIVE_ASKPASS_TIMEOUT: Duration = Duration::from_secs(90);
/// The native broker could not safely deliver a credential. The exact adapter
/// may use its stock console path only after splash restoration is confirmed.
pub const NATIVE_ASKPASS_TRANSPORT_EXIT_CODE: i32 = 75;
/// The user explicitly cancelled the prompt. This is not a transport failure
/// and must not silently turn into a second console prompt.
pub const NATIVE_ASKPASS_CANCELLED_EXIT_CODE: i32 = 76;

static NEXT_REQUEST_GENERATION: AtomicU64 = AtomicU64::new(1);
static NATIVE_OUTPUT_CLAIMED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeAdapter {
    DracutClassic = 1,
    InitramfsToolsBusybox = 2,
    MkinitfsBusybox = 3,
    MkinitfsBootDeploy = 4,
    MkinitcpioBusybox = 5,
}

impl NativeAdapter {
    /// Secret framing is part of the reviewed framework contract. It is never
    /// selected by arbitrary client input.
    pub const fn secret_framing(self) -> PipeSecretFraming {
        match self {
            // Classic dracut feeds cryptsetup's ordinary stdin command path.
            Self::DracutClassic => PipeSecretFraming::NewlineTerminated,
            // cryptsetup-initramfs's stock askpass emits the exact passphrase
            // bytes to `run_keyscript | unlock_mapping`, with no terminator.
            Self::InitramfsToolsBusybox => PipeSecretFraming::Exact,
            // mkinitfs 3.14.0 nlplug-findfs reads one line with fgets and
            // strips the trailing newline before libcryptsetup activation.
            Self::MkinitfsBusybox => PipeSecretFraming::NewlineTerminated,
            // The reviewed unl0kr producer writes to the left side of the
            // stock `unl0kr | cryptsetup ... -` anonymous pipe.
            Self::MkinitfsBootDeploy => PipeSecretFraming::NewlineTerminated,
            // mkinitcpio's BusyBox encrypt hook uses `--key-file=-`; unlike
            // cryptsetup's interactive stdin path, every pipe byte is key
            // material, so appending a newline changes the LUKS passphrase.
            Self::MkinitcpioBusybox => PipeSecretFraming::Exact,
        }
    }

    const fn prompt_source(self) -> &'static str {
        match self {
            Self::DracutClassic => "dracut-classic-native",
            Self::InitramfsToolsBusybox => "initramfs-tools-busybox-native",
            Self::MkinitfsBusybox => "mkinitfs-busybox-native",
            Self::MkinitfsBootDeploy => "mkinitfs-boot-deploy-native",
            Self::MkinitcpioBusybox => "mkinitcpio-busybox-native",
        }
    }
}

impl TryFrom<u8> for NativeAdapter {
    type Error = NativeAgentError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::DracutClassic),
            2 => Ok(Self::InitramfsToolsBusybox),
            3 => Ok(Self::MkinitfsBusybox),
            4 => Ok(Self::MkinitfsBootDeploy),
            5 => Ok(Self::MkinitcpioBusybox),
            _ => Err(NativeAgentError::InvalidProtocol),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeAskpassClientOutcome {
    Delivered,
    UserCancelled,
    ConsoleFallback,
}

#[derive(Debug)]
pub enum NativeAgentError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Credential(NativeCredentialError),
    Pipe(PipeAskpassError),
    InvalidProtocol,
    InvalidDeadline,
    PeerPidMismatch {
        authenticated: u32,
        declared: u32,
    },
    WrongPeerUid {
        expected: u32,
        actual: u32,
    },
    RequesterIdentityUnavailable,
    ClientTimedOut,
    OutputConsumerGone,
}

impl NativeAgentError {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }

    fn is_would_block(&self) -> bool {
        matches!(
            self,
            Self::Credential(NativeCredentialError::Io { source, .. })
                if source.kind() == io::ErrorKind::WouldBlock
        )
    }
}

impl fmt::Display for NativeAgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Credential(source) => write!(formatter, "native credential transport: {source}"),
            Self::Pipe(source) => write!(formatter, "native inherited pipe: {source}"),
            Self::InvalidProtocol => formatter.write_str("invalid native prompt request"),
            Self::InvalidDeadline => formatter.write_str("invalid native prompt deadline"),
            Self::PeerPidMismatch {
                authenticated,
                declared,
            } => write!(
                formatter,
                "native requester PID {declared} does not match authenticated PID {authenticated}"
            ),
            Self::WrongPeerUid { expected, actual } => write!(
                formatter,
                "native requester UID {actual} does not match required UID {expected}"
            ),
            Self::RequesterIdentityUnavailable => {
                formatter.write_str("native requester process identity is unavailable")
            }
            Self::ClientTimedOut => formatter.write_str("native askpass client deadline expired"),
            Self::OutputConsumerGone => {
                formatter.write_str("native askpass pipe consumer disappeared")
            }
        }
    }
}

impl Error for NativeAgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Credential(source) => Some(source),
            Self::Pipe(source) => Some(source),
            _ => None,
        }
    }
}

impl From<NativeCredentialError> for NativeAgentError {
    fn from(value: NativeCredentialError) -> Self {
        Self::Credential(value)
    }
}

impl From<PipeAskpassError> for NativeAgentError {
    fn from(value: PipeAskpassError) -> Self {
        Self::Pipe(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestIdentity {
    request_id: u64,
    generation: u64,
    requester_pid: u32,
    requester_start_ticks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestMetadata {
    adapter: NativeAdapter,
    identity: RequestIdentity,
    deadline_micros: u64,
    attempt: u16,
    attempts: u16,
    maximum_secret_bytes: usize,
    prompt: String,
    echo: bool,
    silent: bool,
}

/// One request received from the atomic metadata/SCM_RIGHTS boundary. This
/// type intentionally has no `Debug` or `Clone` implementation.
struct NativePromptRequest {
    metadata: RequestMetadata,
    responder: Option<NativeCredentialResponder>,
}

struct PendingCarrier {
    descriptor: OwnedFd,
    requester_pid: u32,
    accepted_at_micros: u64,
}

trait NativeRequestSource {
    fn poll_requests(
        &mut self,
        now_micros: u64,
    ) -> Result<Vec<NativePromptRequest>, NativeAgentError>;

    fn close(&mut self) {}
}

struct NativeRequestInbox {
    listener: Option<NativePasswordListener>,
    required_uid: u32,
    pending: VecDeque<PendingCarrier>,
}

impl NativeRequestInbox {
    fn new(listener: NativePasswordListener, required_uid: u32) -> Self {
        Self {
            listener: Some(listener),
            required_uid,
            pending: VecDeque::new(),
        }
    }

    fn accept_bounded(&mut self, now_micros: u64) -> Result<(), NativeAgentError> {
        let Some(listener) = self.listener.as_ref() else {
            return Ok(());
        };
        for _ in 0..MAX_ACCEPTS_PER_POLL {
            match listener.accept() {
                Ok(descriptor) => {
                    let Ok(credentials) = peer_credentials(descriptor.as_raw_fd()) else {
                        continue;
                    };
                    if credentials.uid != self.required_uid || credentials.pid == 0 {
                        continue;
                    }
                    if self.pending.len() >= MAX_PENDING_CARRIERS {
                        continue;
                    }
                    self.pending.push_back(PendingCarrier {
                        descriptor,
                        requester_pid: credentials.pid,
                        accepted_at_micros: now_micros,
                    });
                }
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => break,
                Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(NativeAgentError::io(
                        "accept native password carrier",
                        source,
                    ));
                }
            }
        }
        Ok(())
    }
}

impl NativeRequestSource for NativeRequestInbox {
    fn poll_requests(
        &mut self,
        now_micros: u64,
    ) -> Result<Vec<NativePromptRequest>, NativeAgentError> {
        self.accept_bounded(now_micros)?;
        let mut requests = Vec::new();
        let attempts = self.pending.len().min(MAX_HANDSHAKES_PER_POLL);
        for _ in 0..attempts {
            let Some(carrier) = self.pending.pop_front() else {
                break;
            };
            let mut packet = [0_u8; REQUEST_HEADER_BYTES + MAX_PROMPT_BYTES];
            match receive_responder_packet(
                carrier.descriptor.as_raw_fd(),
                self.required_uid,
                &LinuxCredentialPeerAuthenticator,
                &mut packet,
            ) {
                Ok((length, responder)) => {
                    let Ok(metadata) = decode_request(&packet[..length], now_micros) else {
                        continue;
                    };
                    if metadata.identity.requester_pid != carrier.requester_pid {
                        continue;
                    }
                    requests.push(NativePromptRequest {
                        metadata,
                        responder: Some(responder),
                    });
                }
                Err(source) => {
                    let error = NativeAgentError::Credential(source);
                    let timed_out = now_micros.saturating_sub(carrier.accepted_at_micros)
                        >= HANDSHAKE_TIMEOUT_MICROS;
                    if error.is_would_block()
                        && !timed_out
                        && !socket_peer_gone(carrier.descriptor.as_raw_fd())
                    {
                        self.pending.push_back(carrier);
                    }
                    // Malformed, unauthenticated, timed-out, and disconnected
                    // peers lose only their own request. The listener remains
                    // available for stock-adapter retries.
                }
            }
        }
        Ok(requests)
    }

    fn close(&mut self) {
        self.pending.clear();
        self.listener.take();
    }
}

trait NativeClock {
    fn now_micros(&self) -> Result<u64, NativeAgentError>;
}

struct LinuxNativeClock;

impl NativeClock for LinuxNativeClock {
    fn now_micros(&self) -> Result<u64, NativeAgentError> {
        monotonic_micros()
    }
}

trait NativeRequesterLiveness {
    fn is_alive(&self, pid: u32, start_ticks: u64) -> Result<bool, NativeAgentError>;
}

struct LinuxNativeRequesterLiveness;

impl NativeRequesterLiveness for LinuxNativeRequesterLiveness {
    fn is_alive(&self, pid: u32, start_ticks: u64) -> Result<bool, NativeAgentError> {
        match process_start_ticks(pid) {
            Ok(actual) => Ok(actual == start_ticks),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(NativeAgentError::io(
                "inspect native requester identity",
                source,
            )),
        }
    }
}

struct NativeActivePrompt {
    request: NativePromptRequest,
    presentation_id: u64,
    input: PromptInput,
}

/// Native prompt coordinator shared by classic-dracut-style inherited pipe
/// adapters. This type intentionally has no `Debug` or `Clone` implementation.
pub struct NativePromptCoordinator {
    source: Box<dyn NativeRequestSource>,
    clock: Box<dyn NativeClock>,
    liveness: Box<dyn NativeRequesterLiveness>,
    queue: VecDeque<NativePromptRequest>,
    active: Option<NativeActivePrompt>,
    retired: VecDeque<RequestIdentity>,
    next_presentation_id: u64,
    next_liveness_micros: u64,
    enabled: bool,
}

impl NativePromptCoordinator {
    pub fn new(listener: NativePasswordListener, required_uid: u32) -> Self {
        Self::with_components(
            Box::new(NativeRequestInbox::new(listener, required_uid)),
            Box::new(LinuxNativeClock),
            Box::new(LinuxNativeRequesterLiveness),
        )
    }

    fn with_components(
        source: Box<dyn NativeRequestSource>,
        clock: Box<dyn NativeClock>,
        liveness: Box<dyn NativeRequesterLiveness>,
    ) -> Self {
        Self {
            source,
            clock,
            liveness,
            queue: VecDeque::new(),
            active: None,
            retired: VecDeque::new(),
            next_presentation_id: 1,
            next_liveness_micros: 0,
            enabled: true,
        }
    }

    fn poll_inner(&mut self, state: &mut SplashState) -> Result<(), NativeAgentError> {
        let now = self.clock.now_micros()?;
        for request in self.source.poll_requests(now)? {
            self.enqueue(request, now);
        }
        if now >= self.next_liveness_micros {
            self.next_liveness_micros = now.saturating_add(LIVENESS_INTERVAL_MICROS);
            self.reap(state, now);
        }
        self.activate_next(state, now)
    }

    fn enqueue(&mut self, request: NativePromptRequest, now: u64) {
        let identity = request.metadata.identity;
        let duplicate = self.retired.contains(&identity)
            || self
                .active
                .as_ref()
                .is_some_and(|active| active.request.metadata.identity == identity)
            || self
                .queue
                .iter()
                .any(|queued| queued.metadata.identity == identity);
        let full = self.queue.len() + usize::from(self.active.is_some()) >= MAX_PROMPT_REQUESTS;
        let invalid = request.metadata.deadline_micros <= now
            || request.metadata.deadline_micros.saturating_sub(now) > MAX_REQUEST_LIFETIME_MICROS;
        if duplicate || full || invalid {
            // Dropping the only responder endpoint wakes the same-ELF client,
            // which closes its inherited pipe and reports transport failure to
            // the exact adapter.
            drop(request);
            return;
        }
        self.queue.push_back(request);
    }

    fn reap(&mut self, state: &mut SplashState, now: u64) {
        let active_reason = self.active.as_ref().and_then(|active| {
            let request = &active.request;
            if request.metadata.deadline_micros <= now {
                Some(PromptOutcome::TimedOut)
            } else if request
                .responder
                .as_ref()
                .is_none_or(|responder| socket_peer_gone(responder.as_raw_fd()))
                || !self.requester_alive(&request.metadata)
            {
                Some(PromptOutcome::RequestGone)
            } else {
                None
            }
        });
        if let Some(outcome) = active_reason {
            self.finish_active(state, outcome);
        }

        let mut retained = VecDeque::new();
        while let Some(request) = self.queue.pop_front() {
            let expired = request.metadata.deadline_micros <= now;
            let gone = request
                .responder
                .as_ref()
                .is_none_or(|responder| socket_peer_gone(responder.as_raw_fd()))
                || !self.requester_alive(&request.metadata);
            if expired || gone {
                self.retire(request.metadata.identity);
            } else {
                retained.push_back(request);
            }
        }
        self.queue = retained;
    }

    fn requester_alive(&self, metadata: &RequestMetadata) -> bool {
        self.liveness
            .is_alive(
                metadata.identity.requester_pid,
                metadata.identity.requester_start_ticks,
            )
            .unwrap_or(false)
    }

    fn activate_next(&mut self, state: &mut SplashState, now: u64) -> Result<(), NativeAgentError> {
        if self.active.is_some() {
            return Ok(());
        }
        while let Some(request) = self.queue.pop_front() {
            if request.metadata.deadline_micros <= now || !self.requester_alive(&request.metadata) {
                self.retire(request.metadata.identity);
                continue;
            }
            let presentation_id = self.next_presentation_id;
            self.next_presentation_id = self
                .next_presentation_id
                .checked_add(1)
                .ok_or(NativeAgentError::InvalidProtocol)?;
            let input = PromptInput::new(
                request.metadata.maximum_secret_bytes,
                request.metadata.echo,
                request.metadata.silent,
            )
            .map_err(|_| NativeAgentError::InvalidProtocol)?;
            let source = request.metadata.adapter.prompt_source();
            let metadata = PromptMetadata::new(presentation_id, request.metadata.prompt.clone())
                .and_then(|metadata| metadata.with_source(source))
                .map_err(|_| NativeAgentError::InvalidProtocol)?
                .with_requester_pid(request.metadata.identity.requester_pid)
                .with_echo(request.metadata.echo)
                .with_silent(request.metadata.silent)
                .with_expiry(request.metadata.deadline_micros / 1_000);
            state
                .apply(StateAction::BeginPrompt(metadata))
                .map_err(|_| NativeAgentError::InvalidProtocol)?;
            self.active = Some(NativeActivePrompt {
                request,
                presentation_id,
                input,
            });
            break;
        }
        Ok(())
    }

    fn submit(&mut self, state: &mut SplashState) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        let identity = active.request.metadata.identity;
        let result = match active.request.responder.take() {
            Some(responder) => active
                .input
                .finish_with(move |secret| responder.reply_secret(secret)),
            None => Err(NativeCredentialError::InvalidProtocol),
        };
        let outcome = if result.is_ok() {
            PromptOutcome::Answered
        } else {
            PromptOutcome::RequestGone
        };
        active.input.clear();
        let _ = state.apply(StateAction::FinishPrompt {
            request_id: active.presentation_id,
            outcome,
        });
        self.retire(identity);
    }

    fn cancel_by_user(&mut self, state: &mut SplashState) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        let identity = active.request.metadata.identity;
        let outcome = active
            .request
            .responder
            .take()
            .and_then(|responder| responder.reply_cancel().ok())
            .map_or(PromptOutcome::RequestGone, |()| PromptOutcome::Cancelled);
        active.input.clear();
        let _ = state.apply(StateAction::FinishPrompt {
            request_id: active.presentation_id,
            outcome,
        });
        self.retire(identity);
    }

    fn finish_active(&mut self, state: &mut SplashState, outcome: PromptOutcome) {
        if let Some(mut active) = self.active.take() {
            active.input.clear();
            self.retire(active.request.metadata.identity);
            let _ = state.apply(StateAction::FinishPrompt {
                request_id: active.presentation_id,
                outcome,
            });
        }
    }

    fn retire(&mut self, identity: RequestIdentity) {
        if self.retired.contains(&identity) {
            return;
        }
        if self.retired.len() == MAX_RETIRED_IDENTITIES {
            self.retired.pop_front();
        }
        self.retired.push_back(identity);
    }

    fn disable(&mut self, state: &mut SplashState) {
        self.enabled = false;
        self.source.close();
        self.queue.clear();
        self.finish_active(state, PromptOutcome::RequestGone);
    }
}

impl PromptCoordinator for NativePromptCoordinator {
    fn poll(&mut self, state: &mut SplashState) {
        if self.enabled && self.poll_inner(state).is_err() {
            self.disable(state);
        }
    }

    fn handle_input(&mut self, state: &mut SplashState, bytes: &mut [u8]) {
        let transient = TransientInput(bytes);
        if !self.enabled || self.active.is_none() {
            return;
        }
        for index in 0..transient.0.len() {
            let outcome = match self.active.as_mut() {
                Some(active) => active.input.feed_byte(transient.0[index]),
                None => break,
            };
            match outcome {
                InputOutcome::Submit => {
                    self.submit(state);
                    break;
                }
                InputOutcome::Cancelled => {
                    self.cancel_by_user(state);
                    break;
                }
                InputOutcome::Pending | InputOutcome::Changed(_) | InputOutcome::Rejected(_) => {}
            }
        }
    }

    fn feedback(&self) -> Option<InputFeedback> {
        self.active.as_ref().map(|active| active.input.feedback())
    }

    fn with_visible_text(&self, render: &mut dyn FnMut(&str)) {
        if let Some(active) = self.active.as_ref() {
            active.input.with_visible_text(|text| {
                if let Some(text) = text {
                    render(text);
                }
            });
        }
    }

    fn abandon(&mut self, state: &mut SplashState) {
        self.disable(state);
    }

    fn enabled(&self) -> bool {
        self.enabled
    }
}

impl Drop for NativePromptCoordinator {
    fn drop(&mut self) {
        self.source.close();
        self.queue.clear();
        if let Some(active) = self.active.as_mut() {
            active.input.clear();
        }
    }
}

struct TransientInput<'a>(&'a mut [u8]);

impl Drop for TransientInput<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::{Ordering, compiler_fence};
        compiler_fence(Ordering::SeqCst);
        for byte in self.0.iter_mut() {
            // SAFETY: byte is uniquely borrowed and points to live input
            // storage. Volatile clearing remains observable to the optimizer.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

/// Run the hidden same-ELF native client against the production runtime.
///
/// The caller must transfer ownership of fixed inherited fd 8. Any failure is
/// reported as `ConsoleFallback`, while explicit user cancellation remains a
/// distinct outcome. This boundary intentionally emits no stdout, stderr,
/// logs, or protocol error text.
pub fn run_native_askpass_client(
    adapter: NativeAdapter,
    metadata: &PipeAskpassMetadata,
    output: OwnedFd,
) -> NativeAskpassClientOutcome {
    let runtime = RuntimePaths::production();
    run_native_askpass_client_at(
        adapter,
        metadata,
        output,
        runtime.native_password_socket(),
        runtime.required_daemon_uid(),
        NATIVE_ASKPASS_TIMEOUT,
        &LinuxProcessSecretPolicy,
    )
}

#[doc(hidden)]
pub fn run_native_askpass_client_at(
    adapter: NativeAdapter,
    metadata: &PipeAskpassMetadata,
    output: OwnedFd,
    socket_path: &Path,
    expected_daemon_uid: u32,
    timeout: Duration,
    policy: &dyn ProcessSecretPolicy,
) -> NativeAskpassClientOutcome {
    match run_native_client_inner(
        adapter,
        metadata,
        output,
        socket_path,
        expected_daemon_uid,
        timeout,
        policy,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            // This channel carries only bounded transport-class diagnostics;
            // prompt text and credential bytes are never included. Adapter
            // wrappers may route it to the kernel log before restoring the
            // stock console path.
            eprintln!("bootart native askpass unavailable: {error}");
            NativeAskpassClientOutcome::ConsoleFallback
        }
    }
}

fn run_native_client_inner(
    adapter: NativeAdapter,
    metadata: &PipeAskpassMetadata,
    output: OwnedFd,
    socket_path: &Path,
    expected_daemon_uid: u32,
    timeout: Duration,
    policy: &dyn ProcessSecretPolicy,
) -> Result<NativeAskpassClientOutcome, NativeAgentError> {
    validate_adapter_metadata(adapter, metadata)?;
    policy
        .protect_process()
        .map_err(|source| NativeAgentError::io("protect native askpass client", source))?;
    let pipe = InheritedSecretPipe::new(output)?;
    let now_micros = monotonic_micros()?;
    let lifetime_micros = u64::try_from(timeout.as_micros())
        .ok()
        .filter(|value| *value > 0 && *value <= MAX_REQUEST_LIFETIME_MICROS)
        .ok_or(NativeAgentError::InvalidDeadline)?;
    let deadline_micros = now_micros
        .checked_add(lifetime_micros)
        .ok_or(NativeAgentError::InvalidDeadline)?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(NativeAgentError::InvalidDeadline)?;

    let requester_pid = std::process::id();
    let requester_start_ticks = process_start_ticks(requester_pid)
        .map_err(|_| NativeAgentError::RequesterIdentityUnavailable)?;
    let generation = NEXT_REQUEST_GENERATION
        .fetch_add(1, Ordering::Relaxed)
        .max(1);
    let request_id =
        now_micros ^ u64::from(requester_pid).rotate_left(17) ^ generation.rotate_left(37);
    let request = RequestMetadata {
        adapter,
        identity: RequestIdentity {
            request_id: request_id.max(1),
            generation,
            requester_pid,
            requester_start_ticks,
        },
        deadline_micros,
        attempt: 1,
        attempts: metadata.attempts(),
        maximum_secret_bytes: metadata.maximum_secret_bytes(),
        prompt: metadata.prompt().to_owned(),
        echo: false,
        silent: false,
    };
    let packet = encode_request(&request)?;
    let carrier = connect_seqpacket(socket_path, deadline)?;
    let credentials = peer_credentials(carrier.as_raw_fd())
        .map_err(|source| NativeAgentError::io("authenticate native password daemon", source))?;
    if credentials.uid != expected_daemon_uid {
        return Err(NativeAgentError::WrongPeerUid {
            expected: expected_daemon_uid,
            actual: credentials.uid,
        });
    }
    let (credential, responder) = native_credential_pair()?;
    send_responder_packet(
        carrier.as_raw_fd(),
        expected_daemon_uid,
        &LinuxCredentialPeerAuthenticator,
        &packet,
        responder,
        deadline.saturating_duration_since(Instant::now()),
    )?;
    drop(carrier);

    poll_client_result(metadata, pipe, credential, deadline)
}

fn validate_adapter_metadata(
    adapter: NativeAdapter,
    metadata: &PipeAskpassMetadata,
) -> Result<(), NativeAgentError> {
    if metadata.framing() != adapter.secret_framing() {
        return Err(NativeAgentError::InvalidProtocol);
    }
    Ok(())
}

fn poll_client_result(
    metadata: &PipeAskpassMetadata,
    pipe: InheritedSecretPipe,
    credential: super::credential::NativeCredentialClient,
    deadline: Instant,
) -> Result<NativeAskpassClientOutcome, NativeAgentError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(NativeAgentError::ClientTimedOut);
        }
        let timeout = deadline.saturating_duration_since(now);
        let milliseconds = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptors = [
            libc::pollfd {
                fd: credential.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: pipe.as_raw_fd(),
                // A pipe write end reports consumer loss through POLLERR even
                // when no ordinary readiness events are requested.
                events: 0,
                revents: 0,
            },
        ];
        // SAFETY: descriptors references exactly two initialized pollfd values.
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as _,
                milliseconds,
            )
        };
        if result < 0 {
            let source = io::Error::last_os_error();
            if source.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(NativeAgentError::io("poll native askpass result", source));
        }
        if result == 0 {
            return Err(NativeAgentError::ClientTimedOut);
        }
        if descriptors[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(NativeAgentError::OutputConsumerGone);
        }
        if descriptors[0].revents & libc::POLLIN != 0 {
            return match pipe.forward_ready(metadata, credential)? {
                PipeAskpassDisposition::Delivered => Ok(NativeAskpassClientOutcome::Delivered),
                PipeAskpassDisposition::Cancelled => Ok(NativeAskpassClientOutcome::UserCancelled),
                PipeAskpassDisposition::FallbackRequired(error) => {
                    Err(NativeAgentError::Pipe(error))
                }
            };
        }
        if descriptors[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(NativeAgentError::Credential(
                NativeCredentialError::InvalidProtocol,
            ));
        }
    }
}

/// Claim the fixed inherited pipe descriptor. No path-taking equivalent is
/// provided.
pub fn claim_native_askpass_output() -> io::Result<OwnedFd> {
    // SAFETY: F_GETFD does not take a pointer and leaves the descriptor owned
    // by this process. A successful query proves fd 8 is open.
    if unsafe { libc::fcntl(NATIVE_ASKPASS_OUTPUT_FD, libc::F_GETFD) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if NATIVE_OUTPUT_CLAIMED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "native askpass output descriptor was already claimed",
        ));
    }
    // SAFETY: the hidden client is the sole owner of its inherited fd 8 copy;
    // converting it transfers that ownership into the close-on-drop boundary.
    Ok(unsafe { OwnedFd::from_raw_fd(NATIVE_ASKPASS_OUTPUT_FD) })
}

fn encode_request(metadata: &RequestMetadata) -> Result<Vec<u8>, NativeAgentError> {
    let prompt = metadata.prompt.as_bytes();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let attempts = PipeAskpassMetadata::new(
        metadata.prompt.clone(),
        metadata.attempts,
        metadata.maximum_secret_bytes,
        metadata.adapter.secret_framing(),
    )
    .map_err(NativeAgentError::Pipe)?;
    if metadata.attempt == 0 || metadata.attempt > attempts.attempts() {
        return Err(NativeAgentError::InvalidProtocol);
    }
    if metadata.identity.request_id == 0
        || metadata.identity.generation == 0
        || metadata.identity.requester_pid == 0
        || metadata.identity.requester_start_ticks == 0
    {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let mut flags = 0;
    if metadata.echo {
        flags |= FLAG_ECHO;
    }
    if metadata.silent {
        flags |= FLAG_SILENT;
    }
    if flags == KNOWN_FLAGS {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let prompt_length =
        u16::try_from(prompt.len()).map_err(|_| NativeAgentError::InvalidProtocol)?;
    let maximum = u16::try_from(metadata.maximum_secret_bytes)
        .map_err(|_| NativeAgentError::InvalidProtocol)?;
    let mut packet = vec![0_u8; REQUEST_HEADER_BYTES + prompt.len()];
    packet[..4].copy_from_slice(&REQUEST_MAGIC);
    packet[4] = REQUEST_VERSION;
    packet[5] = REQUEST_OPCODE;
    packet[6] = metadata.adapter as u8;
    packet[7] = flags;
    packet[8..16].copy_from_slice(&metadata.identity.request_id.to_be_bytes());
    packet[16..24].copy_from_slice(&metadata.identity.generation.to_be_bytes());
    packet[24..28].copy_from_slice(&metadata.identity.requester_pid.to_be_bytes());
    packet[28..36].copy_from_slice(&metadata.identity.requester_start_ticks.to_be_bytes());
    packet[36..44].copy_from_slice(&metadata.deadline_micros.to_be_bytes());
    packet[44..46].copy_from_slice(&metadata.attempt.to_be_bytes());
    packet[46..48].copy_from_slice(&metadata.attempts.to_be_bytes());
    packet[48..50].copy_from_slice(&maximum.to_be_bytes());
    packet[50..52].copy_from_slice(&prompt_length.to_be_bytes());
    packet[REQUEST_HEADER_BYTES..].copy_from_slice(prompt);
    Ok(packet)
}

fn decode_request(packet: &[u8], now_micros: u64) -> Result<RequestMetadata, NativeAgentError> {
    if packet.len() < REQUEST_HEADER_BYTES
        || packet[..4] != REQUEST_MAGIC
        || packet[4] != REQUEST_VERSION
        || packet[5] != REQUEST_OPCODE
        || packet[7] & !KNOWN_FLAGS != 0
        || packet[7] == KNOWN_FLAGS
    {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let prompt_length = usize::from(u16::from_be_bytes([packet[50], packet[51]]));
    if prompt_length == 0
        || prompt_length > MAX_PROMPT_BYTES
        || packet.len() != REQUEST_HEADER_BYTES + prompt_length
    {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let prompt = std::str::from_utf8(&packet[REQUEST_HEADER_BYTES..])
        .map_err(|_| NativeAgentError::InvalidProtocol)?
        .to_owned();
    let attempts = u16::from_be_bytes([packet[46], packet[47]]);
    let maximum_secret_bytes = usize::from(u16::from_be_bytes([packet[48], packet[49]]));
    let adapter = NativeAdapter::try_from(packet[6])?;
    PipeAskpassMetadata::new(
        prompt.clone(),
        attempts,
        maximum_secret_bytes,
        adapter.secret_framing(),
    )?;
    let attempt = u16::from_be_bytes([packet[44], packet[45]]);
    if attempt == 0 || attempt > attempts {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let identity = RequestIdentity {
        request_id: u64::from_be_bytes(packet[8..16].try_into().expect("fixed slice")),
        generation: u64::from_be_bytes(packet[16..24].try_into().expect("fixed slice")),
        requester_pid: u32::from_be_bytes(packet[24..28].try_into().expect("fixed slice")),
        requester_start_ticks: u64::from_be_bytes(packet[28..36].try_into().expect("fixed slice")),
    };
    if identity.request_id == 0
        || identity.generation == 0
        || identity.requester_pid == 0
        || identity.requester_start_ticks == 0
    {
        return Err(NativeAgentError::InvalidProtocol);
    }
    let deadline_micros = u64::from_be_bytes(packet[36..44].try_into().expect("fixed slice"));
    if deadline_micros <= now_micros
        || deadline_micros.saturating_sub(now_micros) > MAX_REQUEST_LIFETIME_MICROS
    {
        return Err(NativeAgentError::InvalidDeadline);
    }
    Ok(RequestMetadata {
        adapter,
        identity,
        deadline_micros,
        attempt,
        attempts,
        maximum_secret_bytes,
        prompt,
        echo: packet[7] & FLAG_ECHO != 0,
        silent: packet[7] & FLAG_SILENT != 0,
    })
}

fn monotonic_micros() -> Result<u64, NativeAgentError> {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: time points to writable storage for CLOCK_MONOTONIC.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(NativeAgentError::io(
            "read monotonic clock",
            io::Error::last_os_error(),
        ));
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| NativeAgentError::InvalidDeadline)?;
    let nanos = u64::try_from(time.tv_nsec).map_err(|_| NativeAgentError::InvalidDeadline)?;
    seconds
        .checked_mul(1_000_000)
        .and_then(|micros| micros.checked_add(nanos / 1_000))
        .ok_or(NativeAgentError::InvalidDeadline)
}

fn process_start_ticks(pid: u32) -> io::Result<u64> {
    let mut bytes = Vec::with_capacity(PROC_STAT_MAX_BYTES as usize);
    File::open(format!("/proc/{pid}/stat"))?
        .take(PROC_STAT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > PROC_STAT_MAX_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "process stat record exceeds bound",
        ));
    }
    let close = bytes
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process stat"))?;
    let remainder = std::str::from_utf8(bytes.get(close + 1..).unwrap_or_default())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid process stat"))?;
    // After the comm field, token zero is field 3 (state); starttime is field
    // 22, therefore token 19 in this remainder.
    remainder
        .split_ascii_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short process stat"))?
        .parse::<u64>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid process start time"))
}

fn socket_peer_gone(descriptor: RawFd) -> bool {
    let mut poll_fd = libc::pollfd {
        fd: descriptor,
        events: 0,
        revents: 0,
    };
    // SAFETY: poll_fd points to one initialized record and timeout zero never
    // blocks the daemon.
    let result = unsafe { libc::poll(&mut poll_fd, 1, 0) };
    result < 0 || poll_fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
}

fn connect_seqpacket(path: &Path, deadline: Instant) -> Result<OwnedFd, NativeAgentError> {
    let (address, length) = unix_address(path)
        .map_err(|source| NativeAgentError::io("encode native password socket", source))?;
    // SAFETY: socket has no pointer arguments and returns a newly owned fd.
    let descriptor = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if descriptor < 0 {
        return Err(NativeAgentError::io(
            "create native password carrier",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: socket returned a newly owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // SAFETY: address is initialized for the exact length supplied.
    let result = unsafe {
        libc::connect(
            descriptor.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    };
    if result == 0 {
        return Ok(descriptor);
    }
    let source = io::Error::last_os_error();
    if !matches!(source.raw_os_error(), Some(libc::EINPROGRESS)) {
        return Err(NativeAgentError::io(
            "connect native password carrier",
            source,
        ));
    }
    poll_connected(descriptor.as_raw_fd(), deadline)?;
    Ok(descriptor)
}

fn poll_connected(descriptor: RawFd, deadline: Instant) -> Result<(), NativeAgentError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(NativeAgentError::ClientTimedOut);
        }
        let milliseconds = deadline
            .saturating_duration_since(now)
            .as_millis()
            .clamp(1, i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: descriptor,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: poll_fd points to one initialized record.
        let result = unsafe { libc::poll(&mut poll_fd, 1, milliseconds) };
        if result == 0 {
            return Err(NativeAgentError::ClientTimedOut);
        }
        if result < 0 {
            let source = io::Error::last_os_error();
            if source.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(NativeAgentError::io(
                "poll native password connection",
                source,
            ));
        }
        let mut socket_error = 0;
        let mut length = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        // SAFETY: socket_error and length are writable option storage.
        if unsafe {
            libc::getsockopt(
                descriptor,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut socket_error as *mut libc::c_int).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(NativeAgentError::io(
                "inspect native password connection",
                io::Error::last_os_error(),
            ));
        }
        if socket_error != 0 {
            return Err(NativeAgentError::io(
                "connect native password carrier",
                io::Error::from_raw_os_error(socket_error),
            ));
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{NativeCredentialOutcome, SecureSecret};
    use crate::splash::state::{Mode, View};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    type FakeBatches = Rc<RefCell<VecDeque<Result<Vec<NativePromptRequest>, NativeAgentError>>>>;

    struct FakeSource {
        batches: FakeBatches,
        closed: Rc<Cell<bool>>,
    }

    impl NativeRequestSource for FakeSource {
        fn poll_requests(
            &mut self,
            _now_micros: u64,
        ) -> Result<Vec<NativePromptRequest>, NativeAgentError> {
            self.batches
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Ok(Vec::new()))
        }

        fn close(&mut self) {
            self.closed.set(true);
        }
    }

    struct FakeClock(Rc<Cell<u64>>);

    impl NativeClock for FakeClock {
        fn now_micros(&self) -> Result<u64, NativeAgentError> {
            Ok(self.0.get())
        }
    }

    struct FakeLiveness {
        alive: Rc<Cell<bool>>,
        expected_start: u64,
    }

    impl NativeRequesterLiveness for FakeLiveness {
        fn is_alive(&self, _pid: u32, start_ticks: u64) -> Result<bool, NativeAgentError> {
            Ok(self.alive.get() && start_ticks == self.expected_start)
        }
    }

    fn request_metadata(request_id: u64, generation: u64, deadline: u64) -> RequestMetadata {
        RequestMetadata {
            adapter: NativeAdapter::DracutClassic,
            identity: RequestIdentity {
                request_id,
                generation,
                requester_pid: 41,
                requester_start_ticks: 9001,
            },
            deadline_micros: deadline,
            attempt: 1,
            attempts: 5,
            maximum_secret_bytes: 128,
            prompt: "Password (/dev/vda2)".to_owned(),
            echo: false,
            silent: false,
        }
    }

    fn request(
        request_id: u64,
        generation: u64,
        deadline: u64,
    ) -> (
        super::super::credential::NativeCredentialClient,
        NativePromptRequest,
    ) {
        let (client, responder) = native_credential_pair().expect("credential pair");
        (
            client,
            NativePromptRequest {
                metadata: request_metadata(request_id, generation, deadline),
                responder: Some(responder),
            },
        )
    }

    type CoordinatorFixture = (
        NativePromptCoordinator,
        SplashState,
        Rc<RefCell<VecDeque<Result<Vec<NativePromptRequest>, NativeAgentError>>>>,
        Rc<Cell<u64>>,
        Rc<Cell<bool>>,
        Rc<Cell<bool>>,
    );

    fn coordinator_fixture() -> CoordinatorFixture {
        let batches = Rc::new(RefCell::new(VecDeque::new()));
        let now = Rc::new(Cell::new(1_000_000));
        let alive = Rc::new(Cell::new(true));
        let closed = Rc::new(Cell::new(false));
        let coordinator = NativePromptCoordinator::with_components(
            Box::new(FakeSource {
                batches: Rc::clone(&batches),
                closed: Rc::clone(&closed),
            }),
            Box::new(FakeClock(Rc::clone(&now))),
            Box::new(FakeLiveness {
                alive: Rc::clone(&alive),
                expected_start: 9001,
            }),
        );
        let mut state = SplashState::new(Mode::Boot);
        state.apply(StateAction::MarkRunning).unwrap();
        (coordinator, state, batches, now, alive, closed)
    }

    #[test]
    fn metadata_codec_is_versioned_bounded_and_strict() {
        let metadata = request_metadata(7, 11, 2_000_000);
        let packet = encode_request(&metadata).expect("encode");
        assert_eq!(&packet[..4], b"BNAP");
        assert_eq!(packet[4], REQUEST_VERSION);
        assert_eq!(decode_request(&packet, 1_000_000).unwrap(), metadata);

        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(matches!(
            decode_request(&trailing, 1_000_000),
            Err(NativeAgentError::InvalidProtocol)
        ));
        let mut unknown_flags = packet.clone();
        unknown_flags[7] = 0x80;
        assert!(matches!(
            decode_request(&unknown_flags, 1_000_000),
            Err(NativeAgentError::InvalidProtocol)
        ));
        assert!(matches!(
            decode_request(&packet, 2_000_000),
            Err(NativeAgentError::InvalidDeadline)
        ));

        let mut initramfs_tools = request_metadata(8, 12, 2_000_000);
        initramfs_tools.adapter = NativeAdapter::InitramfsToolsBusybox;
        let packet = encode_request(&initramfs_tools).expect("encode exact-pipe adapter");
        assert_eq!(packet[6], NativeAdapter::InitramfsToolsBusybox as u8);
        assert_eq!(decode_request(&packet, 1_000_000).unwrap(), initramfs_tools);
        assert_eq!(
            NativeAdapter::DracutClassic.secret_framing(),
            PipeSecretFraming::NewlineTerminated
        );
        assert_eq!(
            NativeAdapter::InitramfsToolsBusybox.secret_framing(),
            PipeSecretFraming::Exact
        );
        assert_eq!(
            NativeAdapter::MkinitfsBusybox.secret_framing(),
            PipeSecretFraming::NewlineTerminated
        );
        assert_eq!(
            NativeAdapter::MkinitfsBootDeploy.secret_framing(),
            PipeSecretFraming::NewlineTerminated
        );
        assert_eq!(
            NativeAdapter::MkinitcpioBusybox.secret_framing(),
            PipeSecretFraming::Exact
        );

        let mut mkinitfs = request_metadata(9, 13, 2_000_000);
        mkinitfs.adapter = NativeAdapter::MkinitfsBusybox;
        let packet = encode_request(&mkinitfs).expect("encode mkinitfs pipe adapter");
        assert_eq!(packet[6], NativeAdapter::MkinitfsBusybox as u8);
        assert_eq!(decode_request(&packet, 1_000_000).unwrap(), mkinitfs);

        let mut boot_deploy = request_metadata(10, 14, 2_000_000);
        boot_deploy.adapter = NativeAdapter::MkinitfsBootDeploy;
        let packet = encode_request(&boot_deploy).expect("encode boot-deploy pipe adapter");
        assert_eq!(packet[6], NativeAdapter::MkinitfsBootDeploy as u8);
        assert_eq!(decode_request(&packet, 1_000_000).unwrap(), boot_deploy);

        let mut mkinitcpio = request_metadata(11, 15, 2_000_000);
        mkinitcpio.adapter = NativeAdapter::MkinitcpioBusybox;
        let packet = encode_request(&mkinitcpio).expect("encode mkinitcpio pipe adapter");
        assert_eq!(packet[6], NativeAdapter::MkinitcpioBusybox as u8);
        assert_eq!(decode_request(&packet, 1_000_000).unwrap(), mkinitcpio);

        let wrong_framing = PipeAskpassMetadata::new(
            "Please unlock disk cryptroot: ",
            1,
            1024,
            PipeSecretFraming::NewlineTerminated,
        )
        .unwrap();
        assert!(matches!(
            validate_adapter_metadata(NativeAdapter::InitramfsToolsBusybox, &wrong_framing),
            Err(NativeAgentError::InvalidProtocol)
        ));
    }

    #[test]
    fn coordinator_submits_secret_once_and_zeroes_transient_input() {
        let (mut coordinator, mut state, batches, _now, _alive, _closed) = coordinator_fixture();
        let (client, prompt) = request(1, 1, 2_000_000);
        batches.borrow_mut().push_back(Ok(vec![prompt]));

        coordinator.poll(&mut state);
        assert!(matches!(state.view(), View::Prompt { .. }));
        let mut input = *b"private\r";
        coordinator.handle_input(&mut state, &mut input);
        assert_eq!(input, [0; 8]);
        assert!(state.view().prompt().is_none());

        let outcome = client.receive(128).expect("credential reply");
        let mut secret = outcome.into_secret().expect("secret outcome");
        assert_eq!(secret.expose(|bytes| bytes.to_vec()), b"private");
        secret.clear();
    }

    #[test]
    fn explicit_cancel_is_distinct_but_timeout_and_pid_reuse_are_local() {
        let (mut coordinator, mut state, batches, now, alive, _closed) = coordinator_fixture();
        let (cancel_client, prompt) = request(2, 1, 2_000_000);
        batches.borrow_mut().push_back(Ok(vec![prompt]));
        coordinator.poll(&mut state);
        let mut cancel = [0x1b];
        coordinator.handle_input(&mut state, &mut cancel);
        assert!(
            cancel_client
                .receive(128)
                .expect("cancel packet")
                .is_cancelled()
        );

        let (timeout_client, prompt) = request(3, 1, 1_100_000);
        batches.borrow_mut().push_back(Ok(vec![prompt]));
        now.set(1_050_000);
        coordinator.poll(&mut state);
        assert!(state.view().prompt().is_some());
        now.set(1_200_000);
        coordinator.poll(&mut state);
        assert!(state.view().prompt().is_none());
        assert!(matches!(
            timeout_client.receive(128),
            Err(NativeCredentialError::InvalidProtocol)
        ));

        let (reuse_client, prompt) = request(4, 1, 2_000_000);
        batches.borrow_mut().push_back(Ok(vec![prompt]));
        now.set(1_300_000);
        alive.set(false);
        coordinator.poll(&mut state);
        assert!(state.view().prompt().is_none());
        assert!(matches!(
            reuse_client.receive(128),
            Err(NativeCredentialError::InvalidProtocol)
        ));
    }

    #[test]
    fn retired_identity_cannot_be_replayed_and_shutdown_closes_source() {
        let (mut coordinator, mut state, batches, now, _alive, closed) = coordinator_fixture();
        let (first_client, first) = request(9, 22, 3_000_000);
        batches.borrow_mut().push_back(Ok(vec![first]));
        coordinator.poll(&mut state);
        let mut cancel = [3];
        coordinator.handle_input(&mut state, &mut cancel);
        assert!(first_client.receive(128).unwrap().is_cancelled());

        let (replay_client, replay) = request(9, 22, 3_000_000);
        batches.borrow_mut().push_back(Ok(vec![replay]));
        now.set(1_200_000);
        coordinator.poll(&mut state);
        assert!(state.view().prompt().is_none());
        assert!(matches!(
            replay_client.receive(128),
            Err(NativeCredentialError::InvalidProtocol)
        ));

        coordinator.abandon(&mut state);
        assert!(closed.get());
        assert!(!coordinator.enabled());
    }

    #[test]
    fn credential_peer_hup_removes_request_without_cancel_packet() {
        let (mut coordinator, mut state, batches, _now, _alive, _closed) = coordinator_fixture();
        let (client, prompt) = request(10, 1, 2_000_000);
        drop(client);
        batches.borrow_mut().push_back(Ok(vec![prompt]));
        coordinator.poll(&mut state);
        assert!(state.view().prompt().is_none());
    }

    #[test]
    fn current_process_start_identity_is_parseable_and_nonzero() {
        assert!(process_start_ticks(std::process::id()).unwrap() > 0);
        assert!(monotonic_micros().unwrap() > 0);
    }

    #[test]
    fn responder_reply_is_one_shot_at_the_type_boundary() {
        let (client, responder) = native_credential_pair().unwrap();
        let mut secret = SecureSecret::new(32).unwrap();
        secret.push_str("one-shot").unwrap();
        responder.reply_secret(&mut secret).unwrap();
        assert!(secret.is_empty());
        assert!(matches!(
            client.receive(32).unwrap(),
            NativeCredentialOutcome::Secret(_)
        ));
    }
}
