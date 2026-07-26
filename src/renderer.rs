use std::io;
use std::time::{Duration, Instant};

use crate::animation::{AnimationMetadata, AnsiColor, cell_color_at};
use crate::art::{Art, Layout, Size, layout};
use crate::signals;
use crate::terminal::{TerminalOutput, TerminalSize};

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub duration_ms: u64,
    pub fps: u64,
    pub seed: u64,
    pub no_color: bool,
    pub clear_first: bool,
    pub leave_final: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameOptions {
    pub progress: f32,
    pub no_color: bool,
    pub first_frame: bool,
    pub clear_first: bool,
    pub iteration: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            duration_ms: 2500,
            fps: 30,
            seed: 42,
            no_color: false,
            clear_first: true,
            leave_final: true,
        }
    }
}

pub fn select_art<'a>(
    art: &'a Art,
    small_art: Option<&'a Art>,
    term_size: TerminalSize,
) -> &'a Art {
    let art_fits = art.width <= term_size.width && art.height <= term_size.height;
    if !art_fits {
        return small_art
            .filter(|s| s.width <= term_size.width && s.height <= term_size.height)
            .unwrap_or(art);
    }
    art
}

pub fn generate_frame_bytes(
    art: &Art,
    meta: &AnimationMetadata,
    layout_info: &Layout,
    options: FrameOptions,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);

    if options.first_frame {
        buf.extend_from_slice(b"\x1b[?25l"); // Hide cursor
        AnsiColor::Reset.write_to_buf(&mut buf);
        if options.clear_first {
            buf.extend_from_slice(b"\x1b[2J"); // Clear screen
        }
    }

    let mut active_color: Option<AnsiColor> = None;
    let mut char_buf = [0u8; 4];

    for r in 0..layout_info.visible_height {
        let art_y = layout_info.source_y + r;
        let dest_y_ansi = layout_info.destination_y + r + 1; // 1-based ANSI coord
        let dest_x_ansi = layout_info.destination_x + 1;

        // Position cursor at line start
        let pos_cmd = format!("\x1b[{};{}H", dest_y_ansi, dest_x_ansi);
        buf.extend_from_slice(pos_cmd.as_bytes());

        for c in 0..layout_info.visible_width {
            let art_x = layout_info.source_x + c;

            let cell_opt = meta.cell_at(art_x, art_y);

            let (glyph, color) = match cell_opt {
                Some(cell) => {
                    match cell_color_at(
                        cell,
                        options.progress,
                        art.width,
                        art.height,
                        options.no_color,
                        options.iteration,
                    ) {
                        Some((g, col)) => (g, col),
                        None => (' ', AnsiColor::Reset),
                    }
                }
                None => (' ', AnsiColor::Reset),
            };

            if active_color != Some(color) {
                color.write_to_buf(&mut buf);
                active_color = Some(color);
            }
            let encoded = glyph.encode_utf8(&mut char_buf);
            buf.extend_from_slice(encoded.as_bytes());
        }

        if active_color != Some(AnsiColor::Reset) {
            AnsiColor::Reset.write_to_buf(&mut buf);
            active_color = Some(AnsiColor::Reset);
        }
    }

    buf
}

pub fn build_exit_bytes(layout_info: &Layout, term_size: &TerminalSize) -> Vec<u8> {
    let mut buf = Vec::new();
    AnsiColor::Reset.write_to_buf(&mut buf);
    buf.extend_from_slice(b"\x1b[?25h"); // Show cursor

    let final_y = if term_size.height > 0 {
        (layout_info.destination_y + layout_info.visible_height + 1).min(term_size.height)
    } else {
        1
    };
    let pos_cmd = format!("\x1b[{};1H", final_y);
    buf.extend_from_slice(pos_cmd.as_bytes());
    buf
}

