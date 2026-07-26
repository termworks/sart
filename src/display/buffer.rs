//! Deterministic in-memory display used by state-machine and daemon tests.

use std::collections::VecDeque;
use std::time::Duration;

use super::{
    Dimensions, DisplayBackend, DisplayError, DisplayState, InputEvent, RestoreMode, Scene, Style,
    validate_sensitive_text,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferOperation {
    Acquire,
    Show,
    Hide,
    Render,
    RenderSensitiveText { row: u16, column: u16, cells: usize },
    PollInput(Duration),
    Details(bool),
    Restore(RestoreMode),
}

#[derive(Debug)]
pub struct BufferBackend {
    dimensions: Dimensions,
    state: DisplayState,
    frames: Vec<Scene>,
    input: VecDeque<InputEvent>,
    operations: Vec<BufferOperation>,
}

impl BufferBackend {
    pub fn new(dimensions: Dimensions) -> Self {
        Self {
            dimensions,
            state: DisplayState::Unacquired,
            frames: Vec::new(),
            input: VecDeque::new(),
            operations: Vec::new(),
        }
    }

    pub fn queue_input(&mut self, event: InputEvent) {
        self.input.push_back(event);
    }

    pub fn frames(&self) -> &[Scene] {
        &self.frames
    }

    pub fn operations(&self) -> &[BufferOperation] {
        &self.operations
    }

    fn require_owned(&self, operation: &'static str) -> Result<(), DisplayError> {
        if self.state.owns_resources() && self.state != DisplayState::Acquiring {
            Ok(())
        } else {
            Err(DisplayError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

impl DisplayBackend for BufferBackend {
    fn state(&self) -> DisplayState {
        self.state
    }

    fn dimensions(&self) -> Option<Dimensions> {
        self.state.owns_resources().then_some(self.dimensions)
    }

    fn acquire(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Unacquired => {
                self.operations.push(BufferOperation::Acquire);
                self.state = DisplayState::Hidden;
                Ok(())
            }
            DisplayState::Hidden | DisplayState::Splash | DisplayState::Details => Ok(()),
            state => Err(DisplayError::InvalidState {
                operation: "acquire",
                state,
            }),
        }
    }

    fn show(&mut self) -> Result<(), DisplayError> {
        self.require_owned("show")?;
        if self.state != DisplayState::Splash {
            self.operations.push(BufferOperation::Show);
            self.state = DisplayState::Splash;
        }
        Ok(())
    }

    fn hide(&mut self) -> Result<(), DisplayError> {
        self.require_owned("hide")?;
        if self.state != DisplayState::Hidden {
            self.operations.push(BufferOperation::Hide);
            self.state = DisplayState::Hidden;
        }
        Ok(())
    }

    fn render(&mut self, scene: &Scene) -> Result<(), DisplayError> {
        if self.state != DisplayState::Splash {
            return Err(DisplayError::InvalidState {
                operation: "render",
                state: self.state,
            });
        }
        if scene.dimensions() != self.dimensions {
            return Err(DisplayError::SizeMismatch {
                expected: self.dimensions,
                actual: scene.dimensions(),
            });
        }
        self.operations.push(BufferOperation::Render);
        self.frames.push(scene.clone());
        Ok(())
    }

    fn render_sensitive_text(
        &mut self,
        row: u16,
        column: u16,
        text: &str,
        _style: Style,
    ) -> Result<(), DisplayError> {
        if self.state != DisplayState::Splash {
            return Err(DisplayError::InvalidState {
                operation: "render sensitive text",
                state: self.state,
            });
        }
        let cells = validate_sensitive_text(self.dimensions, row, column, text)?;
        self.operations
            .push(BufferOperation::RenderSensitiveText { row, column, cells });
        Ok(())
    }

    fn poll_input(&mut self, timeout: Duration) -> Result<Option<InputEvent>, DisplayError> {
        self.require_owned("poll input")?;
        self.operations.push(BufferOperation::PollInput(timeout));
        let event = match self.state {
            DisplayState::Splash => self.input.pop_front(),
            DisplayState::Details
                if matches!(self.input.front(), Some(InputEvent::ReturnToSplash)) =>
            {
                self.input.pop_front()
            }
            _ => None,
        };
        if let Some(InputEvent::Resized(dimensions)) = event.as_ref() {
            self.dimensions = *dimensions;
        }
        Ok(event)
    }

    fn details(&mut self, visible: bool) -> Result<(), DisplayError> {
        self.require_owned("change details visibility")?;
        let target = if visible {
            DisplayState::Details
        } else {
            DisplayState::Splash
        };
        if self.state != target {
            self.operations.push(BufferOperation::Details(visible));
            self.state = target;
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<(), DisplayError> {
        self.restore_with_mode(RestoreMode::Clear)
    }

    fn restore_with_mode(&mut self, mode: RestoreMode) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Restored | DisplayState::FailedOpen => Ok(()),
            _ => {
                self.operations.push(BufferOperation::Restore(mode));
                self.state = DisplayState::Restored;
                Ok(())
            }
        }
    }
}
