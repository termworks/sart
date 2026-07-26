use crate::art::Art;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiColor {
    Reset,
    DarkGray,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    LightGray,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl AnsiColor {
    pub fn write_to_buf(self, buf: &mut Vec<u8>) {
        match self {
            AnsiColor::Reset => buf.extend_from_slice(b"\x1b[0m"),
            AnsiColor::DarkGray => buf.extend_from_slice(b"\x1b[90m"),
            AnsiColor::Red => buf.extend_from_slice(b"\x1b[31m"),
            AnsiColor::Green => buf.extend_from_slice(b"\x1b[32m"),
            AnsiColor::Yellow => buf.extend_from_slice(b"\x1b[33m"),
            AnsiColor::Blue => buf.extend_from_slice(b"\x1b[34m"),
            AnsiColor::Magenta => buf.extend_from_slice(b"\x1b[35m"),
            AnsiColor::Cyan => buf.extend_from_slice(b"\x1b[36m"),
            AnsiColor::LightGray => buf.extend_from_slice(b"\x1b[37m"),
            AnsiColor::BrightRed => buf.extend_from_slice(b"\x1b[91m"),
            AnsiColor::BrightGreen => buf.extend_from_slice(b"\x1b[92m"),
            AnsiColor::BrightYellow => buf.extend_from_slice(b"\x1b[93m"),
            AnsiColor::BrightBlue => buf.extend_from_slice(b"\x1b[94m"),
            AnsiColor::BrightMagenta => buf.extend_from_slice(b"\x1b[95m"),
            AnsiColor::BrightCyan => buf.extend_from_slice(b"\x1b[96m"),
            AnsiColor::BrightWhite => buf.extend_from_slice(b"\x1b[97m"),
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
    cell_indices: Vec<Option<usize>>,
}

impl AnimationMetadata {
    pub fn new(art: &Art, seed: u64) -> Self {
        let mut cells = Vec::new();
        let mut cell_indices = vec![None; art.width.saturating_mul(art.height)];
        for y in 0..art.height {
            for x in 0..art.width {
                let glyph = art.get_cell(x, y);
                if glyph == ' ' {
                    continue;
                }

                let cell_index = cells.len();
                cells.push(AnimatedCell {
                    x,
                    y,
                    glyph,
                    reveal_threshold: 0.0,
                    color_phase: 0,
                });
                cell_indices[y * art.width + x] = Some(cell_index);
            }
        }

        Self {
            seed,
            width: art.width,
            height: art.height,
            cells,
            cell_indices,
        }
    }

    /// Return animation metadata for a cell in constant time.
    pub fn cell_at(&self, x: usize, y: usize) -> Option<&AnimatedCell> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let index = self
            .cell_indices
            .get(y * self.width + x)
            .copied()
            .flatten()?;
        self.cells.get(index)
    }
}

pub fn smoothstep(p: f32) -> f32 {
    let clamped = p.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

/// Compute cell projected position for any of 12 30-degree angles around the 360-degree circle:
fn angle_effective_pos(x: usize, y: usize, width: usize, height: usize, angle_idx: usize) -> i32 {
    let cx = x as f32 - (width as f32 / 2.0);
    let cy = (y as f32 * 2.0) - (height as f32); // 2:1 terminal character aspect correction

    let angle_rad = (angle_idx % 12) as f32 * (std::f32::consts::PI / 6.0);
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    (cx * cos_a + cy * sin_a) as i32
}

/// 12-Angle Directional Sweep Animation Engine (every 30 degrees):
pub fn cell_color_at(
    cell: &AnimatedCell,
    smooth_progress: f32,
    art_width: usize,
    art_height: usize,
    no_color: bool,
    iteration: usize,
) -> Option<(char, AnsiColor)> {
    if no_color {
        return Some((cell.glyph, AnsiColor::LightGray));
    }

    let max_span = (art_width
        .saturating_add(art_height.saturating_mul(2))
        .saturating_add(30) as f32
        * 0.75) as i32;

    // Pick random 12-angle sweep directions per iteration
    let dir_1 = (cell_hash(iteration as u64 ^ 0x3000, 0, 0) % 12) as usize;
    let dir_2 = ((cell_hash(iteration as u64 ^ 0x4000, 0, 0) + 1) % 12) as usize;

    // 12 standard initramfs-compatible ANSI colors
    let initramfs_12_palette = [
        AnsiColor::DarkGray,
        AnsiColor::Red,
        AnsiColor::BrightRed,
        AnsiColor::Yellow,
        AnsiColor::BrightYellow,
        AnsiColor::Green,
        AnsiColor::BrightGreen,
        AnsiColor::Cyan,
        AnsiColor::BrightCyan,
        AnsiColor::Blue,
        AnsiColor::BrightBlue,
        AnsiColor::Magenta,
    ];

    let shift_1 = iteration.wrapping_mul(5).wrapping_add(1) % initramfs_12_palette.len();
    let mut colors_0 = [AnsiColor::Reset; 7];
    for i in 0..6 {
        colors_0[i] = initramfs_12_palette[(i + shift_1) % initramfs_12_palette.len()];
    }
    colors_0[6] = AnsiColor::BrightWhite;

    let shift_2 = iteration.wrapping_mul(5).wrapping_add(7) % initramfs_12_palette.len();
    let mut colors_1 = [AnsiColor::Reset; 7];
    for i in 0..6 {
        colors_1[i] = initramfs_12_palette
            [initramfs_12_palette.len() - 1 - (i + shift_2) % initramfs_12_palette.len()];
    }
    colors_1[6] = AnsiColor::Reset;

    if smooth_progress < 0.45 {
        // Pass 1: Random 12-angle direction sweep in
        let sub_p = smooth_progress / 0.45;
        let t = (sub_p * (max_span as f32 * 2.0 + 30.0)) as i32;

        let pos = angle_effective_pos(cell.x, cell.y, art_width, art_height, dir_1);
        let f = pos - max_span + t;
        let off = ((cell_hash(42 ^ (t as u64), cell.x, cell.y) % 15) + 1) as i32;

        for i in (0..=6).rev() {
            if f > (i * 3) + off {
                return Some((cell.glyph, colors_0[i as usize]));
            }
        }
        None // 100% empty black screen
    } else if smooth_progress < 0.55 {
        // Hold Full Logo in BrightWhite
        Some((cell.glyph, AnsiColor::BrightWhite))
    } else if smooth_progress < 0.95 {
        // Pass 2: Random 12-angle direction sweep out
        let sub_p = (smooth_progress - 0.55) / 0.40;
        let t = (sub_p * (max_span as f32 * 2.0 + 30.0)) as i32;

        let pos = angle_effective_pos(cell.x, cell.y, art_width, art_height, dir_2);
        let f = pos - max_span + t;
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

    #[test]
    fn test_metadata_cell_lookup() {
        let art = Art::parse("X X\n XX").unwrap();
        let metadata = AnimationMetadata::new(&art, 42);

        assert_eq!(metadata.cell_at(0, 0).map(|cell| cell.glyph), Some('X'));
        assert!(metadata.cell_at(1, 0).is_none());
        assert_eq!(metadata.cell_at(2, 1).map(|cell| cell.glyph), Some('X'));
        assert!(metadata.cell_at(3, 0).is_none());
        assert!(metadata.cell_at(0, 2).is_none());
    }
}
