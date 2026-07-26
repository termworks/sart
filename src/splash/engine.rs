//! Persistent splash presentation engine.
//!
//! Timing, presentation state, frame generation, and display ownership meet in
//! this module.  The engine has no knowledge of init systems or boot control:
//! callers supply elapsed monotonic time and presentation commands, and the
//! engine owns only a [`DisplayBackend`].

use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use crate::art::Art;
use crate::display::{
    Cell, Color, Dimensions, DisplayBackend, DisplayError, DisplayState, InputEvent, RestoreMode,
    Scene, Style, sensitive_cell_width,
};
use crate::password::{EchoMode, InputFeedback, PromptCoordinator};
use crate::render::{FrameEngine, FrameError};
use crate::terminal::TerminalSize;

use super::state::{Lifecycle, SplashState, StateAction, StateError, View};

pub const DEFAULT_FRAMES_PER_SECOND: u16 = 30;
pub const MAX_FRAMES_PER_SECOND: u16 = 60;
pub const DEFAULT_ANIMATION_CYCLE: Duration = Duration::from_millis(2_500);
pub const MIN_ANIMATION_CYCLE: Duration = Duration::from_millis(100);
pub const MAX_ANIMATION_CYCLE: Duration = Duration::from_secs(60);
const MAX_INPUT_EVENTS_PER_TICK: usize = 8;

/// Monotonic time source used by the engine.
///
/// Production uses [`SystemClock`]. Tests can advance a fake clock without
/// sleeping or running the product binary.
pub trait Clock {
    fn elapsed(&self) -> Duration;
}

#[derive(Debug)]
pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    pub frames_per_second: u16,
    pub animation_cycle: Duration,
    pub seed: u64,
    pub no_color: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            frames_per_second: DEFAULT_FRAMES_PER_SECOND,
            animation_cycle: DEFAULT_ANIMATION_CYCLE,
            seed: 42,
            no_color: false,
        }
    }
}

