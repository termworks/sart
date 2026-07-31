//! Injected coordinator for the experimental systemd password-agent adapter.
//!
//! The coordinator owns secret input and the dedicated reply path.  It exposes
//! only non-secret feedback to the splash engine and deliberately fails open:
//! an adapter error dismisses the local prompt without replying, leaving the
//! request available to another password agent (for example the console
//! agent).

use std::sync::atomic::{Ordering, compiler_fence};

use crate::splash::state::{PromptMetadata, PromptOutcome, SplashState, StateAction};

use super::input::{InputFeedback, InputOutcome, PromptInput};
use super::secure::{DEFAULT_SECRET_BYTES, SecureSecret};
use super::systemd_agent::{
    AgentError, AskQueue, AskRequest, AskRequestId, CancellationReason, InotifyWatcher,
    LinuxRequesterLiveness, MonotonicClock, QueueEvent, RequestDirectory, RequestWatcher,
    RequesterLiveness, SystemMonotonicClock, SystemdReplySocket,
};

const LIVENESS_INTERVAL_MICROS: u64 = 100_000;

/// Directory-snapshot seam used by deterministic coordinator tests.
pub trait AskRequestSource {
    fn scan_requests(&mut self) -> Result<Vec<AskRequest>, AgentError>;
}

impl AskRequestSource for RequestDirectory {
    fn scan_requests(&mut self) -> Result<Vec<AskRequest>, AgentError> {
        self.scan().map(|result| result.into_requests())
    }
}

/// Dedicated systemd reply transport.  Implementations must not retain secret
/// bytes passed to `send_success`.
pub trait SystemdReplyTransport {
    fn send_success(
        &mut self,
        request: &AskRequest,
        secret: &mut SecureSecret,
    ) -> Result<(), AgentError>;

    fn send_cancel(&mut self, request: &AskRequest) -> Result<(), AgentError>;
}

impl SystemdReplyTransport for SystemdReplySocket {
    fn send_success(
        &mut self,
        request: &AskRequest,
        secret: &mut SecureSecret,
    ) -> Result<(), AgentError> {
        SystemdReplySocket::send_success(self, request, secret)
    }

    fn send_cancel(&mut self, request: &AskRequest) -> Result<(), AgentError> {
        SystemdReplySocket::send_cancel(self, request)
    }
}

/// The narrow password-prompt surface consumed by the daemon and renderer.
///
/// No method returns or stores plaintext in presentation state.  Visible echo
/// is exposed only during a renderer-owned callback.
pub trait PromptCoordinator {
    /// Poll nonblocking request sources and update prompt metadata.
    fn poll(&mut self, state: &mut SplashState);

    /// Consume transient VT bytes. Implementations zero the supplied buffer on
    /// every return path.
    fn handle_input(&mut self, state: &mut SplashState, bytes: &mut [u8]);

    fn feedback(&self) -> Option<InputFeedback>;

    fn with_visible_text(&self, render: &mut dyn FnMut(&str));

    /// Clear and dismiss locally without sending systemd's `-` marker.
    fn abandon(&mut self, state: &mut SplashState);

    fn enabled(&self) -> bool;
}

struct ActivePrompt {
    request_id: AskRequestId,
    presentation_id: u64,
    input: PromptInput,
}

/// Experimental, injected systemd password prompt coordinator.
///
/// This type intentionally has no `Debug` or `Clone` implementation because it
/// owns a [`PromptInput`].
pub struct SystemdPromptCoordinator {
    source: Box<dyn AskRequestSource>,
    watcher: Box<dyn RequestWatcher>,
    clock: Box<dyn MonotonicClock>,
    liveness: Box<dyn RequesterLiveness>,
    reply: Box<dyn SystemdReplyTransport>,
    queue: AskQueue,
    active: Option<ActivePrompt>,
    initial_scan: bool,
    next_liveness_micros: u64,
    next_presentation_id: u64,
    enabled: bool,
}

impl SystemdPromptCoordinator {
    /// Open production systemd adapter resources.  Construction failure is
    /// expected to be handled by disabling only this adapter.
    pub fn open_system() -> Result<Self, AgentError> {
        Ok(Self::with_components(
            Box::new(RequestDirectory::open_system()?),
            Box::new(InotifyWatcher::open_system()?),
            Box::new(SystemMonotonicClock),
            Box::new(LinuxRequesterLiveness),
            Box::new(SystemdReplySocket::new()?),
        ))
    }

