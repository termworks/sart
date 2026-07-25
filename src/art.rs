use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Art {
    pub width: usize,
    pub height: usize,
    pub lines: Vec<Vec<char>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub source_x: usize,
    pub source_y: usize,
    pub visible_width: usize,
    pub visible_height: usize,
    pub destination_x: usize,
    pub destination_y: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Empty,
    NoVisibleCharacters,
    ContainsTab,
    ContainsNul,
    ContainsStandaloneCarriageReturn,
    ExceedsMaxWidth { width: usize, max: usize },
    ExceedsMaxHeight { height: usize, max: usize },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::Empty => write!(f, "art file is empty"),
            ValidationError::NoVisibleCharacters => write!(f, "art contains no non-space visible characters"),
            ValidationError::ContainsTab => write!(f, "art contains tab characters"),
            ValidationError::ContainsNul => write!(f, "art contains NUL bytes"),
            ValidationError::ContainsStandaloneCarriageReturn => {
                write!(f, "art contains un-normalized carriage return '\\r'")
            }
            ValidationError::ExceedsMaxWidth { width, max } => {
                write!(f, "art width ({} cols) exceeds maximum allowed ({} cols)", width, max)
            }
            ValidationError::ExceedsMaxHeight { height, max } => {
                write!(f, "art height ({} rows) exceeds maximum allowed ({} rows)", height, max)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

pub const MAX_ART_WIDTH: usize = 512;
pub const MAX_ART_HEIGHT: usize = 256;

impl Art {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        Self::parse_with_limits(input, MAX_ART_WIDTH, MAX_ART_HEIGHT)
    }

    pub fn parse_with_limits(
        input: &str,
        max_width: usize,
        max_height: usize,
    ) -> Result<Self, ValidationError> {
        if input.is_empty() {
            return Err(ValidationError::Empty);
        }

        // Normalize CRLF to LF
        let normalized = input.replace("\r\n", "\n");

        for ch in normalized.chars() {
            if ch == '\r' {
                return Err(ValidationError::ContainsStandaloneCarriageReturn);
            }
            if ch == '\0' {
                return Err(ValidationError::ContainsNul);
            }
            if ch == '\t' {
                return Err(ValidationError::ContainsTab);
            }
        }

        let raw_lines: Vec<&str> = normalized.split('\n').collect();

        // Find non-empty lines bounds
        let mut first = 0;
        while first < raw_lines.len() && raw_lines[first].trim_end().is_empty() {
            first += 1;
        }

        if first >= raw_lines.len() {
            return Err(ValidationError::NoVisibleCharacters);
        }

        let mut last = raw_lines.len() - 1;
        while last > first && raw_lines[last].trim_end().is_empty() {
            last -= 1;
        }

        let mut lines = Vec::new();
        let mut max_len = 0;
        let mut has_non_space = false;

        for line in &raw_lines[first..=last] {
            let trimmed_right = line.trim_end();
            let line_chars: Vec<char> = trimmed_right.chars().collect();
            if line_chars.iter().any(|&c| c != ' ') {
                has_non_space = true;
            }
            if line_chars.len() > max_len {
                max_len = line_chars.len();
            }
            lines.push(line_chars);
        }

        if !has_non_space {
            return Err(ValidationError::NoVisibleCharacters);
        }

        let height = lines.len();
        let width = max_len;

        if height > max_height {
            return Err(ValidationError::ExceedsMaxHeight { height, max: max_height });
        }
        if width > max_width {
            return Err(ValidationError::ExceedsMaxWidth { width, max: max_width });
        }

        Ok(Art {
            width,
            height,
            lines,
        })
    }

    pub fn size(&self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    pub fn get_cell(&self, x: usize, y: usize) -> char {
        if y < self.lines.len() {
            let line = &self.lines[y];
            if x < line.len() {
                return line[x];
            }
        }
        ' '
    }
}

pub fn layout(art_size: Size, terminal_size: Size) -> Layout {
    if terminal_size.width == 0 || terminal_size.height == 0 {
        return Layout {
            source_x: 0,
            source_y: 0,
            visible_width: 0,
            visible_height: 0,
            destination_x: 0,
            destination_y: 0,
        };
    }

    let (visible_width, source_x, destination_x) = if art_size.width <= terminal_size.width {
        let dest_x = (terminal_size.width - art_size.width) / 2;
        (art_size.width, 0, dest_x)
    } else {
        let src_x = (art_size.width - terminal_size.width) / 2;
        (terminal_size.width, src_x, 0)
    };

    let (visible_height, source_y, destination_y) = if art_size.height <= terminal_size.height {
        let dest_y = (terminal_size.height - art_size.height) / 2;
        (art_size.height, 0, dest_y)
    } else {
        let src_y = (art_size.height - terminal_size.height) / 2;
        (terminal_size.height, src_y, 0)
    };

    Layout {
        source_x,
        source_y,
        visible_width,
        visible_height,
        destination_x,
        destination_y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid() {
        let input = "  foo  \r\n  bar\n";
        let art = Art::parse(input).unwrap();
        assert_eq!(art.width, 5); // "  foo" length = 5
        assert_eq!(art.height, 2);
        assert_eq!(art.get_cell(0, 0), ' ');
        assert_eq!(art.get_cell(2, 0), 'f');
    }

    #[test]
    fn test_parse_unicode_blocks() {
        let input = " ▄▄██ \n ▀▀██ ";
        let art = Art::parse(input).unwrap();
        assert_eq!(art.width, 5);
        assert_eq!(art.height, 2);
        assert_eq!(art.get_cell(1, 0), '▄');
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(Art::parse("").unwrap_err(), ValidationError::Empty);
        assert_eq!(Art::parse("   \n  \n").unwrap_err(), ValidationError::NoVisibleCharacters);
    }

    #[test]
    fn test_parse_invalid_chars() {
        assert_eq!(Art::parse("hello\tworld").unwrap_err(), ValidationError::ContainsTab);
        assert_eq!(Art::parse("hello\0world").unwrap_err(), ValidationError::ContainsNul);
        assert_eq!(Art::parse("hello\rworld").unwrap_err(), ValidationError::ContainsStandaloneCarriageReturn);
    }

    #[test]
    fn test_layout_fitting() {
        let art_size = Size { width: 10, height: 4 };
        let term_size = Size { width: 80, height: 24 };
        let l = layout(art_size, term_size);

        assert_eq!(l.visible_width, 10);
        assert_eq!(l.visible_height, 4);
        assert_eq!(l.source_x, 0);
        assert_eq!(l.source_y, 0);
        assert_eq!(l.destination_x, 35);
        assert_eq!(l.destination_y, 10);
    }
}
