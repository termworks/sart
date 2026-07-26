use std::error::Error;
use std::fmt;

use crate::animation::{AnimationMetadata, AnsiColor, cell_color_at};
use crate::art::{Art, Size, layout};
use crate::display::{Cell, Color, Dimensions, Scene, SceneError, Style};
use crate::terminal::TerminalSize;

/// Deterministic renderer that produces backend-neutral character-cell scenes.
pub struct FrameEngine<'art> {
    art: &'art Art,
    metadata: AnimationMetadata,
}

impl<'art> FrameEngine<'art> {
    pub fn new(art: &'art Art, seed: u64) -> Self {
        Self {
            art,
            metadata: AnimationMetadata::new(art, seed),
        }
    }

    pub const fn art_size(&self) -> Size {
        Size {
            width: self.art.width,
            height: self.art.height,
        }
    }

    pub fn render(
        &self,
        terminal_size: TerminalSize,
        progress: f32,
        no_color: bool,
        iteration: usize,
    ) -> Result<Scene, FrameError> {
        let columns = u16::try_from(terminal_size.width)
            .map_err(|_| FrameError::UnsupportedDimensions(terminal_size))?;
        let rows = u16::try_from(terminal_size.height)
            .map_err(|_| FrameError::UnsupportedDimensions(terminal_size))?;
        let dimensions = Dimensions::new(columns, rows)?;
        let mut scene = Scene::blank(dimensions);
        let layout_info = layout(
            self.art.size(),
            Size {
                width: terminal_size.width,
                height: terminal_size.height,
            },
        );

        for row in 0..layout_info.visible_height {
            let art_y = layout_info.source_y + row;
            for column in 0..layout_info.visible_width {
                let art_x = layout_info.source_x + column;
                let Some(animated) = self.metadata.cell_at(art_x, art_y) else {
                    continue;
                };
                let Some((glyph, color)) = cell_color_at(
                    animated,
                    progress,
                    self.art.width,
                    self.art.height,
                    no_color,
                    iteration,
                ) else {
                    continue;
                };

                let destination_x = u16::try_from(layout_info.destination_x + column)
                    .map_err(|_| FrameError::UnsupportedDimensions(terminal_size))?;
                let destination_y = u16::try_from(layout_info.destination_y + row)
                    .map_err(|_| FrameError::UnsupportedDimensions(terminal_size))?;
                let cell = Cell::styled(
                    glyph,
                    Style {
                        foreground: map_color(color),
                        background: Color::Default,
                        bold: false,
                    },
                )?;
                scene.set(destination_x, destination_y, cell)?;
            }
        }

        Ok(scene)
    }
}

fn map_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Reset => Color::Default,
        AnsiColor::DarkGray => Color::BrightBlack,
        AnsiColor::Red => Color::Red,
        AnsiColor::Green => Color::Green,
        AnsiColor::Yellow => Color::Yellow,
        AnsiColor::Blue => Color::Blue,
        AnsiColor::Magenta => Color::Magenta,
        AnsiColor::Cyan => Color::Cyan,
        AnsiColor::LightGray => Color::White,
        AnsiColor::BrightRed => Color::BrightRed,
        AnsiColor::BrightGreen => Color::BrightGreen,
        AnsiColor::BrightYellow => Color::BrightYellow,
        AnsiColor::BrightBlue => Color::BrightBlue,
        AnsiColor::BrightMagenta => Color::BrightMagenta,
        AnsiColor::BrightCyan => Color::BrightCyan,
        AnsiColor::BrightWhite => Color::BrightWhite,
    }
}

#[derive(Debug)]
pub enum FrameError {
    UnsupportedDimensions(TerminalSize),
    Scene(SceneError),
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDimensions(size) => write!(
                formatter,
                "terminal dimensions {}x{} are unsupported",
                size.width, size.height
            ),
            Self::Scene(error) => error.fmt(formatter),
        }
    }
}

impl Error for FrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scene(error) => Some(error),
            Self::UnsupportedDimensions(_) => None,
        }
    }
}

impl From<SceneError> for FrameError {
    fn from(value: SceneError) -> Self {
        Self::Scene(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_is_centered_and_backend_neutral() {
        let art = Art::parse("X").unwrap();
        let engine = FrameEngine::new(&art, 42);
        let scene = engine
            .render(
                TerminalSize {
                    width: 5,
                    height: 3,
                },
                0.5,
                true,
                0,
            )
            .unwrap();

        assert_eq!(scene.dimensions(), Dimensions::new(5, 3).unwrap());
        assert_eq!(scene.get(2, 1).unwrap().glyph(), 'X');
        assert_eq!(scene.get(2, 1).unwrap().style().foreground, Color::White);
        assert_eq!(scene.get(0, 0).unwrap(), Cell::default());
    }

    #[test]
    fn zero_sized_terminals_are_rejected() {
        let art = Art::parse("X").unwrap();
        let error = FrameEngine::new(&art, 42)
            .render(
                TerminalSize {
                    width: 0,
                    height: 24,
                },
                0.5,
                false,
                0,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            FrameError::Scene(SceneError::EmptyDimensions { .. })
        ));
    }
}
