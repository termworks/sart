//! Display ownership and backend-neutral frame data.
//!
//! A display backend owns presentation only.  None of the APIs in this module
//! can start an init system, reboot, halt, mount, or otherwise control boot.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::atomic::{Ordering, compiler_fence};
use std::time::Duration;

pub mod buffer;

#[cfg(target_os = "linux")]
pub mod drm_kms;
#[cfg(target_os = "linux")]
pub mod text_vt;

/// Hard allocation bound for one logical frame.
pub const MAX_SCENE_CELLS: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    columns: u16,
    rows: u16,
}

impl Dimensions {
    pub fn new(columns: u16, rows: u16) -> Result<Self, SceneError> {
        if columns == 0 || rows == 0 {
            return Err(SceneError::EmptyDimensions { columns, rows });
        }

        let cell_count =
            usize::from(columns)
                .checked_mul(usize::from(rows))
                .ok_or(SceneError::TooLarge {
                    cells: usize::MAX,
                    max: MAX_SCENE_CELLS,
                })?;
        if cell_count > MAX_SCENE_CELLS {
            return Err(SceneError::TooLarge {
                cells: cell_count,
                max: MAX_SCENE_CELLS,
            });
        }

        Ok(Self { columns, rows })
    }

    pub const fn columns(self) -> u16 {
        self.columns
    }

    pub const fn rows(self) -> u16 {
        self.rows
    }

    pub fn cell_count(self) -> usize {
        usize::from(self.columns) * usize::from(self.rows)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    glyph: char,
    style: Style,
}

impl Cell {
    pub fn new(glyph: char) -> Result<Self, SceneError> {
        Self::styled(glyph, Style::default())
    }

    pub fn styled(glyph: char, style: Style) -> Result<Self, SceneError> {
        if glyph.is_control() {
            return Err(SceneError::ControlGlyph {
                codepoint: glyph as u32,
            });
        }
        Ok(Self { glyph, style })
    }

    pub const fn glyph(self) -> char {
        self.glyph
    }

    pub const fn style(self) -> Style {
        self.style
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ',
            style: Style {
                foreground: Color::Default,
                background: Color::Default,
                bold: false,
            },
        }
    }
}

/// A complete logical character-cell frame.
///
/// It intentionally contains no ANSI escape sequences or VT identifiers, so
/// a future DRM backend can consume the same renderer output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scene {
    dimensions: Dimensions,
    cells: Vec<Cell>,
}

impl Scene {
    pub fn blank(dimensions: Dimensions) -> Self {
        Self {
            dimensions,
            cells: vec![Cell::default(); dimensions.cell_count()],
        }
    }

    pub fn new(dimensions: Dimensions, cells: Vec<Cell>) -> Result<Self, SceneError> {
        let expected = dimensions.cell_count();
        if cells.len() != expected {
            return Err(SceneError::WrongCellCount {
                expected,
                actual: cells.len(),
            });
        }
        Ok(Self { dimensions, cells })
    }

    pub fn from_rows(rows: &[&str]) -> Result<Self, SceneError> {
        let row_count = u16::try_from(rows.len()).map_err(|_| SceneError::TooLarge {
            cells: usize::MAX,
            max: MAX_SCENE_CELLS,
        })?;
        let column_count = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let columns = u16::try_from(column_count).map_err(|_| SceneError::TooLarge {
            cells: usize::MAX,
            max: MAX_SCENE_CELLS,
        })?;
        let dimensions = Dimensions::new(columns, row_count)?;
        let mut scene = Self::blank(dimensions);

        for (y, row) in rows.iter().enumerate() {
            for (x, glyph) in row.chars().enumerate() {
                scene.set(x as u16, y as u16, Cell::new(glyph)?)?;
            }
        }
        Ok(scene)
    }

    pub const fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn get(&self, column: u16, row: u16) -> Option<Cell> {
        self.index(column, row).map(|index| self.cells[index])
    }

    pub fn set(&mut self, column: u16, row: u16, cell: Cell) -> Result<(), SceneError> {
        let index = self.index(column, row).ok_or(SceneError::OutOfBounds {
            column,
            row,
            dimensions: self.dimensions,
        })?;
        self.cells[index] = cell;
        Ok(())
    }