impl EngineConfig {
    fn frame_period(self) -> Result<Duration, EngineError> {
        if !(1..=MAX_FRAMES_PER_SECOND).contains(&self.frames_per_second) {
            return Err(EngineError::InvalidFramesPerSecond(self.frames_per_second));
        }
        if !(MIN_ANIMATION_CYCLE..=MAX_ANIMATION_CYCLE).contains(&self.animation_cycle) {
            return Err(EngineError::InvalidAnimationCycle(self.animation_cycle));
        }

        Ok(Duration::from_nanos(
            1_000_000_000 / u64::from(self.frames_per_second),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineTick {
    pub frame_rendered: bool,
    pub stopped: bool,
}

/// A persistent, backend-neutral splash renderer.
///
/// `Drop` always attempts restoration. Callers should still call
/// [`SplashEngine::restore`] at their normal boundary so restoration failures
/// can be reported.
pub struct SplashEngine<'art, B: DisplayBackend> {
    backend: B,
    main: FrameEngine<'art>,
    small: Option<FrameEngine<'art>>,
    config: EngineConfig,
    frame_period: Duration,
    next_frame: Duration,
    acquired: bool,
    restored: bool,
}

impl<'art, B: DisplayBackend> SplashEngine<'art, B> {
    pub fn new(
        backend: B,
        main_art: &'art Art,
        small_art: Option<&'art Art>,
        config: EngineConfig,
    ) -> Result<Self, EngineError> {
        let frame_period = config.frame_period()?;
        Ok(Self {
            backend,
            main: FrameEngine::new(main_art, config.seed),
            small: small_art.map(|art| FrameEngine::new(art, config.seed)),
            config,
            frame_period,
            next_frame: Duration::ZERO,
            acquired: false,
            restored: false,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    #[doc(hidden)]
    pub fn backend_mut_for_test(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn start(&mut self, state: &mut SplashState) -> Result<(), EngineError> {
        if self.acquired {
            return Ok(());
        }
        if self.restored {
            return Err(EngineError::AlreadyRestored);
        }

        if let Err(error) = self.backend.acquire() {
            return Err(self.fail_open(state, EngineError::Display(error)));
        }
        self.acquired = true;
        Ok(())
    }

    pub fn tick<C: Clock>(
        &mut self,
        state: &mut SplashState,
        clock: &C,
    ) -> Result<EngineTick, EngineError> {
        self.tick_at(state, clock.elapsed())
    }

    pub fn tick_with_prompt<C: Clock>(
        &mut self,
        state: &mut SplashState,
        clock: &C,
        prompt: &mut dyn PromptCoordinator,
    ) -> Result<EngineTick, EngineError> {
        let mut prompt = Some(prompt);
        self.tick_at_inner(state, clock.elapsed(), &mut prompt)
    }

    pub fn tick_at(
        &mut self,
        state: &mut SplashState,
        elapsed: Duration,
    ) -> Result<EngineTick, EngineError> {
        let mut prompt = None;
        self.tick_at_inner(state, elapsed, &mut prompt)
    }

    fn tick_at_inner(
        &mut self,
        state: &mut SplashState,
        elapsed: Duration,
        prompt: &mut Option<&mut dyn PromptCoordinator>,
    ) -> Result<EngineTick, EngineError> {
        if !self.acquired {
            return Err(EngineError::NotStarted);
        }
        if self.restored {
            return Ok(EngineTick {
                frame_rendered: false,
                stopped: true,
            });
        }

        if matches!(
            state.lifecycle(),
            Lifecycle::Quitting | Lifecycle::Stopped | Lifecycle::FailedOpen
        ) {
            self.restore()?;
            return Ok(EngineTick {
                frame_rendered: false,
                stopped: true,
            });
        }

        if let Err(error) = self.reconcile_display(state) {
            return Err(self.fail_open(state, error));
        }

        if let Err(error) = self.process_input(state, prompt) {
            return Err(self.fail_open(state, error));
        }

        if let Err(error) = self.reconcile_display(state) {
            return Err(self.fail_open(state, error));
        }

        let mut frame_rendered = false;
        if elapsed >= self.next_frame {
            // Never render catch-up frames. A slow backend lowers visual FPS
            // rather than monopolizing the daemon's command loop.
            self.next_frame = elapsed.saturating_add(self.frame_period);
            if self.backend.state() == DisplayState::Splash {
                if let Err(error) = self.render_frame(state, elapsed, prompt) {
                    return Err(self.fail_open(state, error));
                }
                frame_rendered = true;
            }
        }

        Ok(EngineTick {
            frame_rendered,
            stopped: false,
        })
    }

    pub fn time_until_next_frame(&self, elapsed: Duration) -> Duration {
        self.next_frame.saturating_sub(elapsed)
    }

    pub fn restore(&mut self) -> Result<(), EngineError> {
        self.restore_with_mode(RestoreMode::Clear)
    }

    pub fn shutdown(&mut self, retain_splash: bool) -> Result<(), EngineError> {
        let mode = if retain_splash {
            RestoreMode::RetainPixels
        } else {
            RestoreMode::Clear
        };
        self.restore_with_mode(mode)
    }

    fn restore_with_mode(&mut self, mode: RestoreMode) -> Result<(), EngineError> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        self.backend
            .restore_with_mode(mode)
            .map_err(EngineError::Restoration)
    }

    fn reconcile_display(&mut self, state: &SplashState) -> Result<(), EngineError> {
        match state.lifecycle() {
            Lifecycle::Starting => self.backend.hide(),
            Lifecycle::Running => match state.view() {
                View::Hidden => self.backend.hide(),
                View::Details => self.backend.details(true),
                View::Splash | View::Prompt { .. } => match self.backend.state() {
                    DisplayState::Details => self.backend.details(false),
                    _ => self.backend.show(),
                },
            },
            Lifecycle::Deactivated => self.backend.hide(),
            Lifecycle::Quitting | Lifecycle::Stopped | Lifecycle::FailedOpen => {
                return self.restore();
            }
        }
        .map_err(EngineError::Display)
    }

    fn process_input(
        &mut self,
        state: &mut SplashState,
        prompt: &mut Option<&mut dyn PromptCoordinator>,
    ) -> Result<(), EngineError> {
        for _ in 0..MAX_INPUT_EVENTS_PER_TICK {
            let event = self
                .backend
                .poll_input(Duration::ZERO)
                .map_err(EngineError::Display)?;
            let Some(mut event) = event else {
                break;
            };

            match &mut event {
                InputEvent::Resized(_) => {
                    // Backends update their authoritative dimensions before
                    // returning this notification. The next frame uses them.
                    self.next_frame = Duration::ZERO;
                }
                InputEvent::ReturnToSplash => {
                    if matches!(state.view(), View::Details) {
                        state
                            .apply(StateAction::HideDetails)
                            .map_err(EngineError::State)?;
                    }
                }
                InputEvent::Bytes(bytes) => {
                    if state.view().prompt().is_some() {
                        if let Some(prompt) = prompt.as_deref_mut() {
                            prompt.handle_input(state, bytes);
                        }
                    } else if bytes.as_slice() == [0x1b] {
                        // A lone ESC is the documented details gesture. Escape
                        // sequences (for example arrow keys) never toggle.
                        state
                            .apply(StateAction::ToggleDetails)
                            .map_err(EngineError::State)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn render_frame(
        &mut self,
        state: &SplashState,
        elapsed: Duration,
        prompt: &mut Option<&mut dyn PromptCoordinator>,
    ) -> Result<(), EngineError> {
        let dimensions = self
            .backend
            .dimensions()
            .ok_or(EngineError::MissingDimensions)?;
        let renderer = self.select_renderer(dimensions);
        let (progress, iteration) = animation_position(elapsed, self.config.animation_cycle);
        let terminal_size = TerminalSize {
            width: usize::from(dimensions.columns()),
            height: usize::from(dimensions.rows()),
        };
        let mut scene = renderer
            .render(terminal_size, progress, self.config.no_color, iteration)
            .map_err(EngineError::Frame)?;
        let feedback = prompt.as_deref().and_then(PromptCoordinator::feedback);
        let sensitive =
            apply_overlays(&mut scene, state, feedback).map_err(EngineError::Display)?;
        self.backend.render(&scene).map_err(EngineError::Display)?;

        if let (Some(layout), Some(prompt)) = (sensitive, prompt.as_deref()) {
            let mut result = Ok(());
            prompt.with_visible_text(&mut |text| {
                let (visible, cells) = visible_prefix(text, usize::from(layout.columns));
                let column = (usize::from(layout.columns).saturating_sub(cells)) / 2;
                result = self.backend.render_sensitive_text(
                    layout.row,
                    column as u16,
                    visible,
                    layout.style,
                );
            });
            result.map_err(EngineError::Display)?;
        }
        Ok(())
    }

    fn select_renderer(&self, dimensions: Dimensions) -> &FrameEngine<'art> {
        let main_size = self.main.art_size();
        let main_fits = main_size.width <= usize::from(dimensions.columns())
            && main_size.height <= usize::from(dimensions.rows());
        if main_fits {
            return &self.main;
        }

        self.small
            .as_ref()
            .filter(|small| {
                let size = small.art_size();
                size.width <= usize::from(dimensions.columns())
                    && size.height <= usize::from(dimensions.rows())
            })
            .unwrap_or(&self.main)
    }

    fn fail_open(&mut self, state: &mut SplashState, operation: EngineError) -> EngineError {
        let _ = state.apply(StateAction::FailOpen);
        match self.restore() {
            Ok(()) => operation,
            Err(restoration) => EngineError::OperationAndRestore {
                operation: Box::new(operation),
                restoration: Box::new(restoration),
            },
        }
    }
}

impl<B: DisplayBackend> Drop for SplashEngine<'_, B> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn animation_position(elapsed: Duration, cycle: Duration) -> (f32, usize) {
    let cycle_nanos = cycle.as_nanos().max(1);
    let elapsed_nanos = elapsed.as_nanos();
    let iteration = usize::try_from(elapsed_nanos / cycle_nanos).unwrap_or(usize::MAX);
    let within_cycle = elapsed_nanos % cycle_nanos;
    let progress = (within_cycle as f64 / cycle_nanos as f64) as f32;
    (progress, iteration)
}

#[derive(Debug, Clone, Copy)]
struct SensitiveLayout {
    row: u16,
    columns: u16,
    style: Style,
}

fn apply_overlays(
    scene: &mut Scene,
    state: &SplashState,
    feedback: Option<InputFeedback>,
) -> Result<Option<SensitiveLayout>, DisplayError> {
    let dimensions = scene.dimensions();
    let mut lines: Vec<(String, Style)> = Vec::with_capacity(4);
    let input_style = Style {
        foreground: Color::BrightWhite,
        background: Color::Default,
        bold: true,
    };
    let mut visible_input_offset = None;

    if let Some(prompt) = state.view().prompt() {
        match feedback.map(InputFeedback::echo_mode) {
            Some(EchoMode::Obscured) => lines.push((
                "•".repeat(
                    feedback
                        .map(InputFeedback::character_count)
                        .unwrap_or(0)
                        .min(usize::from(dimensions.columns())),
                ),
                input_style,
            )),
            Some(EchoMode::Visible) => {
                visible_input_offset = Some(lines.len());
                // Clear/reserve the row in the ordinary scene, but never put
                // plaintext in it. The backend receives plaintext later via
                // its non-retaining sensitive-text seam.
                lines.push((String::new(), input_style));
            }
            Some(EchoMode::Silent) | None => {}
        }
        lines.push((
            prompt.text().to_owned(),
            Style {
                foreground: Color::BrightWhite,
                background: Color::Blue,
                bold: true,
            },
        ));
    } else {
        if let Some(message) = state.message() {
            lines.push((
                message.to_owned(),
                Style {
                    foreground: Color::BrightYellow,
                    background: Color::Default,
                    bold: true,
                },
            ));
        }
        if let Some(status) = state.status() {
            lines.push((
                status.to_owned(),
                Style {
                    foreground: Color::White,
                    background: Color::Default,
                    bold: false,
                },
            ));
        }
        if let Some(progress) = state.progress() {
            lines.push((
                progress_line(progress, usize::from(dimensions.columns())),
                Style {
                    foreground: Color::BrightCyan,
                    background: Color::Default,
                    bold: false,
                },
            ));
        }
    }

    let mut sensitive = None;
    for (offset, (text, style)) in lines
        .into_iter()
        .take(usize::from(dimensions.rows()))
        .enumerate()
    {
        let row = dimensions.rows() - 1 - offset as u16;
        clear_row(scene, row)?;
        write_centered(scene, row, &text, style)?;
        if visible_input_offset == Some(offset) {
            sensitive = Some(SensitiveLayout {
                row,
                columns: dimensions.columns(),
                style,
            });
        }
    }
    Ok(sensitive)
}

fn visible_prefix(text: &str, maximum_cells: usize) -> (&str, usize) {
    if maximum_cells == 0 {
        return ("", 0);
    }
    let mut cells = 0_usize;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let width = sensitive_cell_width(character);
        if cells.saturating_add(width) > maximum_cells {
            break;
        }
        cells += width;
        end = index + character.len_utf8();
    }
    (&text[..end], cells)
}

fn clear_row(scene: &mut Scene, row: u16) -> Result<(), DisplayError> {
    for column in 0..scene.dimensions().columns() {
        scene.set(column, row, Cell::default())?;
    }
    Ok(())
}

fn write_centered(
    scene: &mut Scene,
    row: u16,
    text: &str,
    style: Style,
) -> Result<(), DisplayError> {
    let width = usize::from(scene.dimensions().columns());
    let glyphs: Vec<char> = text.chars().take(width).collect();
    let start = (width.saturating_sub(glyphs.len())) / 2;
    for (offset, glyph) in glyphs.into_iter().enumerate() {
        scene.set((start + offset) as u16, row, Cell::styled(glyph, style)?)?;
    }
    Ok(())
}

fn progress_line(progress: u8, width: usize) -> String {
    let percentage = format!("{progress}%");
    if width < 9 {
        return percentage;
    }

    let bar_width = width.saturating_sub(8).clamp(1, 40);
    let filled = bar_width * usize::from(progress) / 100;
    format!(
        "[{}{}] {progress:3}%",
        "#".repeat(filled),
        "-".repeat(bar_width - filled)
    )
}

#[derive(Debug)]
pub enum EngineError {
    InvalidFramesPerSecond(u16),
    InvalidAnimationCycle(Duration),
    AlreadyRestored,
    NotStarted,
    MissingDimensions,
    Display(DisplayError),
    Restoration(DisplayError),
    OperationAndRestore {
        operation: Box<EngineError>,
        restoration: Box<EngineError>,
    },
    Frame(FrameError),
    State(StateError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFramesPerSecond(value) => write!(
                formatter,
                "animation FPS must be in 1..={MAX_FRAMES_PER_SECOND}, got {value}"
            ),
            Self::InvalidAnimationCycle(value) => write!(
                formatter,
                "animation cycle must be between {MIN_ANIMATION_CYCLE:?} and {MAX_ANIMATION_CYCLE:?}, got {value:?}"
            ),
            Self::AlreadyRestored => formatter.write_str("display engine was already restored"),
            Self::NotStarted => formatter.write_str("display engine was not started"),
            Self::MissingDimensions => {
                formatter.write_str("display backend did not report acquired dimensions")
            }
            Self::Display(error) => error.fmt(formatter),
            Self::Restoration(error) => write!(formatter, "display restoration failed: {error}"),
            Self::OperationAndRestore {
                operation,
                restoration,
            } => write!(formatter, "{operation}; {restoration}"),
            Self::Frame(error) => error.fmt(formatter),
            Self::State(error) => error.fmt(formatter),
        }
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Display(error) | Self::Restoration(error) => Some(error),
            Self::OperationAndRestore { restoration, .. } => Some(restoration.as_ref()),
            Self::Frame(error) => Some(error),
            Self::State(error) => Some(error),
            _ => None,
        }
    }
}

impl EngineError {
    /// Whether the failure includes an unsuccessful display-restoration
    /// attempt and therefore cannot prove that console ownership is safe.
    pub fn restoration_failed(&self) -> bool {
        match self {
            Self::Restoration(_) | Self::OperationAndRestore { .. } => true,
            Self::Display(error) => error.restoration_failed(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell as TimeCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::display::buffer::{BufferBackend, BufferOperation};
    use crate::password::PromptInput;
    use crate::splash::state::{Mode, PromptMetadata};

    use super::*;

    #[derive(Default)]
    struct FakeClock(TimeCell<Duration>);

    impl FakeClock {
        fn set(&self, elapsed: Duration) {
            self.0.set(elapsed);
        }
    }

    impl Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.0.get()
        }
    }

    fn running_state() -> SplashState {
        let mut state = SplashState::new(Mode::Boot);
        state.apply(StateAction::MarkRunning).unwrap();
        state
    }

    struct EnginePrompt {
        input: PromptInput,
    }

    impl EnginePrompt {
        fn new(echo: bool, silent: bool, text: &str) -> Self {
            let mut input = PromptInput::new(128, echo, silent).unwrap();
            for byte in text.bytes() {
                let _ = input.feed_byte(byte);
            }
            Self { input }
        }
    }

    impl PromptCoordinator for EnginePrompt {
        fn poll(&mut self, _state: &mut SplashState) {}

        fn handle_input(&mut self, _state: &mut SplashState, bytes: &mut [u8]) {
            for byte in bytes.iter().copied() {
                let _ = self.input.feed_byte(byte);
            }
            bytes.fill(0);
        }

        fn feedback(&self) -> Option<InputFeedback> {
            Some(self.input.feedback())
        }

        fn with_visible_text(&self, render: &mut dyn FnMut(&str)) {
            self.input.with_visible_text(|text| {
                if let Some(text) = text {
                    render(text);
                }
            });
        }

        fn abandon(&mut self, _state: &mut SplashState) {
            self.input.clear();
        }

        fn enabled(&self) -> bool {
            true
        }
    }

    #[test]
    fn animation_continues_across_multiple_old_duration_boundaries_until_quit() {
        let art = Art::parse("BOOT").unwrap();
        let backend = BufferBackend::new(Dimensions::new(24, 6).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let clock = FakeClock::default();
        let mut state = running_state();
        engine.start(&mut state).unwrap();

        for millis in [0, 2_600, 5_200, 7_800] {
            clock.set(Duration::from_millis(millis));
            assert!(engine.tick(&mut state, &clock).unwrap().frame_rendered);
        }

        assert_eq!(state.lifecycle(), Lifecycle::Running);
        assert_eq!(engine.backend().frames().len(), 4);
        state.apply(StateAction::Quit).unwrap();
        assert!(engine.tick(&mut state, &clock).unwrap().stopped);
        assert!(
            engine
                .backend()
                .operations()
                .contains(&BufferOperation::Restore(RestoreMode::Clear))
        );
    }

    #[test]
    fn fixed_time_and_state_produce_identical_frames() {
        let art = Art::parse("XX\nXX").unwrap();
        let dimensions = Dimensions::new(20, 6).unwrap();
        let mut first = SplashEngine::new(
            BufferBackend::new(dimensions),
            &art,
            None,
            EngineConfig::default(),
        )
        .unwrap();
        let mut second = SplashEngine::new(
            BufferBackend::new(dimensions),
            &art,
            None,
            EngineConfig::default(),
        )
        .unwrap();
        let clock = FakeClock::default();
        clock.set(Duration::from_millis(1_234));
        let mut first_state = running_state();
        let mut second_state = first_state.clone();
        first_state
            .apply(StateAction::SetStatus(Some("Mounting".into())))
            .unwrap();
        second_state
            .apply(StateAction::SetStatus(Some("Mounting".into())))
            .unwrap();
        first_state
            .apply(StateAction::SetProgress(Some(37)))
            .unwrap();
        second_state
            .apply(StateAction::SetProgress(Some(37)))
            .unwrap();
        first.start(&mut first_state).unwrap();
        second.start(&mut second_state).unwrap();
        first.tick(&mut first_state, &clock).unwrap();
        second.tick(&mut second_state, &clock).unwrap();

        assert_eq!(first.backend().frames(), second.backend().frames());
    }

    #[test]
    fn retain_request_keeps_only_pixels_and_releases_display_ownership() {
        let art = Art::parse("X").unwrap();
        let backend = BufferBackend::new(Dimensions::new(8, 3).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let mut state = running_state();
        engine.start(&mut state).unwrap();
        engine.tick_at(&mut state, Duration::ZERO).unwrap();
        state.apply(StateAction::Quit).unwrap();

        engine.shutdown(true).unwrap();
        assert_eq!(engine.backend().state(), DisplayState::Restored);
        assert!(
            engine
                .backend()
                .operations()
                .contains(&BufferOperation::Restore(RestoreMode::RetainPixels))
        );
    }

    #[test]
    fn details_return_event_restores_splash_but_escape_never_toggles_while_prompting() {
        let art = Art::parse("X").unwrap();
        let backend = BufferBackend::new(Dimensions::new(10, 4).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let clock = FakeClock::default();
        let mut state = running_state();
        engine.start(&mut state).unwrap();

        engine
            .backend_mut_for_test()
            .queue_input(InputEvent::Bytes(vec![0x1b]));
        engine.tick(&mut state, &clock).unwrap();
        assert_eq!(state.view(), &View::Details);

        engine
            .backend_mut_for_test()
            .queue_input(InputEvent::ReturnToSplash);
        clock.set(Duration::from_millis(20));
        engine.tick(&mut state, &clock).unwrap();
        assert_eq!(state.view(), &View::Splash);

        state
            .apply(StateAction::BeginPrompt(
                PromptMetadata::new(7, "Disk password").unwrap(),
            ))
            .unwrap();
        engine
            .backend_mut_for_test()
            .queue_input(InputEvent::Bytes(vec![0x1b]));
        clock.set(Duration::from_millis(40));
        engine.tick(&mut state, &clock).unwrap();
        assert!(matches!(state.view(), View::Prompt { .. }));
    }

    #[test]
    fn prompt_input_is_routed_to_coordinator_instead_of_details_state() {
        let art = Art::parse("X").unwrap();
        let backend = BufferBackend::new(Dimensions::new(20, 5).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let clock = FakeClock::default();
        let mut state = running_state();
        state
            .apply(StateAction::BeginPrompt(
                PromptMetadata::new(7, "Disk password").unwrap(),
            ))
            .unwrap();
        let mut prompt = EnginePrompt::new(false, false, "");
        engine.start(&mut state).unwrap();
        engine
            .backend_mut_for_test()
            .queue_input(InputEvent::Bytes(b"ab".to_vec()));

        engine
            .tick_with_prompt(&mut state, &clock, &mut prompt)
            .unwrap();

        assert_eq!(prompt.feedback().unwrap().character_count(), 2);
        assert!(matches!(state.view(), View::Prompt { .. }));
    }

    #[test]
    fn visible_echo_bypasses_scene_and_backend_logs_plaintext() {
        let art = Art::parse("X").unwrap();
        let backend = BufferBackend::new(Dimensions::new(20, 5).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let clock = FakeClock::default();
        let mut state = running_state();
        state
            .apply(StateAction::BeginPrompt(
                PromptMetadata::new(7, "Disk password")
                    .unwrap()
                    .with_echo(true),
            ))
            .unwrap();
        let mut prompt = EnginePrompt::new(true, false, "hunter2");
        engine.start(&mut state).unwrap();

        engine
            .tick_with_prompt(&mut state, &clock, &mut prompt)
            .unwrap();

        let frame_text: String = engine
            .backend()
            .frames()
            .last()
            .unwrap()
            .cells()
            .iter()
            .map(|cell| cell.glyph())
            .collect();
        assert!(!frame_text.contains("hunter2"));
        assert!(!format!("{:?}", engine.backend().operations()).contains("hunter2"));
        assert!(
            engine
                .backend()
                .operations()
                .iter()
                .any(|operation| matches!(
                    operation,
                    BufferOperation::RenderSensitiveText { cells: 7, .. }
                ))
        );
    }

    #[test]
    fn visible_echo_uses_conservative_cells_and_never_wraps_non_ascii() {
        assert_eq!(visible_prefix("界界", 3), ("界", 2));
        assert_eq!(visible_prefix("a界b", 3), ("a界", 3));
        assert_eq!(visible_prefix("🔐", 1), ("", 0));
    }

    #[test]
    fn resize_uses_new_dimensions_even_for_tiny_displays() {
        let art = Art::parse("A LARGE LOGO").unwrap();
        let backend = BufferBackend::new(Dimensions::new(20, 5).unwrap());
        let mut engine = SplashEngine::new(backend, &art, None, EngineConfig::default()).unwrap();
        let clock = FakeClock::default();
        let mut state = running_state();
        engine.start(&mut state).unwrap();
        engine.tick(&mut state, &clock).unwrap();

        let tiny = Dimensions::new(3, 1).unwrap();
        engine
            .backend_mut_for_test()
            .queue_input(InputEvent::Resized(tiny));
        clock.set(Duration::from_millis(1));
        engine.tick(&mut state, &clock).unwrap();
        assert_eq!(engine.backend().frames().last().unwrap().dimensions(), tiny);
    }

    struct RestoreProbe {
        inner: BufferBackend,
        restores: Arc<AtomicUsize>,
        fail_render: bool,
        fail_restore: bool,
    }

    impl DisplayBackend for RestoreProbe {
        fn state(&self) -> DisplayState {
            self.inner.state()
        }
        fn dimensions(&self) -> Option<Dimensions> {
            self.inner.dimensions()
        }
        fn acquire(&mut self) -> Result<(), DisplayError> {
            self.inner.acquire()
        }
        fn show(&mut self) -> Result<(), DisplayError> {
            self.inner.show()
        }
        fn hide(&mut self) -> Result<(), DisplayError> {
            self.inner.hide()
        }
        fn render(&mut self, scene: &Scene) -> Result<(), DisplayError> {
            if self.fail_render {
                return Err(DisplayError::Backend {
                    backend: "restore-probe",
                    operation: "render",
                    source: std::io::Error::other("injected write failure"),
                });
            }
            self.inner.render(scene)
        }
        fn poll_input(&mut self, timeout: Duration) -> Result<Option<InputEvent>, DisplayError> {
            self.inner.poll_input(timeout)
        }
        fn details(&mut self, visible: bool) -> Result<(), DisplayError> {
            self.inner.details(visible)
        }
        fn restore(&mut self) -> Result<(), DisplayError> {
            if self.inner.state() != DisplayState::Restored {
                self.restores.fetch_add(1, Ordering::SeqCst);
            }
            if self.fail_restore {
                return Err(DisplayError::Backend {
                    backend: "restore-probe",
                    operation: "restore",
                    source: std::io::Error::other("injected restoration failure"),
                });
            }
            self.inner.restore()
        }
    }

    fn restore_probe(
        restores: Arc<AtomicUsize>,
        fail_render: bool,
        fail_restore: bool,
    ) -> RestoreProbe {
        RestoreProbe {
            inner: BufferBackend::new(Dimensions::new(10, 4).unwrap()),
            restores,
            fail_render,
            fail_restore,
        }
    }

    #[test]
    fn render_error_fails_open_and_restores_once() {
        let restores = Arc::new(AtomicUsize::new(0));
        let art = Art::parse("X").unwrap();
        let mut engine = SplashEngine::new(
            restore_probe(Arc::clone(&restores), true, false),
            &art,
            None,
            EngineConfig::default(),
        )
        .unwrap();
        let mut state = running_state();
        engine.start(&mut state).unwrap();

        assert!(matches!(
            engine.tick_at(&mut state, Duration::ZERO),
            Err(EngineError::Display(_))
        ));
        assert_eq!(state.lifecycle(), Lifecycle::FailedOpen);
        assert_eq!(restores.load(Ordering::SeqCst), 1);
        drop(engine);
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn operation_and_restoration_errors_remain_distinguishable() {
        let restores = Arc::new(AtomicUsize::new(0));
        let art = Art::parse("X").unwrap();
        let mut engine = SplashEngine::new(
            restore_probe(Arc::clone(&restores), true, true),
            &art,
            None,
            EngineConfig::default(),
        )
        .unwrap();
        let mut state = running_state();
        engine.start(&mut state).unwrap();

        let error = engine
            .tick_at(&mut state, Duration::ZERO)
            .expect_err("render and restoration must fail");
        assert!(matches!(error, EngineError::OperationAndRestore { .. }));
        assert!(error.restoration_failed());
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unwind_drop_is_a_panic_restoration_boundary() {
        let restores = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&restores);
        let result = catch_unwind(AssertUnwindSafe(move || {
            let art = Art::parse("X").unwrap();
            let mut engine = SplashEngine::new(
                restore_probe(observed, false, false),
                &art,
                None,
                EngineConfig::default(),
            )
            .unwrap();
            let mut state = running_state();
            engine.start(&mut state).unwrap();
            panic!("injected panic after display acquisition");
        }));

        assert!(result.is_err());
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }
}