    /// Inject all effectful boundaries.  Public for pure integration tests;
    /// selecting this adapter still remains experimental until VM gates pass.
    pub fn with_components(
        source: Box<dyn AskRequestSource>,
        watcher: Box<dyn RequestWatcher>,
        clock: Box<dyn MonotonicClock>,
        liveness: Box<dyn RequesterLiveness>,
        reply: Box<dyn SystemdReplyTransport>,
    ) -> Self {
        Self {
            source,
            watcher,
            clock,
            liveness,
            reply,
            queue: AskQueue::new(),
            active: None,
            initial_scan: true,
            next_liveness_micros: 0,
            next_presentation_id: 1,
            enabled: true,
        }
    }

    fn poll_inner(&mut self, state: &mut SplashState) -> Result<(), CoordinatorFailure> {
        let batch = self
            .watcher
            .drain()
            .map_err(|_| CoordinatorFailure::Agent)?;
        let now = self
            .clock
            .now_micros()
            .map_err(|_| CoordinatorFailure::Agent)?;

        let events = if self.initial_scan || batch.rescan() || batch.overflowed() {
            let requests = self
                .source
                .scan_requests()
                .map_err(|_| CoordinatorFailure::Agent)?;
            self.initial_scan = false;
            self.next_liveness_micros = now.saturating_add(LIVENESS_INTERVAL_MICROS);
            self.queue
                .reconcile(requests, now, self.liveness.as_ref())
                .map_err(|_| CoordinatorFailure::Agent)?
        } else if now >= self.next_liveness_micros {
            self.next_liveness_micros = now.saturating_add(LIVENESS_INTERVAL_MICROS);
            self.queue
                .tick(now, self.liveness.as_ref())
                .map_err(|_| CoordinatorFailure::Agent)?
        } else {
            Vec::new()
        };

        self.apply_events(state, events)
    }

    fn apply_events(
        &mut self,
        state: &mut SplashState,
        events: Vec<QueueEvent>,
    ) -> Result<(), CoordinatorFailure> {
        for event in events {
            match event {
                QueueEvent::Activated(descriptor) => {
                    if self.active.is_some() {
                        return Err(CoordinatorFailure::Invariant);
                    }
                    let presentation_id = self.next_presentation_id;
                    self.next_presentation_id = self
                        .next_presentation_id
                        .checked_add(1)
                        .ok_or(CoordinatorFailure::Invariant)?;
                    let input = PromptInput::new(
                        DEFAULT_SECRET_BYTES,
                        descriptor.echo(),
                        descriptor.silent(),
                    )
                    .map_err(|_| CoordinatorFailure::SecretAllocation)?;
                    let mut metadata = PromptMetadata::new(presentation_id, descriptor.message())
                        .and_then(|metadata| metadata.with_source("systemd"))
                        .map_err(|_| CoordinatorFailure::InvalidMetadata)?
                        .with_requester_pid(descriptor.requester_pid())
                        .with_echo(descriptor.echo())
                        .with_silent(descriptor.silent());
                    if descriptor.not_after_micros() != 0 {
                        metadata = metadata.with_expiry(descriptor.not_after_micros() / 1_000);
                    }
                    state
                        .apply(StateAction::BeginPrompt(metadata))
                        .map_err(|_| CoordinatorFailure::State)?;
                    self.active = Some(ActivePrompt {
                        request_id: descriptor.id().clone(),
                        presentation_id,
                        input,
                    });
                }
                QueueEvent::Dismissed { id, reason } => {
                    let Some(mut active) = self.active.take() else {
                        return Err(CoordinatorFailure::Invariant);
                    };
                    if active.request_id != id {
                        active.input.clear();
                        return Err(CoordinatorFailure::Invariant);
                    }
                    active.input.clear();
                    state
                        .apply(StateAction::FinishPrompt {
                            request_id: active.presentation_id,
                            outcome: prompt_outcome(reason),
                        })
                        .map_err(|_| CoordinatorFailure::State)?;
                }
            }
        }
        Ok(())
    }

