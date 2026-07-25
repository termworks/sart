use std::io;
use std::time::{Duration, Instant};

use crate::animation::{cell_color_at, AnimationMetadata, AnsiColor};
use crate::art::{layout, Art, Layout, Size};
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

pub fn select_art<'a>(art: &'a Art, small_art: Option<&'a Art>, term_size: TerminalSize) -> &'a Art {
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
    smooth_progress: f32,
    no_color: bool,
    is_first_frame: bool,
    clear_first: bool,
    iteration: usize,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);

    if is_first_frame {
        buf.extend_from_slice(b"\x1b[?25l"); // Hide cursor
        AnsiColor::Reset.write_to_buf(&mut buf);
        if clear_first {
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

            // Find matching cell in metadata
            let cell_opt = meta
                .cells
                .iter()
                .find(|cell| cell.x == art_x && cell.y == art_y);

            let (glyph, color) = match cell_opt {
                Some(cell) => {
                    match cell_color_at(cell, smooth_progress, art.width, art.height, no_color, iteration) {
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
    let layout_info = layout(selected_art.size(), Size { width: term_size.width, height: term_size.height });
    let meta = AnimationMetadata::new(selected_art, options.seed);

    let fps = options.fps.clamp(1, 60);
    let duration_ms = options.duration_ms.clamp(100, 10000);
    let frame_count = (duration_ms * fps / 1000).max(1) as usize;
    let frame_period = Duration::from_micros(1_000_000 / fps);

    let start_time = Instant::now();

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
            smooth_p,
            options.no_color,
            i == 0,
            options.clear_first && iteration == 0,
            iteration,
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



    let exit_bytes = build_exit_bytes(&layout_info, &term_size);
    term.write_frame(&exit_bytes)?;
    term.flush()?;

    Ok(())
}

pub fn render_final<T: TerminalOutput>(
    term: &mut T,
    art: &Art,
    small_art: Option<&Art>,
    no_color: bool,
) -> io::Result<()> {
    let term_size = term.dimensions()?;
    let selected_art = select_art(art, small_art, term_size);
    let layout_info = layout(selected_art.size(), Size { width: term_size.width, height: term_size.height });
    let meta = AnimationMetadata::new(selected_art, 42);

    let frame_bytes = generate_frame_bytes(
        selected_art,
        &meta,
        &layout_info,
        0.50, // Full logo state at midpoint
        no_color,
        true,
        true,
        0,
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

    #[test]
    fn test_render_final_output() {
        let art = Art::parse("X").unwrap();
        let mut term = BufferTerminal::new(TerminalSize { width: 10, height: 5 });

        render_final(&mut term, &art, None, false).unwrap();
        let output = term.contents_as_string();

        assert!(output.contains("\x1b[?25l")); // Hide cursor
        assert!(output.contains("X"));          // Art character
        assert!(output.contains("\x1b[?25h")); // Show cursor
    }
}
