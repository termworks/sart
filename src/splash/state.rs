use std::error::Error;
use std::fmt;

/// Maximum text retained in presentation state.
///
/// The wire protocol applies smaller, field-specific limits where appropriate.
pub const MAX_DISPLAY_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_PROMPT_TEXT_BYTES: usize = 1024;
pub const MAX_PROMPT_SOURCE_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Starting,
    Running,
    Deactivated,
    Quitting,
    Stopped,
    FailedOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseView {
    Hidden,
    Splash,
    Details,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Hidden,
    Splash,
    Details,
    Prompt {
        previous_view: BaseView,
        metadata: PromptMetadata,
    },
}

impl View {
    pub fn base_view(&self) -> Option<BaseView> {
        match self {
            Self::Hidden => Some(BaseView::Hidden),
            Self::Splash => Some(BaseView::Splash),
            Self::Details => Some(BaseView::Details),
            Self::Prompt { .. } => None,
        }
    }

    pub fn prompt(&self) -> Option<&PromptMetadata> {
        match self {
            Self::Prompt { metadata, .. } => Some(metadata),
            _ => None,
        }
    }
}

impl From<BaseView> for View {
    fn from(value: BaseView) -> Self {
        match value {
            BaseView::Hidden => Self::Hidden,
            BaseView::Splash => Self::Splash,
            BaseView::Details => Self::Details,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Boot,
    Shutdown,
    Reboot,
    Update,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootStage {
    Initramfs,
    Switching,
    RealRoot,
}

/// Non-secret information needed to render and retire a prompt.
///
/// The answer deliberately has no representation in this type or in
/// [`SplashState`]. Secret bytes must travel over a dedicated credential
/// channel, never through presentation state or the general control protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMetadata {
    request_id: u64,
    text: String,
    source: Option<String>,
    requester_pid: Option<u32>,
    echo: bool,
    silent: bool,
    expires_at_millis: Option<u64>,
}

impl PromptMetadata {
    pub fn new(request_id: u64, text: impl Into<String>) -> Result<Self, TextError> {
        let text = text.into();
        if text.is_empty() {
            return Err(TextError::Empty);
        }
        validate_display_text(&text, MAX_PROMPT_TEXT_BYTES)?;

        Ok(Self {
            request_id,
            text,
            source: None,
            requester_pid: None,
            echo: false,
            silent: false,
            expires_at_millis: None,
        })
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Result<Self, TextError> {
        let source = source.into();
        if source.is_empty() {
            return Err(TextError::Empty);
        }
        validate_display_text(&source, MAX_PROMPT_SOURCE_BYTES)?;
        self.source = Some(source);
        Ok(self)
    }

    pub fn with_requester_pid(mut self, requester_pid: u32) -> Self {
        self.requester_pid = Some(requester_pid);
        self
    }

    pub fn with_echo(mut self, echo: bool) -> Self {
        self.echo = echo;
        self
    }

    pub fn with_silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    pub fn with_expiry(mut self, expires_at_millis: u64) -> Self {
        self.expires_at_millis = Some(expires_at_millis);
        self
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn requester_pid(&self) -> Option<u32> {
        self.requester_pid
    }

    pub fn echo(&self) -> bool {
        self.echo
    }

    pub fn silent(&self) -> bool {
        self.silent
    }

    pub fn expires_at_millis(&self) -> Option<u64> {
        self.expires_at_millis
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    Answered,
    Cancelled,
    TimedOut,
    RequestGone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateAction {
    MarkRunning,
    Show,
    Hide,
    ShowDetails,
    HideDetails,
    ToggleDetails,
    Deactivate,
    Reactivate,
    SetMode(Mode),
    SetRootStage(RootStage),
    SetStatus(Option<String>),
    SetMessage(Option<String>),
    SetProgress(Option<u8>),
    BeginPrompt(PromptMetadata),
    FinishPrompt {
        request_id: u64,
        outcome: PromptOutcome,
    },
    Quit,
    MarkStopped,
    FailOpen,
}

impl StateAction {
    fn name(&self) -> &'static str {
        match self {
            Self::MarkRunning => "mark-running",
            Self::Show => "show",
            Self::Hide => "hide",
            Self::ShowDetails => "show-details",
            Self::HideDetails => "hide-details",
            Self::ToggleDetails => "toggle-details",
            Self::Deactivate => "deactivate",
            Self::Reactivate => "reactivate",
            Self::SetMode(_) => "set-mode",
            Self::SetRootStage(_) => "set-root-stage",
            Self::SetStatus(_) => "set-status",
            Self::SetMessage(_) => "set-message",
            Self::SetProgress(_) => "set-progress",
            Self::BeginPrompt(_) => "begin-prompt",
            Self::FinishPrompt { .. } => "finish-prompt",
            Self::Quit => "quit",
            Self::MarkStopped => "mark-stopped",
            Self::FailOpen => "fail-open",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionResult {
    Changed,
    Unchanged,
}

impl TransitionResult {
    pub fn changed(self) -> bool {
        matches!(self, Self::Changed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplashState {
    lifecycle: Lifecycle,
    view: View,
    mode: Mode,
    root_stage: RootStage,
    status: Option<String>,
    message: Option<String>,
    progress: Option<u8>,
    last_finished_prompt_id: Option<u64>,
}

impl SplashState {
    pub fn new(mode: Mode) -> Self {
        Self {
            lifecycle: Lifecycle::Starting,
            view: View::Splash,
            mode,
            root_stage: RootStage::Initramfs,
            status: None,
            message: None,
            progress: None,
            last_finished_prompt_id: None,
        }
    }

    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn root_stage(&self) -> RootStage {
        self.root_stage
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn progress(&self) -> Option<u8> {
        self.progress
    }

    pub fn apply(&mut self, action: StateAction) -> Result<TransitionResult, StateError> {
        let operation = action.name();
        match action {
            StateAction::MarkRunning => match self.lifecycle {
                Lifecycle::Starting => {
                    self.lifecycle = Lifecycle::Running;
                    Ok(TransitionResult::Changed)
                }
                Lifecycle::Running => Ok(TransitionResult::Unchanged),
                _ => Err(self.invalid_transition(operation)),
            },
            StateAction::Show => self.set_view(BaseView::Splash, operation),
            StateAction::Hide => self.set_view(BaseView::Hidden, operation),
            StateAction::ShowDetails => self.set_view(BaseView::Details, operation),
            StateAction::HideDetails => self.set_view(BaseView::Splash, operation),
            StateAction::ToggleDetails => {
                self.require_running(operation)?;
                let target = match self.view.base_view() {
                    Some(BaseView::Details) => BaseView::Splash,
                    Some(_) => BaseView::Details,
                    None => return Ok(TransitionResult::Unchanged),
                };
                self.replace_view(target)
            }
            StateAction::Deactivate => {
                if self.view.prompt().is_some() {
                    return Err(StateError::PromptActive);
                }
                match self.lifecycle {
                    Lifecycle::Running => {
                        self.lifecycle = Lifecycle::Deactivated;
                        Ok(TransitionResult::Changed)
                    }
                    Lifecycle::Deactivated => Ok(TransitionResult::Unchanged),
                    _ => Err(self.invalid_transition(operation)),
                }
            }
            StateAction::Reactivate => match self.lifecycle {
                Lifecycle::Deactivated => {
                    self.lifecycle = Lifecycle::Running;
                    Ok(TransitionResult::Changed)
                }
                Lifecycle::Running => Ok(TransitionResult::Unchanged),
                _ => Err(self.invalid_transition(operation)),
            },
            StateAction::SetMode(mode) => {
                self.require_presentable(operation)?;
                replace_if_different(&mut self.mode, mode)
            }
            StateAction::SetRootStage(stage) => self.set_root_stage(stage, operation),
            StateAction::SetStatus(status) => {
                self.require_presentable(operation)?;
                validate_optional_text(status.as_deref(), MAX_DISPLAY_TEXT_BYTES)?;
                replace_if_different(&mut self.status, status)
            }
            StateAction::SetMessage(message) => {
                self.require_presentable(operation)?;
                validate_optional_text(message.as_deref(), MAX_DISPLAY_TEXT_BYTES)?;
                replace_if_different(&mut self.message, message)
            }
            StateAction::SetProgress(progress) => {
                self.require_presentable(operation)?;
                if let Some(value) = progress
                    && value > 100
                {
                    return Err(StateError::InvalidProgress(value));
                }
                replace_if_different(&mut self.progress, progress)
            }
            StateAction::BeginPrompt(metadata) => {
                self.require_running(operation)?;
                match &self.view {
                    View::Prompt {
                        metadata: active, ..
                    } if active == &metadata => Ok(TransitionResult::Unchanged),
                    View::Prompt {
                        metadata: active, ..
                    } => Err(StateError::PromptConflict {
                        active_request_id: active.request_id(),
                        requested_id: metadata.request_id(),
                    }),
                    _ => {
                        let previous_view = self
                            .view
                            .base_view()
                            .expect("non-prompt views always have a base view");
                        self.view = View::Prompt {
                            previous_view,
                            metadata,
                        };
                        Ok(TransitionResult::Changed)
                    }
                }
            }
            StateAction::FinishPrompt {
                request_id,
                outcome: _,
            } => self.finish_prompt(request_id),
            StateAction::Quit => match self.lifecycle {
                Lifecycle::Quitting | Lifecycle::Stopped => Ok(TransitionResult::Unchanged),
                Lifecycle::Starting
                | Lifecycle::Running
                | Lifecycle::Deactivated
                | Lifecycle::FailedOpen => {
                    self.retire_prompt();
                    self.lifecycle = Lifecycle::Quitting;
                    Ok(TransitionResult::Changed)
                }
            },
            StateAction::MarkStopped => match self.lifecycle {
                Lifecycle::Quitting | Lifecycle::FailedOpen => {
                    self.lifecycle = Lifecycle::Stopped;
                    Ok(TransitionResult::Changed)
                }
                Lifecycle::Stopped => Ok(TransitionResult::Unchanged),
                _ => Err(self.invalid_transition(operation)),
            },
            StateAction::FailOpen => match self.lifecycle {
                Lifecycle::Starting | Lifecycle::Running | Lifecycle::Deactivated => {
                    self.retire_prompt();
                    self.lifecycle = Lifecycle::FailedOpen;
                    Ok(TransitionResult::Changed)
                }
                Lifecycle::FailedOpen => Ok(TransitionResult::Unchanged),
                Lifecycle::Quitting | Lifecycle::Stopped => Err(self.invalid_transition(operation)),
            },
        }
    }

    fn set_view(
        &mut self,
        target: BaseView,
        operation: &'static str,
    ) -> Result<TransitionResult, StateError> {
        self.require_running(operation)?;
        if self.view.prompt().is_some() {
            return Ok(TransitionResult::Unchanged);
        }
        self.replace_view(target)
    }

    fn replace_view(&mut self, target: BaseView) -> Result<TransitionResult, StateError> {
        let target = View::from(target);
        replace_if_different(&mut self.view, target)
    }

    fn set_root_stage(
        &mut self,
        target: RootStage,
        operation: &'static str,
    ) -> Result<TransitionResult, StateError> {
        self.require_presentable(operation)?;
        if self.view.prompt().is_some() {
            return Err(StateError::PromptActive);
        }
        if self.root_stage == target {
            return Ok(TransitionResult::Unchanged);
        }

        let valid = matches!(
            (self.root_stage, target),
            (RootStage::Initramfs, RootStage::Switching)
                | (RootStage::Switching, RootStage::RealRoot)
        );
        if !valid {
            return Err(StateError::InvalidRootTransition {
                from: self.root_stage,
                to: target,
            });
        }

        self.root_stage = target;
        Ok(TransitionResult::Changed)
    }

    fn finish_prompt(&mut self, request_id: u64) -> Result<TransitionResult, StateError> {
        self.require_running("finish-prompt")?;
        match &self.view {
            View::Prompt {
                previous_view,
                metadata,
            } if metadata.request_id() == request_id => {
                let previous_view = *previous_view;
                self.view = View::from(previous_view);
                self.last_finished_prompt_id = Some(request_id);
                Ok(TransitionResult::Changed)
            }
            View::Prompt { metadata, .. } => Err(StateError::PromptIdMismatch {
                active_request_id: metadata.request_id(),
                received_id: request_id,
            }),
            _ if self.last_finished_prompt_id == Some(request_id) => {
                Ok(TransitionResult::Unchanged)
            }
            _ => Err(StateError::NoActivePrompt),
        }
    }

    fn retire_prompt(&mut self) {
        if let View::Prompt {
            previous_view,
            metadata,
        } = &self.view
        {
            let previous_view = *previous_view;
            self.last_finished_prompt_id = Some(metadata.request_id());
            self.view = View::from(previous_view);
        }
    }

    fn require_running(&self, operation: &'static str) -> Result<(), StateError> {
        if self.lifecycle == Lifecycle::Running {
            Ok(())
        } else {
            Err(self.invalid_transition(operation))
        }
    }

    fn require_presentable(&self, operation: &'static str) -> Result<(), StateError> {
        if matches!(
            self.lifecycle,
            Lifecycle::Starting | Lifecycle::Running | Lifecycle::Deactivated
        ) {
            Ok(())
        } else {
            Err(self.invalid_transition(operation))
        }
    }

    fn invalid_transition(&self, operation: &'static str) -> StateError {
        StateError::InvalidLifecycleTransition {
            lifecycle: self.lifecycle,
            operation,
        }
    }
}

impl Default for SplashState {
    fn default() -> Self {
        Self::new(Mode::Boot)
    }
}

fn replace_if_different<T: PartialEq>(
    current: &mut T,
    replacement: T,
) -> Result<TransitionResult, StateError> {
    if current == &replacement {
        Ok(TransitionResult::Unchanged)
    } else {
        *current = replacement;
        Ok(TransitionResult::Changed)
    }
}

fn validate_optional_text(value: Option<&str>, max_bytes: usize) -> Result<(), StateError> {
    if let Some(value) = value {
        validate_display_text(value, max_bytes)?;
    }
    Ok(())
}

/// Validates text before it can reach terminal rendering.
///
/// Rejecting rather than stripping controls makes state changes atomic and
/// keeps different render backends from interpreting the same input
/// differently.
pub fn validate_display_text(value: &str, max_bytes: usize) -> Result<(), TextError> {
    if value.len() > max_bytes {
        return Err(TextError::TooLong {
            length: value.len(),
            maximum: max_bytes,
        });
    }

    for (byte_index, character) in value.char_indices() {
        if is_unsafe_display_character(character) {
            return Err(TextError::UnsafeCharacter {
                byte_index,
                codepoint: character as u32,
            });
        }
    }

    Ok(())
}

fn is_unsafe_display_character(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    Empty,
    TooLong { length: usize, maximum: usize },
    UnsafeCharacter { byte_index: usize, codepoint: u32 },
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "text must not be empty"),
            Self::TooLong { length, maximum } => {
                write!(formatter, "text is {length} bytes; maximum is {maximum}")
            }
            Self::UnsafeCharacter {
                byte_index,
                codepoint,
            } => write!(
                formatter,
                "text contains unsafe character U+{codepoint:04X} at byte {byte_index}"
            ),
        }
    }
}

impl Error for TextError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    InvalidLifecycleTransition {
        lifecycle: Lifecycle,
        operation: &'static str,
    },
    InvalidRootTransition {
        from: RootStage,
        to: RootStage,
    },
    InvalidProgress(u8),
    InvalidText(TextError),
    PromptActive,
    PromptConflict {
        active_request_id: u64,
        requested_id: u64,
    },
    PromptIdMismatch {
        active_request_id: u64,
        received_id: u64,
    },
    NoActivePrompt,
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLifecycleTransition {
                lifecycle,
                operation,
            } => write!(
                formatter,
                "operation {operation} is invalid while lifecycle is {lifecycle:?}"
            ),
            Self::InvalidRootTransition { from, to } => {
                write!(formatter, "root stage cannot move from {from:?} to {to:?}")
            }
            Self::InvalidProgress(value) => {
                write!(formatter, "progress {value} is outside 0..=100")
            }
            Self::InvalidText(error) => error.fmt(formatter),
            Self::PromptActive => write!(formatter, "a prompt has priority over this operation"),
            Self::PromptConflict {
                active_request_id,
                requested_id,
            } => write!(
                formatter,
                "prompt {active_request_id} is active; cannot start prompt {requested_id}"
            ),
            Self::PromptIdMismatch {
                active_request_id,
                received_id,
            } => write!(
                formatter,
                "prompt {active_request_id} is active; received completion for {received_id}"
            ),
            Self::NoActivePrompt => write!(formatter, "there is no active prompt"),
        }
    }
}

impl Error for StateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidText(error) => Some(error),
            _ => None,
        }
    }
}

impl From<TextError> for StateError {
    fn from(value: TextError) -> Self {
        Self::InvalidText(value)
    }
}