    fn index(&self, column: u16, row: u16) -> Option<usize> {
        if column >= self.dimensions.columns || row >= self.dimensions.rows {
            return None;
        }
        Some(usize::from(row) * usize::from(self.dimensions.columns) + usize::from(column))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneError {
    EmptyDimensions {
        columns: u16,
        rows: u16,
    },
    TooLarge {
        cells: usize,
        max: usize,
    },
    WrongCellCount {
        expected: usize,
        actual: usize,
    },
    ControlGlyph {
        codepoint: u32,
    },
    OutOfBounds {
        column: u16,
        row: u16,
        dimensions: Dimensions,
    },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDimensions { columns, rows } => {
                write!(f, "scene dimensions must be non-zero, got {columns}x{rows}")
            }
            Self::TooLarge { cells, max } => {
                write!(f, "scene has {cells} cells, maximum is {max}")
            }
            Self::WrongCellCount { expected, actual } => {
                write!(f, "scene needs {expected} cells, got {actual}")
            }
            Self::ControlGlyph { codepoint } => {
                write!(f, "scene glyph U+{codepoint:04X} is a terminal control")
            }
            Self::OutOfBounds {
                column,
                row,
                dimensions,
            } => write!(
                f,
                "cell ({column}, {row}) is outside {}x{} scene",
                dimensions.columns, dimensions.rows
            ),
        }
    }
}

impl Error for SceneError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayState {
    Unacquired,
    Acquiring,
    Hidden,
    Splash,
    Details,
    Restored,
    FailedOpen,
}

/// Pixel policy for a graceful daemon shutdown.
///
/// Both modes must restore terminal, cursor, keyboard, KD, and VT ownership.
/// `RetainPixels` only asks the backend not to clear its last frame when that
/// can be done safely; it never permits retaining an open/active display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    Clear,
    RetainPixels,
}

impl DisplayState {
    pub const fn owns_resources(self) -> bool {
        matches!(
            self,
            Self::Acquiring | Self::Hidden | Self::Splash | Self::Details
        )
    }
}

#[derive(PartialEq, Eq)]
pub enum InputEvent {
    Bytes(Vec<u8>),
    Resized(Dimensions),
    /// The kernel reports that the user switched back to the backend-owned
    /// splash VT while the original boot console was visible. No bytes are
    /// consumed from that console, so getty and password agents retain input.
    ReturnToSplash,
}

impl fmt::Debug for InputEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes(bytes) => formatter
                .debug_struct("Bytes")
                .field("length", &bytes.len())
                .field("contents", &"<redacted>")
                .finish(),
            Self::Resized(dimensions) => {
                formatter.debug_tuple("Resized").field(dimensions).finish()
            }
            Self::ReturnToSplash => formatter.write_str("ReturnToSplash"),
        }
    }
}

