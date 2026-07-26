use super::protocol::{Frame, Opcode, ProtocolError};
use super::root_transition::{DeferredRootTransition, RootTransition, RootTransitionError};
use super::state::{Lifecycle, Mode, RootStage, SplashState, StateAction, View};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub response: Frame,
    pub should_quit: bool,
    /// Keep the last splash pixels when the backend can do so without keeping
    /// terminal, keyboard, KD, or VT ownership.
    pub retain_splash: bool,
    /// A root transition failure. The daemon must restore presentation state
    /// and terminate rather than risk retaining the VT after its handoff unit
    /// has already allowed boot to continue. `rollback_incomplete()` further
    /// indicates that the process filesystem namespace is unknown.
    pub fatal_root_transition: Option<RootTransitionError>,
}

pub fn is_mutating(opcode: Opcode) -> bool {
    !matches!(opcode, Opcode::Ping | Opcode::State | Opcode::NativeReady)
}

pub fn handle_request(
    state: &mut SplashState,
    request: &Frame,
) -> Result<CommandOutcome, ProtocolError> {
    let mut transition = DeferredRootTransition;
    handle_request_with_root_transition(state, request, &mut transition)
}

pub fn handle_request_with_root_transition(
    state: &mut SplashState,
    request: &Frame,
    root_transition: &mut dyn RootTransition,
) -> Result<CommandOutcome, ProtocolError> {
    let request_id = request.request_id();
    let mut should_quit = false;
    let mut retain_splash = false;

    let action = match request.opcode() {
        Opcode::Ping => {
            return Ok(CommandOutcome {
                response: Frame::pong(request_id),
                should_quit: false,
                retain_splash: false,
                fatal_root_transition: None,
            });
        }
        Opcode::State => {
            return Ok(CommandOutcome {
                response: Frame::state_result(request_id, state_json(state))?,
                should_quit: false,
                retain_splash: false,
                fatal_root_transition: None,
            });
        }
        Opcode::NativeReady => {
            // Readiness is a daemon-scoped capability: only the event loop can
            // prove that the prepared native broker still has an enabled
            // coordinator. Pure command handling must never guess from socket
            // or filesystem presence.
            return Ok(CommandOutcome {
                response: Frame::error(
                    request_id,
                    "native password broker is unavailable in this command context",
                )?,
                should_quit: false,
                retain_splash: false,
                fatal_root_transition: None,
            });
        }
        Opcode::Show => Some(StateAction::Show),
        Opcode::Hide => Some(StateAction::Hide),
        Opcode::Status => {
            let text = request.payload_text()?;
            Some(StateAction::SetStatus(
                (!text.is_empty()).then(|| text.to_owned()),
            ))
        }
        Opcode::Progress => Some(StateAction::SetProgress(request.progress_value())),
        Opcode::Message => Some(StateAction::SetMessage(Some(
            request.payload_text()?.to_owned(),
        ))),
        Opcode::HideMessage => {
            let requested = request.payload_text()?;
            if requested.is_empty() || state.message() == Some(requested) {
                Some(StateAction::SetMessage(None))
            } else {
                None
            }
        }
        Opcode::DetailsShow => Some(StateAction::ShowDetails),
        Opcode::DetailsHide => Some(StateAction::HideDetails),
        Opcode::DetailsToggle => Some(StateAction::ToggleDetails),
        Opcode::Deactivate => Some(StateAction::Deactivate),
        Opcode::Reactivate => Some(StateAction::Reactivate),
        Opcode::SetMode => Some(StateAction::SetMode(
            request
                .mode_value()
                .expect("validated mode requests contain a mode"),
        )),
        Opcode::UpdateRootFs => {
            let validated_path = request.payload_text()?;
            if state.root_stage() == RootStage::Initramfs
                && let Err(error) = state.apply(StateAction::SetRootStage(RootStage::Switching))
            {
                let response = Frame::error(request_id, error.to_string())?;
                // The authenticated initramfs handoff job is best-effort and
                // boot is allowed to continue after it returns. Even a
                // pre-transition rejection (most importantly an active
                // prompt) must therefore stop presentation ownership: a later
                // real-root client is not guaranteed to remain able to reach
                // this initramfs namespace. The error response is deferred by
                // the daemon until display/runtime cleanup completes.
                let _ = state.apply(StateAction::FailOpen);
                return Ok(CommandOutcome {
                    response,
                    should_quit: true,
                    retain_splash: false,
                    fatal_root_transition: None,
                });
            }
            if state.root_stage() == RootStage::Switching {
                if let Err(error) = root_transition.transition(Path::new(validated_path)) {
                    let response = Frame::error(request_id, error.to_string())?;
                    // A fully rolled-back chroot is safe at the filesystem
                    // layer, but it is not safe to keep owning the display:
                    // init will continue past this best-effort handoff and no
                    // later real-root client is guaranteed to reach us. Every
                    // transition failure therefore takes the same fail-open
                    // restoration path. Incomplete rollback remains encoded in
                    // the error for diagnostics.
                    let _ = state.apply(StateAction::FailOpen);
                    return Ok(CommandOutcome {
                        response,
                        should_quit: true,
                        retain_splash: false,
                        fatal_root_transition: Some(error),
                    });
                }
                Some(StateAction::SetRootStage(RootStage::RealRoot))
            } else {
                None
            }
        }
        Opcode::Quit => {
            should_quit = true;
            retain_splash = request.retains_splash();
            Some(StateAction::Quit)
        }
        Opcode::Ack | Opcode::Error | Opcode::Pong | Opcode::StateResult => {
            return Ok(CommandOutcome {
                response: Frame::error(request_id, "response opcode is invalid in a request")?,
                should_quit: false,
                retain_splash: false,
                fatal_root_transition: None,
            });
        }
    };

    if let Some(action) = action
        && let Err(error) = state.apply(action)
    {
        return state_error(request_id, error);
    }

    Ok(CommandOutcome {
        response: Frame::ack(request_id),
        should_quit,
        retain_splash,
        fatal_root_transition: None,
    })
}

