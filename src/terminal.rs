use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: usize,
    pub height: usize,
}

impl TerminalSize {
    pub const DEFAULT: Self = Self {
        width: 80,
        height: 24,
    };
}

pub trait TerminalOutput {
    fn dimensions(&self) -> io::Result<TerminalSize>;
    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
}

pub struct StdoutTerminal {
    stdout: io::Stdout,
    override_size: Option<TerminalSize>,
}

impl StdoutTerminal {
    pub fn new() -> Self {
        Self {
            stdout: io::stdout(),
            override_size: None,
        }
    }

    pub fn with_override(cols: Option<usize>, rows: Option<usize>) -> Self {
        let override_size = match (cols, rows) {
            (Some(c), Some(r)) if c > 0 && r > 0 => Some(TerminalSize {
                width: c,
                height: r,
            }),
            _ => None,
        };
        Self {
            stdout: io::stdout(),
            override_size,
        }
    }
}

impl Default for StdoutTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalOutput for StdoutTerminal {
    fn dimensions(&self) -> io::Result<TerminalSize> {
        if let Some(size) = self.override_size {
            return Ok(size);
        }

        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            let res = libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws);
            if res == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                return Ok(TerminalSize {
                    width: ws.ws_col as usize,
                    height: ws.ws_row as usize,
                });
            }
        }

        Ok(TerminalSize::DEFAULT)
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stdout.write_all(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

pub struct BufferTerminal {
    pub buffer: Vec<u8>,
    pub size: TerminalSize,
}

impl BufferTerminal {
    pub fn new(size: TerminalSize) -> Self {
        Self {
            buffer: Vec::new(),
            size,
        }
    }

    pub fn contents_as_string(&self) -> String {
        String::from_utf8_lossy(&self.buffer).to_string()
    }
}

impl TerminalOutput for BufferTerminal {
    fn dimensions(&self) -> io::Result<TerminalSize> {
        Ok(self.size)
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_terminal() {
        let size = TerminalSize {
            width: 40,
            height: 10,
        };
        let mut term = BufferTerminal::new(size);
        assert_eq!(term.dimensions().unwrap(), size);

        term.write_frame(b"hello").unwrap();
        term.flush().unwrap();
        assert_eq!(term.contents_as_string(), "hello");
    }

    #[test]
    fn test_stdout_terminal_override() {
        let term = StdoutTerminal::with_override(Some(120), Some(40));
        assert_eq!(
            term.dimensions().unwrap(),
            TerminalSize {
                width: 120,
                height: 40
            }
        );
    }
}
