use bootart::splash::state::{
    BaseView, Lifecycle, Mode, PromptMetadata, PromptOutcome, RootStage, SplashState, StateAction,
    StateError, TextError, TransitionResult, View,
};

fn running_state() -> SplashState {
    let mut state = SplashState::new(Mode::Boot);
    assert_eq!(
        state.apply(StateAction::MarkRunning).unwrap(),
        TransitionResult::Changed
    );
    state
}

fn prompt(request_id: u64) -> PromptMetadata {
    PromptMetadata::new(request_id, "Password for encrypted root:")
        .unwrap()
        .with_source("systemd-cryptsetup")
        .unwrap()
        .with_requester_pid(42)
        .with_expiry(50_000)
}

#[test]
fn state_axes_are_orthogonal_and_updates_are_idempotent() {
    let mut state = running_state();

    assert_eq!(state.lifecycle(), Lifecycle::Running);
    assert_eq!(state.view(), &View::Splash);
    assert_eq!(state.mode(), Mode::Boot);
    assert_eq!(state.root_stage(), RootStage::Initramfs);

    assert_eq!(
        state.apply(StateAction::SetMode(Mode::Update)).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state
            .apply(StateAction::SetStatus(Some("Mounting filesystems".into())))
            .unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::SetProgress(Some(37))).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::SetProgress(Some(37))).unwrap(),
        TransitionResult::Unchanged
    );

    assert_eq!(state.lifecycle(), Lifecycle::Running);
    assert_eq!(state.view(), &View::Splash);
    assert_eq!(state.mode(), Mode::Update);
    assert_eq!(state.root_stage(), RootStage::Initramfs);
    assert_eq!(state.status(), Some("Mounting filesystems"));
    assert_eq!(state.progress(), Some(37));
}

#[test]
fn lifecycle_rejects_invalid_transitions_without_mutation() {
    let mut state = SplashState::default();
    let before = state.clone();

    assert!(matches!(
        state.apply(StateAction::Show),
        Err(StateError::InvalidLifecycleTransition {
            lifecycle: Lifecycle::Starting,
            operation: "show"
        })
    ));
    assert_eq!(state, before);

    assert_eq!(
        state.apply(StateAction::MarkRunning).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::MarkRunning).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state.apply(StateAction::Deactivate).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::Deactivate).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state.apply(StateAction::Reactivate).unwrap(),
        TransitionResult::Changed
    );
}

#[test]
fn view_commands_are_idempotent() {
    let mut state = running_state();

    assert_eq!(
        state.apply(StateAction::Show).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state.apply(StateAction::Hide).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::Hide).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(state.view().base_view(), Some(BaseView::Hidden));

    state.apply(StateAction::ToggleDetails).unwrap();
    assert_eq!(state.view().base_view(), Some(BaseView::Details));
    state.apply(StateAction::ToggleDetails).unwrap();
    assert_eq!(state.view().base_view(), Some(BaseView::Splash));
}