impl Drop for InputEvent {
    fn drop(&mut self) {
        if let Self::Bytes(bytes) = self {
            compiler_fence(Ordering::SeqCst);
            for byte in bytes.iter_mut() {
                // SAFETY: every byte is uniquely borrowed from the live Vec.
                unsafe { std::ptr::write_volatile(byte, 0) };
            }
            compiler_fence(Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
pub enum DisplayError {
    InvalidState {
        operation: &'static str,
        state: DisplayState,
    },
    SizeMismatch {
        expected: Dimensions,
        actual: Dimensions,
    },
    SensitiveTextUnsupported,
    SensitiveTextOutOfBounds,
    UnsafeSensitiveText,
    Scene(SceneError),
    Backend {
        backend: &'static str,
        operation: &'static str,
        source: io::Error,
    },
    /// The triggering display operation failed and the best-effort cleanup
    /// also reported a failure. Both errors are retained so callers never
    /// mistake ambiguous VT ownership for a clean fail-open exit.
    OperationAndRestore {
        operation: Box<DisplayError>,
        restoration: Box<DisplayError>,
    },
}

impl DisplayError {
    pub(crate) fn backend(
        backend: &'static str,
        operation: &'static str,
        source: io::Error,
    ) -> Self {
        Self::Backend {
            backend,
            operation,
            source,
        }
    }

    pub(crate) fn with_restoration(
        operation: DisplayError,
        restoration: Result<(), DisplayError>,
    ) -> Self {
        match restoration {
            Ok(()) => operation,
            Err(restoration) => Self::OperationAndRestore {
                operation: Box::new(operation),
                restoration: Box::new(restoration),
            },
        }
    }

    /// True when a restoration attempt itself failed, as opposed to an
    /// ordinary rendering/acquisition operation that was cleaned up safely.
    pub fn restoration_failed(&self) -> bool {
        matches!(self, Self::OperationAndRestore { .. })
    }
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidState { operation, state } => {
                write!(f, "cannot {operation} display while it is {state:?}")
            }
            Self::SizeMismatch { expected, actual } => write!(
                f,
                "frame is {}x{}, display is {}x{}",
                actual.columns, actual.rows, expected.columns, expected.rows
            ),
            Self::SensitiveTextUnsupported => {
                f.write_str("display backend does not support direct sensitive text")
            }
            Self::SensitiveTextOutOfBounds => {
                f.write_str("sensitive text is outside display bounds")
            }
            Self::UnsafeSensitiveText => {
                f.write_str("sensitive text contains unsafe terminal characters")
            }
            Self::Scene(error) => error.fmt(f),
            Self::Backend {
                backend,
                operation,
                source,
            } => write!(f, "{backend} failed to {operation}: {source}"),
            Self::OperationAndRestore {
                operation,
                restoration,
            } => write!(
                f,
                "{operation}; display restoration also failed: {restoration}"
            ),
        }
    }
}

impl Error for DisplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scene(error) => Some(error),
            Self::Backend { source, .. } => Some(source),
            Self::OperationAndRestore { restoration, .. } => Some(restoration.as_ref()),
            _ => None,
        }
    }
}

impl From<SceneError> for DisplayError {
    fn from(value: SceneError) -> Self {
        Self::Scene(value)
    }
}

/// Lifecycle contract implemented by every display owner.
pub trait DisplayBackend {
    fn state(&self) -> DisplayState;
    fn dimensions(&self) -> Option<Dimensions>;
    fn acquire(&mut self) -> Result<(), DisplayError>;
    fn show(&mut self) -> Result<(), DisplayError>;
    fn hide(&mut self) -> Result<(), DisplayError>;
    fn render(&mut self, scene: &Scene) -> Result<(), DisplayError>;

    /// Render plaintext without placing it in a [`Scene`] or ordinary backend
    /// operation log. Implementations must not retain `text` after return.
    fn render_sensitive_text(
        &mut self,
        _row: u16,
        _column: u16,
        _text: &str,
        _style: Style,
    ) -> Result<(), DisplayError> {
        Err(DisplayError::SensitiveTextUnsupported)
    }
    fn poll_input(&mut self, timeout: Duration) -> Result<Option<InputEvent>, DisplayError>;

    /// `visible = true` returns the original boot console to the user;
    /// `visible = false` returns to the splash display.
    fn details(&mut self, visible: bool) -> Result<(), DisplayError>;

    /// Release all display resources and restore the pre-acquisition state.
    /// Implementations must make this operation idempotent.
    fn restore(&mut self) -> Result<(), DisplayError>;

    fn restore_with_mode(&mut self, mode: RestoreMode) -> Result<(), DisplayError> {
        let _ = mode;
        self.restore()
    }
}

pub(crate) fn validate_sensitive_text(
    dimensions: Dimensions,
    row: u16,
    column: u16,
    text: &str,
) -> Result<usize, DisplayError> {
    let cells = text.chars().try_fold(0_usize, |total, character| {
        total.checked_add(sensitive_cell_width(character))
    });
    let cells = cells.ok_or(DisplayError::SensitiveTextOutOfBounds)?;
    if row >= dimensions.rows()
        || column >= dimensions.columns()
        || cells > usize::from(dimensions.columns() - column)
    {
        return Err(DisplayError::SensitiveTextOutOfBounds);
    }
    if text.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
    }) {
        return Err(DisplayError::UnsafeSensitiveText);
    }
    Ok(cells)
}

/// Conservative terminal-cell estimate for visible secret echo.  ASCII uses
/// one cell; every non-ASCII scalar reserves two. Combining marks are
/// intentionally over-counted so an unknown console width table can never
/// make plaintext wrap beyond the cleared row.
pub(crate) const fn sensitive_cell_width(character: char) -> usize {
    if character.is_ascii() { 1 } else { 2 }
}