    fn submit(&mut self, state: &mut SplashState) -> Result<(), CoordinatorFailure> {
        let request = self
            .queue
            .active_request()
            .cloned()
            .ok_or(CoordinatorFailure::Invariant)?;
        let active = self.active.as_mut().ok_or(CoordinatorFailure::Invariant)?;
        let reply = self.reply.as_mut();
        active
            .input
            .finish_with(|secret| reply.send_success(&request, secret))
            .map_err(|_| CoordinatorFailure::Agent)?;
        // A systemd requester keeps the same ask.* identity and reply socket
        // alive while cryptsetup rejects a wrong passphrase and asks again.
        // Sending one answer therefore clears only this input attempt; the
        // prompt stays active and can deliver another datagram. The request
        // file's deletion (normally after a correct answer) is the
        // authenticated completion signal that retires the prompt.
        debug_assert!(state.view().prompt().is_some());
        Ok(())
    }

    fn cancel_by_user(&mut self, state: &mut SplashState) -> Result<(), CoordinatorFailure> {
        let request = self
            .queue
            .active_request()
            .cloned()
            .ok_or(CoordinatorFailure::Invariant)?;
        self.reply
            .send_cancel(&request)
            .map_err(|_| CoordinatorFailure::Agent)?;
        let events = self
            .queue
            .complete_active(CancellationReason::UserCancelled)
            .map_err(|_| CoordinatorFailure::Agent)?;
        self.apply_events(state, events)
    }

    fn disable(&mut self, state: &mut SplashState) {
        self.enabled = false;
        self.dismiss_locally(state);
    }

    fn dismiss_locally(&mut self, state: &mut SplashState) {
        if let Some(mut active) = self.active.take() {
            active.input.clear();
            let _ = state.apply(StateAction::FinishPrompt {
                request_id: active.presentation_id,
                outcome: PromptOutcome::RequestGone,
            });
        }
    }
}

impl PromptCoordinator for SystemdPromptCoordinator {
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
            let result = match outcome {
                InputOutcome::Submit => self.submit(state),
                InputOutcome::Cancelled => self.cancel_by_user(state),
                InputOutcome::Pending | InputOutcome::Changed(_) | InputOutcome::Rejected(_) => {
                    continue;
                }
            };
            if result.is_err() {
                self.disable(state);
            }
            break;
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

impl Drop for SystemdPromptCoordinator {
    fn drop(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.input.clear();
        }
    }
}

fn prompt_outcome(reason: CancellationReason) -> PromptOutcome {
    match reason {
        CancellationReason::Answered => PromptOutcome::Answered,
        CancellationReason::UserCancelled => PromptOutcome::Cancelled,
        CancellationReason::Expired => PromptOutcome::TimedOut,
        CancellationReason::Deleted
        | CancellationReason::RequesterGone
        | CancellationReason::ReplyFailed => PromptOutcome::RequestGone,
    }
}

struct TransientInput<'a>(&'a mut [u8]);