#[test]
fn prompt_has_priority_and_restores_the_exact_previous_view() {
    let mut state = running_state();
    state.apply(StateAction::Hide).unwrap();
    let metadata = prompt(71);

    assert_eq!(
        state
            .apply(StateAction::BeginPrompt(metadata.clone()))
            .unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(state.view().prompt(), Some(&metadata));

    // Presentation commands cannot obscure a secret prompt or alter the view
    // that must be restored after it is retired.
    assert_eq!(
        state.apply(StateAction::Show).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state.apply(StateAction::ShowDetails).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(state.view().prompt(), Some(&metadata));
    assert_eq!(
        state.apply(StateAction::Deactivate),
        Err(StateError::PromptActive)
    );

    assert_eq!(
        state
            .apply(StateAction::FinishPrompt {
                request_id: 71,
                outcome: PromptOutcome::Answered,
            })
            .unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(state.view(), &View::Hidden);
    assert_eq!(
        state
            .apply(StateAction::FinishPrompt {
                request_id: 71,
                outcome: PromptOutcome::Answered,
            })
            .unwrap(),
        TransitionResult::Unchanged
    );
}

#[test]
fn conflicting_prompt_and_wrong_completion_are_rejected_atomically() {
    let mut state = running_state();
    let active = prompt(10);
    state
        .apply(StateAction::BeginPrompt(active.clone()))
        .unwrap();
    let before = state.clone();

    assert!(matches!(
        state.apply(StateAction::BeginPrompt(prompt(11))),
        Err(StateError::PromptConflict {
            active_request_id: 10,
            requested_id: 11
        })
    ));
    assert_eq!(state, before);

    assert!(matches!(
        state.apply(StateAction::FinishPrompt {
            request_id: 11,
            outcome: PromptOutcome::Cancelled,
        }),
        Err(StateError::PromptIdMismatch {
            active_request_id: 10,
            received_id: 11
        })
    ));
    assert_eq!(state, before);
}

#[test]
fn prompt_metadata_carries_no_answer_and_rejects_terminal_controls() {
    let metadata = prompt(99);
    assert_eq!(metadata.request_id(), 99);
    assert_eq!(metadata.text(), "Password for encrypted root:");
    assert_eq!(metadata.source(), Some("systemd-cryptsetup"));
    assert_eq!(metadata.requester_pid(), Some(42));
    assert!(!metadata.echo());
    assert!(!metadata.silent());
    assert_eq!(metadata.expires_at_millis(), Some(50_000));

    assert!(matches!(
        PromptMetadata::new(1, "password\u{1b}[2J"),
        Err(TextError::UnsafeCharacter {
            codepoint: 0x1b,
            ..
        })
    ));
}

#[test]
fn invalid_text_and_progress_do_not_partially_update_state() {
    let mut state = running_state();
    state
        .apply(StateAction::SetStatus(Some("safe".into())))
        .unwrap();
    state.apply(StateAction::SetProgress(Some(50))).unwrap();
    let before = state.clone();

    assert!(matches!(
        state.apply(StateAction::SetStatus(Some("unsafe\nstatus".into()))),
        Err(StateError::InvalidText(TextError::UnsafeCharacter { .. }))
    ));
    assert_eq!(state, before);

    assert_eq!(
        state.apply(StateAction::SetProgress(Some(101))),
        Err(StateError::InvalidProgress(101))
    );
    assert_eq!(state, before);
}

#[test]
fn root_stage_is_monotonic_and_each_step_is_idempotent() {
    let mut state = running_state();

    let before = state.clone();
    assert_eq!(
        state.apply(StateAction::SetRootStage(RootStage::RealRoot)),
        Err(StateError::InvalidRootTransition {
            from: RootStage::Initramfs,
            to: RootStage::RealRoot,
        })
    );
    assert_eq!(state, before);

    assert_eq!(
        state
            .apply(StateAction::SetRootStage(RootStage::Switching))
            .unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state
            .apply(StateAction::SetRootStage(RootStage::Switching))
            .unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state
            .apply(StateAction::SetRootStage(RootStage::RealRoot))
            .unwrap(),
        TransitionResult::Changed
    );
    assert!(matches!(
        state.apply(StateAction::SetRootStage(RootStage::Switching)),
        Err(StateError::InvalidRootTransition { .. })
    ));
}

#[test]
fn quit_and_stop_are_idempotent_and_terminal_state_is_immutable() {
    let mut state = running_state();
    state.apply(StateAction::BeginPrompt(prompt(123))).unwrap();

    assert_eq!(
        state.apply(StateAction::Quit).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(state.lifecycle(), Lifecycle::Quitting);
    assert_eq!(state.view(), &View::Splash);
    assert_eq!(
        state.apply(StateAction::Quit).unwrap(),
        TransitionResult::Unchanged
    );
    assert_eq!(
        state.apply(StateAction::MarkStopped).unwrap(),
        TransitionResult::Changed
    );
    assert_eq!(
        state.apply(StateAction::MarkStopped).unwrap(),
        TransitionResult::Unchanged
    );

    let before = state.clone();
    assert!(matches!(
        state.apply(StateAction::SetMessage(Some("too late".into()))),
        Err(StateError::InvalidLifecycleTransition { .. })
    ));
    assert_eq!(state, before);
}