fn state_error(
    request_id: u64,
    error: impl std::fmt::Display,
) -> Result<CommandOutcome, ProtocolError> {
    Ok(CommandOutcome {
        response: Frame::error(request_id, error.to_string())?,
        should_quit: false,
        retain_splash: false,
        fatal_root_transition: None,
    })
}

pub fn state_json(state: &SplashState) -> String {
    let lifecycle = match state.lifecycle() {
        Lifecycle::Starting => "starting",
        Lifecycle::Running => "running",
        Lifecycle::Deactivated => "deactivated",
        Lifecycle::Quitting => "quitting",
        Lifecycle::Stopped => "stopped",
        Lifecycle::FailedOpen => "failed-open",
    };
    let view = match state.view() {
        View::Hidden => "hidden",
        View::Splash => "splash",
        View::Details => "details",
        View::Prompt { .. } => "prompt",
    };
    let mode = mode_name(state.mode());
    let root_stage = match state.root_stage() {
        RootStage::Initramfs => "initramfs",
        RootStage::Switching => "switching",
        RootStage::RealRoot => "real-root",
    };

    format!(
        "{{\"lifecycle\":\"{lifecycle}\",\"view\":\"{view}\",\"mode\":\"{mode}\",\"root_stage\":\"{root_stage}\",\"status\":{},\"message\":{},\"progress\":{}}}",
        json_optional_string(state.status()),
        json_optional_string(state.message()),
        state
            .progress()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_owned()),
    )
}

pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Boot => "boot",
        Mode::Shutdown => "shutdown",
        Mode::Reboot => "reboot",
        Mode::Update => "update",
        Mode::Upgrade => "upgrade",
    }
}

