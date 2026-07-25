use crate::art::Art;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiColor {
    Reset,
    DarkGray,
    Red,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Green,
    LightGray,
    BrightWhite,
}

impl AnsiColor {
    pub fn escape_code(self) -> &'static str {
        match self {
            AnsiColor::Reset => "\x1b[0m",
            AnsiColor::DarkGray => "\x1b[90m",
            AnsiColor::Red => "\x1b[31m",
            AnsiColor::Yellow => "\x1b[33m",
            AnsiColor::Blue => "\x1b[34m",
            AnsiColor::Magenta => "\x1b[35m",
            AnsiColor::Cyan => "\x1b[36m",
            AnsiColor::Green => "\x1b[32m",
            AnsiColor::LightGray => "\x1b[37m",
            AnsiColor::BrightWhite => "\x1b[97m",
        }
    }
}

pub fn cell_hash(seed: u64, x: usize, y: usize) -> u64 {
    let mut z = seed ^ ((x as u64) << 32 | (y as u64));
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub fn normalized_hash(seed: u64, x: usize, y: usize) -> f32 {
    (cell_hash(seed, x, y) as f64 / u64::MAX as f64) as f32
}

#[derive(Debug, Clone)]
pub struct AnimatedCell {
    pub x: usize,
    pub y: usize,
    pub glyph: char,
    pub reveal_threshold: f32,
    pub color_phase: u8,
}

pub struct AnimationMetadata {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
    pub cells: Vec<AnimatedCell>,
}

impl AnimationMetadata {
    pub fn new(art: &Art, seed: u64) -> Self {
        let mut cells = Vec::new();
        for y in 0..art.height {
            for x in 0..art.width {
                let glyph = art.get_cell(x, y);
                if glyph == ' ' {
                    continue;
                }

                cells.push(AnimatedCell {
                    x,
                    y,
                    glyph,
                    reveal_threshold: 0.0,
                    color_phase: 0,
                });
            }
        }

        Self {
            seed,
            width: art.width,
            height: art.height,
            cells,
        }
    }
}

pub fn smoothstep(p: f32) -> f32 {
    let clamped = p.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

/// Zero-Instant-Appearing Animation Engine:
///
/// Frame 0 (progress 0.0) starts 100% EMPTY (black) with ZERO pixels visible.
/// Pass 1 [0.00 .. 0.45]: Pixels SLOWLY APPEAR one by one across the screen via color wave.
/// Hold   [0.45 .. 0.55]: Hold full logo in BrightWhite.
/// Pass 2 [0.55 .. 0.95]: Pixels SLOWLY DISAPPEAR into blackness.
/// Hold   [0.95 .. 1.00]: Hold 100% empty black screen.
pub fn cell_color_at(
    cell: &AnimatedCell,
    smooth_progress: f32,
    art_width: usize,
    art_height: usize,
    no_color: bool,
    _iteration: usize,
) -> Option<(char, AnsiColor)> {
    if no_color {
        return Some((cell.glyph, AnsiColor::LightGray));
    }

    let max_span = (art_width + art_height * 3 + 30) as i32;

    let colors_0 = [
        AnsiColor::DarkGray,
        AnsiColor::Red,
        AnsiColor::Yellow,
        AnsiColor::Blue,
        AnsiColor::Magenta,
        AnsiColor::Cyan,
        AnsiColor::BrightWhite,
    ];

    let colors_1 = [
        AnsiColor::Cyan,
        AnsiColor::Magenta,
        AnsiColor::Blue,
        AnsiColor::Yellow,
        AnsiColor::Red,
        AnsiColor::DarkGray,
        AnsiColor::Reset, // Disappeared / Space ' '
    ];

    if smooth_progress < 0.45 {
        // Pass 1: Starts 100% EMPTY (f = -30 at sub_p = 0.0) -> pixels slowly appear
        let sub_p = smooth_progress / 0.45;
        let t = (sub_p * (max_span as f32 + 30.0)) as i32;

        let f = (cell.x as i32 + cell.y as i32 * 3) - max_span + t;
        let off = ((cell_hash(42 ^ (t as u64), cell.x, cell.y) % 15) + 1) as i32;

        for i in (0..=6).rev() {
            if f > (i * 3) + off {
                return Some((cell.glyph, colors_0[i as usize]));
            }
        }
        None // 100% empty black screen (0 pixels visible on frame 0)
    } else if smooth_progress < 0.55 {
        // Hold Full Logo in BrightWhite
        Some((cell.glyph, AnsiColor::BrightWhite))
    } else if smooth_progress < 0.95 {
        // Pass 2: Wave sweeps pixels out to empty space ' '
        let sub_p = (smooth_progress - 0.55) / 0.40;
        let t = (sub_p * (max_span as f32 + 30.0)) as i32;

        let f = (cell.x as i32 + cell.y as i32 * 3) - max_span + t;
        let off = ((cell_hash(42 ^ (t as u64), cell.x, cell.y) % 15) + 1) as i32;

        for i in (0..=6).rev() {
            if f > (i * 3) + off {
                let col = colors_1[i as usize];
                if col == AnsiColor::Reset {
                    return None; // Disappeared into blackness
                }
                return Some((cell.glyph, col));
            }
        }
        Some((cell.glyph, AnsiColor::BrightWhite)) // Full logo before wave erases cell
    } else {
        // Hold 100% Empty Black Screen
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_hash_deterministic() {
        let h1 = cell_hash(42, 10, 5);
        let h2 = cell_hash(42, 10, 5);
        let h3 = cell_hash(43, 10, 5);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_normalized_hash_range() {
        for seed in 0..10 {
            for x in 0..5 {
                for y in 0..5 {
                    let val = normalized_hash(seed, x, y);
                    assert!((0.0..=1.0).contains(&val));
                }
            }
        }
    }

    #[test]
    fn test_smoothstep() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(0.5), 0.5);
    }
}