impl Drop for TransientInput<'_> {
    fn drop(&mut self) {
        compiler_fence(Ordering::SeqCst);
        for byte in self.0.iter_mut() {
            // SAFETY: `byte` is a valid uniquely borrowed byte. Volatile writes
            // keep the transient input clearing observable to the optimizer.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

enum CoordinatorFailure {
    Agent,
    SecretAllocation,
    InvalidMetadata,
    State,
    Invariant,
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeSet, VecDeque};
    use std::io;
    use std::os::fd::{AsRawFd, RawFd};
    use std::rc::Rc;

    use crate::splash::command::state_json;
    use crate::splash::state::{Mode, View};

    use super::*;
    use crate::password::systemd_agent::{AskRequestId, WatchBatch};

    #[derive(Default)]
    struct SharedSource(Rc<RefCell<Vec<AskRequest>>>);

    impl AskRequestSource for SharedSource {
        fn scan_requests(&mut self) -> Result<Vec<AskRequest>, AgentError> {
            Ok(self.0.borrow().clone())
        }
    }

    struct FakeWatcher {
        batches: VecDeque<Result<WatchBatch, AgentError>>,
    }

    impl AsRawFd for FakeWatcher {
        fn as_raw_fd(&self) -> RawFd {
            -1
        }
    }

    impl RequestWatcher for FakeWatcher {
        fn drain(&mut self) -> Result<WatchBatch, AgentError> {
            self.batches
                .pop_front()
                .unwrap_or_else(|| Ok(WatchBatch::default()))
        }
    }

    #[derive(Default)]
    struct FakeClock(Rc<Cell<u64>>);

    impl MonotonicClock for FakeClock {
        fn now_micros(&self) -> Result<u64, AgentError> {
            Ok(self.0.get())
        }
    }

    #[derive(Default)]
    struct SharedLiveness(Rc<RefCell<BTreeSet<u32>>>);

    impl RequesterLiveness for SharedLiveness {
        fn is_alive(&self, pid: u32) -> io::Result<bool> {
            Ok(!self.0.borrow().contains(&pid))
        }
    }

    #[derive(Default)]
    struct ReplyRecord {
        successes: Vec<usize>,
        cancels: usize,
        fail_success: bool,
        fail_cancel: bool,
    }

    struct SharedReply(Rc<RefCell<ReplyRecord>>);

    impl SystemdReplyTransport for SharedReply {
        fn send_success(
            &mut self,
            _request: &AskRequest,
            secret: &mut SecureSecret,
        ) -> Result<(), AgentError> {
            let mut record = self.0.borrow_mut();
            if record.fail_success {
                return Err(AgentError::Reply(io::Error::other("injected failure")));
            }
            record.successes.push(secret.expose(|bytes| bytes.len()));
            Ok(())
        }

        fn send_cancel(&mut self, _request: &AskRequest) -> Result<(), AgentError> {
            let mut record = self.0.borrow_mut();
            if record.fail_cancel {
                return Err(AgentError::Reply(io::Error::other("injected failure")));
            }
            record.cancels += 1;
            Ok(())
        }
    }

    struct Harness {
        coordinator: SystemdPromptCoordinator,
        source: Rc<RefCell<Vec<AskRequest>>>,
        time: Rc<Cell<u64>>,
        gone: Rc<RefCell<BTreeSet<u32>>>,
        replies: Rc<RefCell<ReplyRecord>>,
        state: SplashState,
    }

    impl Harness {
        fn new(requests: Vec<AskRequest>) -> Self {
            let source = Rc::new(RefCell::new(requests));
            let time = Rc::new(Cell::new(100));
            let gone = Rc::new(RefCell::new(BTreeSet::new()));
            let replies = Rc::new(RefCell::new(ReplyRecord::default()));
            let coordinator = SystemdPromptCoordinator::with_components(
                Box::new(SharedSource(Rc::clone(&source))),
                Box::new(FakeWatcher {
                    batches: VecDeque::new(),
                }),
                Box::new(FakeClock(Rc::clone(&time))),
                Box::new(SharedLiveness(Rc::clone(&gone))),
                Box::new(SharedReply(Rc::clone(&replies))),
            );
            let mut state = SplashState::new(Mode::Boot);
            state.apply(StateAction::MarkRunning).unwrap();
            Self {
                coordinator,
                source,
                time,
                gone,
                replies,
                state,
            }
        }

        fn activate(&mut self) {
            self.coordinator.poll(&mut self.state);
            assert!(matches!(self.state.view(), View::Prompt { .. }));
        }

        fn input(&mut self, input: &[u8]) {
            let mut bytes = input.to_vec();
            self.coordinator.handle_input(&mut self.state, &mut bytes);
            assert!(bytes.iter().all(|byte| *byte == 0));
        }
    }

    fn request(name: &str, pid: u32, not_after: u64) -> AskRequest {
        let source = format!(
            "[Ask]\nMessage=Unlock volume\nPID={pid}\nSocket=/run/systemd/ask-password/sck.test\nEcho=0\nSilent=0\nNotAfter={not_after}\n"
        );
        AskRequest::parse(
            AskRequestId::new(name, 1, u64::from(pid)).unwrap(),
            source.as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn submission_uses_dedicated_reply_and_never_enters_state() {
        let mut harness = Harness::new(vec![request("ask.a", 10, 0)]);
        harness.activate();
        harness.input(b"correct horse\n");

        assert_eq!(harness.replies.borrow().successes, [13]);
        assert_eq!(harness.replies.borrow().cancels, 0);
        assert!(matches!(harness.state.view(), View::Prompt { .. }));
        assert_eq!(harness.coordinator.feedback().unwrap().character_count(), 0);
        let state = state_json(&harness.state);
        assert!(!state.contains("correct"));
        assert!(!format!("{:?}", harness.state).contains("correct"));
    }

    #[test]
    fn only_explicit_user_cancel_sends_negative_reply() {
        let mut harness = Harness::new(vec![request("ask.a", 10, 0)]);
        harness.activate();
        harness.input(&[0x1b]);

        assert_eq!(harness.replies.borrow().cancels, 1);
        assert!(matches!(harness.state.view(), View::Splash));
    }

    #[test]
    fn deletion_expiry_requester_death_and_shutdown_do_not_reply() {
        let mut deleted = Harness::new(vec![request("ask.a", 10, 0)]);
        deleted.activate();
        deleted.source.borrow_mut().clear();
        deleted.coordinator.initial_scan = true;
        deleted.coordinator.poll(&mut deleted.state);
        assert_eq!(deleted.replies.borrow().cancels, 0);

        let mut expired = Harness::new(vec![request("ask.b", 11, 500)]);
        expired.activate();
        expired.time.set(200_000);
        expired.coordinator.poll(&mut expired.state);
        assert_eq!(expired.replies.borrow().cancels, 0);

        let mut gone = Harness::new(vec![request("ask.c", 12, 0)]);
        gone.activate();
        gone.gone.borrow_mut().insert(12);
        gone.time.set(200_000);
        gone.coordinator.poll(&mut gone.state);
        assert_eq!(gone.replies.borrow().cancels, 0);

        let mut shutdown = Harness::new(vec![request("ask.d", 13, 0)]);
        shutdown.activate();
        shutdown.coordinator.abandon(&mut shutdown.state);
        assert_eq!(shutdown.replies.borrow().cancels, 0);
        assert!(matches!(shutdown.state.view(), View::Splash));
    }

    #[test]
    fn reply_failure_clears_and_leaves_request_for_console_fallback() {
        let mut harness = Harness::new(vec![request("ask.a", 10, 0)]);
        harness.activate();
        harness.replies.borrow_mut().fail_success = true;
        harness.input(b"wrong\n");

        assert!(!harness.coordinator.enabled());
        assert_eq!(harness.replies.borrow().cancels, 0);
        assert!(matches!(harness.state.view(), View::Splash));
    }

    #[test]
    fn request_remains_promptable_for_retry_until_its_identity_disappears() {
        let mut harness = Harness::new(vec![request("ask.b", 11, 0), request("ask.a", 10, 0)]);
        harness.activate();
        assert_eq!(
            harness.state.view().prompt().unwrap().requester_pid(),
            Some(10)
        );
        harness.input(b"one\n");
        assert_eq!(
            harness.state.view().prompt().unwrap().requester_pid(),
            Some(10)
        );
        assert_eq!(harness.replies.borrow().successes, [3]);

        harness.source.borrow_mut().remove(1);
        harness.coordinator.initial_scan = true;
        harness.coordinator.poll(&mut harness.state);
        assert_eq!(
            harness.state.view().prompt().unwrap().requester_pid(),
            Some(11)
        );
        harness.input(b"two\n");
        assert_eq!(harness.replies.borrow().successes, [3, 3]);
        assert!(matches!(harness.state.view(), View::Prompt { .. }));
    }

    #[test]
    fn watcher_failure_disables_only_adapter_and_keeps_splash_running() {
        let source = Rc::new(RefCell::new(vec![request("ask.a", 10, 0)]));
        let replies = Rc::new(RefCell::new(ReplyRecord::default()));
        let mut coordinator = SystemdPromptCoordinator::with_components(
            Box::new(SharedSource(source)),
            Box::new(FakeWatcher {
                batches: VecDeque::from([Err(AgentError::WatchCorrupt)]),
            }),
            Box::new(FakeClock::default()),
            Box::new(SharedLiveness::default()),
            Box::new(SharedReply(Rc::clone(&replies))),
        );
        let mut state = SplashState::new(Mode::Boot);
        state.apply(StateAction::MarkRunning).unwrap();

        coordinator.poll(&mut state);

        assert!(!coordinator.enabled());
        assert_eq!(state.lifecycle(), crate::splash::state::Lifecycle::Running);
        assert!(matches!(state.view(), View::Splash));
        assert!(replies.borrow().successes.is_empty());
        assert_eq!(replies.borrow().cancels, 0);
    }
}