pub fn play_animation<T: TerminalOutput>(
    term: &mut T,
    art: &Art,
    small_art: Option<&Art>,
    options: RenderOptions,
    iteration: usize,
) -> io::Result<()> {
    let term_size = term.dimensions()?;
    let selected_art = select_art(art, small_art, term_size);
    let layout_info = layout(
        selected_art.size(),
        Size {
            width: term_size.width,
            height: term_size.height,
        },
    );
    let meta = AnimationMetadata::new(selected_art, options.seed);

    let fps = options.fps.clamp(1, 60);
    let duration_ms = options.duration_ms.clamp(100, 10000);
    let frame_count = (duration_ms * fps / 1000).max(1) as usize;
    let frame_period = Duration::from_micros(1_000_000 / fps);

    let start_time = Instant::now();

    let render_result = (|| {
        for i in 0..frame_count {
            if signals::should_stop() {
                break;
            }

            let raw_progress = if frame_count <= 1 {
                1.0
            } else {
                i as f32 / (frame_count - 1) as f32
            };
            let smooth_p = raw_progress;

            let frame_bytes = generate_frame_bytes(
                selected_art,
                &meta,
                &layout_info,
                FrameOptions {
                    progress: smooth_p,
                    no_color: options.no_color,
                    first_frame: i == 0,
                    clear_first: options.clear_first && iteration == 0,
                    iteration,
                },
            );

            term.write_frame(&frame_bytes)?;
            term.flush()?;

            let target_deadline = start_time + frame_period * (i as u32 + 1);
            let now = Instant::now();
            if target_deadline > now {
                let sleep_dur = target_deadline - now;
                std::thread::sleep(sleep_dur);
            }
        }
        Ok(())
    })();

    let exit_bytes = build_exit_bytes(&layout_info, &term_size);
    let restore_result = term.write_frame(&exit_bytes).and_then(|()| term.flush());

    match render_result {
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
        Ok(()) => restore_result,
    }
}

pub fn render_final<T: TerminalOutput>(
    term: &mut T,
    art: &Art,
    small_art: Option<&Art>,
    no_color: bool,
) -> io::Result<()> {
    let term_size = term.dimensions()?;
    let selected_art = select_art(art, small_art, term_size);
    let layout_info = layout(
        selected_art.size(),
        Size {
            width: term_size.width,
            height: term_size.height,
        },
    );
    let meta = AnimationMetadata::new(selected_art, 42);

    let frame_bytes = generate_frame_bytes(
        selected_art,
        &meta,
        &layout_info,
        FrameOptions {
            progress: 0.50, // Full logo state at midpoint
            no_color,
            first_frame: true,
            clear_first: true,
            iteration: 0,
        },
    );
    term.write_frame(&frame_bytes)?;

    let exit_bytes = build_exit_bytes(&layout_info, &term_size);
    term.write_frame(&exit_bytes)?;
    term.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::BufferTerminal;
    use std::io;

    #[test]
    fn test_render_final_output() {
        let art = Art::parse("X").unwrap();
        let mut term = BufferTerminal::new(TerminalSize {
            width: 10,
            height: 5,
        });

        render_final(&mut term, &art, None, false).unwrap();
        let output = term.contents_as_string();

        assert!(output.contains("\x1b[?25l")); // Hide cursor
        assert!(output.contains("X")); // Art character
        assert!(output.contains("\x1b[?25h")); // Show cursor
    }

    struct FailAfterFirstFrame {
        writes: usize,
        restoration_attempted: bool,
    }

    impl TerminalOutput for FailAfterFirstFrame {
        fn dimensions(&self) -> io::Result<TerminalSize> {
            Ok(TerminalSize {
                width: 10,
                height: 5,
            })
        }

        fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writes += 1;
            if bytes
                .windows(b"\x1b[?25h".len())
                .any(|window| window == b"\x1b[?25h")
            {
                self.restoration_attempted = true;
                return Ok(());
            }
            if self.writes > 1 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected write failure",
                ));
            }
            Ok(())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn play_attempts_restoration_after_render_error() {
        let art = Art::parse("X").unwrap();
        let mut terminal = FailAfterFirstFrame {
            writes: 0,
            restoration_attempted: false,
        };
        let options = RenderOptions {
            duration_ms: 100,
            fps: 60,
            ..RenderOptions::default()
        };

        let error = play_animation(&mut terminal, &art, None, options, 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(terminal.restoration_attempted);
    }
}