fn json_optional_string(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", json_escape(value)),
        None => "null".to_owned(),
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splash::root_transition::{RootTransitionError, SystemFailure};
    use crate::splash::state::PromptMetadata;
    use std::io;
    use std::path::{Path, PathBuf};

    fn running_state() -> SplashState {
        let mut state = SplashState::default();
        state.apply(StateAction::MarkRunning).unwrap();
        state
    }

    #[test]
    fn request_mapping_updates_only_presentation_state() {
        let mut state = running_state();
        handle_request(
            &mut state,
            &Frame::text(Opcode::Status, 1, "Mounting filesystems").unwrap(),
        )
        .unwrap();
        handle_request(
            &mut state,
            &Frame::text(Opcode::UpdateRootFs, 2, "/path/that/need/not/exist").unwrap(),
        )
        .unwrap();

        assert_eq!(state.status(), Some("Mounting filesystems"));
        assert_eq!(state.root_stage(), RootStage::RealRoot);
    }

    #[test]
    fn json_escapes_text_and_has_no_secret_field() {
        let mut state = running_state();
        state
            .apply(StateAction::SetMessage(Some(
                "quote: \" and slash: \\".into(),
            )))
            .unwrap();
        let json = state_json(&state);

        assert!(json.contains("quote: \\\" and slash: \\\\"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("answer"));
    }

    #[test]
    fn only_observation_commands_are_non_mutating() {
        assert!(!is_mutating(Opcode::Ping));
        assert!(!is_mutating(Opcode::State));
        assert!(!is_mutating(Opcode::NativeReady));
        assert!(is_mutating(Opcode::Show));
        assert!(is_mutating(Opcode::Quit));
    }

    #[test]
    fn native_readiness_cannot_be_inferred_by_the_pure_command_handler() {
        let mut state = running_state();
        let response = handle_request(&mut state, &Frame::empty(Opcode::NativeReady, 27).unwrap())
            .unwrap()
            .response;

        assert_eq!(response.opcode(), Opcode::Error);
    }

    #[derive(Default)]
    struct RecordingTransition(Vec<PathBuf>);

    impl RootTransition for RecordingTransition {
        fn transition(&mut self, new_root: &Path) -> Result<(), RootTransitionError> {
            self.0.push(new_root.to_path_buf());
            Ok(())
        }
    }

    struct FailingTransition {
        error: RootTransitionError,
        calls: usize,
    }

    impl RootTransition for FailingTransition {
        fn transition(&mut self, _new_root: &Path) -> Result<(), RootTransitionError> {
            self.calls += 1;
            Err(self.error.clone())
        }
    }

    fn transition_failure(rollback_incomplete: bool) -> RootTransitionError {
        RootTransitionError::TransitionFailed {
            failure: SystemFailure {
                operation: "change directory to new root",
                kind: io::ErrorKind::Other,
                errno: None,
            },
            rollback_failures: rollback_incomplete
                .then_some(SystemFailure {
                    operation: "restore old root",
                    kind: io::ErrorKind::Other,
                    errno: None,
                })
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn root_handoff_runs_through_injected_adapter_seam() {
        let mut state = running_state();
        let mut transition = RecordingTransition::default();
        let request = Frame::text(Opcode::UpdateRootFs, 9, "/sysroot").unwrap();

        handle_request_with_root_transition(&mut state, &request, &mut transition).unwrap();
        handle_request_with_root_transition(&mut state, &request, &mut transition).unwrap();

        assert_eq!(transition.0, [PathBuf::from("/sysroot")]);
        assert_eq!(state.root_stage(), RootStage::RealRoot);
    }

    #[test]
    fn cleanly_rolled_back_root_failure_fails_open_and_stops_display_ownership() {
        let mut state = running_state();
        let mut transition = FailingTransition {
            error: transition_failure(false),
            calls: 0,
        };
        let request = Frame::text(Opcode::UpdateRootFs, 10, "/sysroot").unwrap();

        let outcome =
            handle_request_with_root_transition(&mut state, &request, &mut transition).unwrap();

        assert_eq!(outcome.response.opcode(), Opcode::Error);
        assert!(outcome.should_quit);
        assert!(
            outcome
                .fatal_root_transition
                .as_ref()
                .is_some_and(|error| !error.rollback_incomplete())
        );
        assert_eq!(state.lifecycle(), Lifecycle::FailedOpen);
        assert_eq!(state.root_stage(), RootStage::Switching);
        assert_eq!(transition.calls, 1);
    }

    #[test]
    fn incomplete_root_rollback_forces_failed_open_shutdown() {
        let mut state = running_state();
        let mut transition = FailingTransition {
            error: transition_failure(true),
            calls: 0,
        };
        let request = Frame::text(Opcode::UpdateRootFs, 11, "/sysroot").unwrap();

        let outcome =
            handle_request_with_root_transition(&mut state, &request, &mut transition).unwrap();

        assert_eq!(outcome.response.opcode(), Opcode::Error);
        assert!(outcome.should_quit);
        assert!(!outcome.retain_splash);
        assert!(
            outcome
                .fatal_root_transition
                .as_ref()
                .is_some_and(RootTransitionError::rollback_incomplete)
        );
        assert_eq!(state.lifecycle(), Lifecycle::FailedOpen);
        assert_eq!(state.root_stage(), RootStage::Switching);
        assert_eq!(transition.calls, 1);
    }

    #[test]
    fn root_handoff_rejection_during_prompt_fails_open_before_adapter_call() {
        let mut state = running_state();
        state
            .apply(StateAction::BeginPrompt(
                PromptMetadata::new(41, "Unlock volume").unwrap(),
            ))
            .unwrap();
        let mut transition = RecordingTransition::default();
        let request = Frame::text(Opcode::UpdateRootFs, 9, "/sysroot").unwrap();

        let outcome =
            handle_request_with_root_transition(&mut state, &request, &mut transition).unwrap();

        assert_eq!(outcome.response.opcode(), Opcode::Error);
        assert!(outcome.should_quit);
        assert!(!outcome.retain_splash);
        assert!(outcome.fatal_root_transition.is_none());
        assert!(transition.0.is_empty());
        assert_eq!(state.root_stage(), RootStage::Initramfs);
        assert_eq!(state.lifecycle(), Lifecycle::FailedOpen);
        assert!(state.view().prompt().is_none());
    }

    #[test]
    fn quit_carries_retain_intent_without_changing_protocol_response() {
        let mut state = running_state();
        let outcome = handle_request(&mut state, &Frame::quit(10, true).unwrap()).unwrap();

        assert!(outcome.should_quit);
        assert!(outcome.retain_splash);
        assert_eq!(outcome.response.opcode(), Opcode::Ack);
    }
}
