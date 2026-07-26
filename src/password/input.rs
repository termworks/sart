use super::secure::{SecretError, SecretProtection, SecureSecret};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoMode {
    /// Render one generic indicator per Unicode scalar value.
    Obscured,
    /// The renderer may expose plaintext only inside a short-lived callback.
    Visible,
    /// Render no indication that input was received.
    Silent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    Character(char),
    Enter,
    Backspace,
    Clear,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFeedback {
    character_count: usize,
    echo_mode: EchoMode,
}

impl InputFeedback {
    pub fn character_count(self) -> usize {
        self.character_count
    }

    pub fn echo_mode(self) -> EchoMode {
        self.echo_mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRejection {
    InvalidUtf8,
    ControlCharacter,
    MaximumLength,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOutcome {
    Pending,
    Changed(InputFeedback),
    Submit,
    Cancelled,
    Rejected(InputRejection),
}

/// Stateful UTF-8 input editor backed by protected secret memory.
///
/// This type intentionally implements neither `Debug` nor `Clone`. Its public
/// feedback contains only character counts and presentation policy.
///
/// ```compile_fail
/// use bootart::password::PromptInput;
/// let input = PromptInput::new(32, false, false).unwrap();
/// println!("{input:?}");
/// ```
pub struct PromptInput {
    secret: SecureSecret,
    echo_mode: EchoMode,
    character_count: usize,
    pending_utf8: [u8; 4],
    pending_length: usize,
    pending_expected: usize,
}

impl PromptInput {
    pub fn new(capacity: usize, echo: bool, silent: bool) -> Result<Self, SecretError> {
        let echo_mode = if silent {
            EchoMode::Silent
        } else if echo {
            EchoMode::Visible
        } else {
            EchoMode::Obscured
        };
        Ok(Self {
            secret: SecureSecret::new(capacity)?,
            echo_mode,
            character_count: 0,
            pending_utf8: [0; 4],
            pending_length: 0,
            pending_expected: 0,
        })
    }

    pub fn feedback(&self) -> InputFeedback {
        InputFeedback {
            character_count: if self.echo_mode == EchoMode::Silent {
                0
            } else {
                self.character_count
            },
            echo_mode: self.echo_mode,
        }
    }

    pub fn protection(&self) -> SecretProtection {
        self.secret.protection()
    }

    pub fn is_empty(&self) -> bool {
        self.secret.is_empty()
    }

    /// Feed one decoded key from a VT input decoder.
    pub fn handle_key(&mut self, key: PromptKey) -> InputOutcome {
        self.reset_pending_utf8();
        match key {
            PromptKey::Character(character) => self.push_character(character),
            PromptKey::Enter => InputOutcome::Submit,
            PromptKey::Backspace => self.backspace(),
            PromptKey::Clear => {
                self.secret.clear();
                self.character_count = 0;
                InputOutcome::Changed(self.feedback())
            }
            PromptKey::Cancel => {
                self.clear();
                InputOutcome::Cancelled
            }
        }
    }

    /// Feed raw VT bytes, including split UTF-8 sequences.
    ///
    /// Enter (`NUL`, CR, or LF), Backspace (`BS` or DEL), Ctrl-U, and cancel
    /// (`ESC`, Ctrl-C, or Ctrl-D) are recognized only outside an incomplete
    /// UTF-8 sequence. Other control bytes are rejected.
    pub fn feed_byte(&mut self, byte: u8) -> InputOutcome {
        if self.pending_length == 0 {
            match byte {
                0 | b'\r' | b'\n' => return InputOutcome::Submit,
                8 | 127 => return self.backspace(),
                21 => {
                    self.secret.clear();
                    self.character_count = 0;
                    return InputOutcome::Changed(self.feedback());
                }
                3 | 4 | 27 => {
                    self.clear();
                    return InputOutcome::Cancelled;
                }
                0x20..=0x7e => return self.push_character(char::from(byte)),
                0xc2..=0xdf => self.pending_expected = 2,
                0xe0..=0xef => self.pending_expected = 3,
                0xf0..=0xf4 => self.pending_expected = 4,
                0x80..=0xbf | 0xc0..=0xc1 | 0xf5..=0xff => {
                    return InputOutcome::Rejected(InputRejection::InvalidUtf8);
                }
                _ => return InputOutcome::Rejected(InputRejection::ControlCharacter),
            }
            self.pending_utf8[0] = byte;
            self.pending_length = 1;
            return InputOutcome::Pending;
        }

        if !(0x80..=0xbf).contains(&byte) || self.pending_length >= self.pending_expected {
            self.reset_pending_utf8();
            return InputOutcome::Rejected(InputRejection::InvalidUtf8);
        }
        self.pending_utf8[self.pending_length] = byte;
        self.pending_length += 1;
        if self.pending_length != self.pending_expected {
            return InputOutcome::Pending;
        }

        let length = self.pending_length;
        let decoded = std::str::from_utf8(&self.pending_utf8[..length])
            .ok()
            .and_then(|text| text.chars().next().filter(|_| text.chars().count() == 1));
        self.reset_pending_utf8();
        match decoded {
            Some(character) => self.push_character(character),
            None => InputOutcome::Rejected(InputRejection::InvalidUtf8),
        }
    }

    /// Expose visible plaintext only during a renderer-owned callback.
    ///
    /// Obscured and silent prompts always pass `None`.
    pub fn with_visible_text<R>(&self, render: impl FnOnce(Option<&str>) -> R) -> R {
        if self.echo_mode == EchoMode::Visible {
            self.secret.expose(|bytes| {
                // PromptInput accepts only complete UTF-8 scalar values.
                let text = std::str::from_utf8(bytes).expect("PromptInput maintains UTF-8");
                render(Some(text))
            })
        } else {
            render(None)
        }
    }

    pub fn clear(&mut self) {
        self.secret.clear();
        self.character_count = 0;
        self.reset_pending_utf8();
    }

    /// Deliver a submitted secret to one dedicated transport callback.
    ///
    /// A drop guard clears the buffer after the callback returns or unwinds.
    /// The callback must not copy or retain the bytes.
    pub fn finish_with<R>(&mut self, deliver: impl FnOnce(&mut SecureSecret) -> R) -> R {
        struct ClearGuard<'a>(&'a mut PromptInput);

        impl Drop for ClearGuard<'_> {
            fn drop(&mut self) {
                self.0.clear();
            }
        }

        let guard = ClearGuard(self);
        let result = deliver(&mut guard.0.secret);
        drop(guard);
        result
    }

    fn push_character(&mut self, character: char) -> InputOutcome {
        if unsafe_for_terminal(character) {
            return InputOutcome::Rejected(InputRejection::ControlCharacter);
        }
        match self.secret.push_char(character) {
            Ok(()) => {
                self.character_count += 1;
                InputOutcome::Changed(self.feedback())
            }
            Err(SecretError::TooLong { .. }) => {
                InputOutcome::Rejected(InputRejection::MaximumLength)
            }
            Err(_) => InputOutcome::Rejected(InputRejection::InvalidUtf8),
        }
    }

    fn backspace(&mut self) -> InputOutcome {
        match self.secret.pop_char() {
            Ok(Some(_)) => self.character_count = self.character_count.saturating_sub(1),
            Ok(None) => {}
            Err(_) => {
                self.clear();
                return InputOutcome::Rejected(InputRejection::InvalidUtf8);
            }
        }
        InputOutcome::Changed(self.feedback())
    }

    fn reset_pending_utf8(&mut self) {
        self.pending_utf8.fill(0);
        self.pending_length = 0;
        self.pending_expected = 0;
    }
}

impl Drop for PromptInput {
    fn drop(&mut self) {
        self.clear();
    }
}

fn unsafe_for_terminal(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unicode_as_complete_scalars() {
        let mut input = PromptInput::new(32, false, false).expect("input");
        for byte in "a🔐".bytes() {
            input.feed_byte(byte);
        }
        assert_eq!(input.feedback().character_count(), 2);
        assert_eq!(
            input.feed_byte(127),
            InputOutcome::Changed(input.feedback())
        );
        assert_eq!(input.feedback().character_count(), 1);
        assert_eq!(
            input.with_visible_text(|text| text.map(str::to_owned)),
            None
        );
    }

    #[test]
    fn clear_cancel_and_submit_have_distinct_outcomes() {
        let mut input = PromptInput::new(32, true, false).expect("input");
        input.handle_key(PromptKey::Character('x'));
        assert_eq!(
            input.handle_key(PromptKey::Clear),
            InputOutcome::Changed(input.feedback())
        );
        assert!(input.is_empty());
        input.handle_key(PromptKey::Character('y'));
        assert_eq!(input.handle_key(PromptKey::Enter), InputOutcome::Submit);
        assert_eq!(
            input.with_visible_text(|text| text.map(str::to_owned)),
            Some("y".into())
        );
        assert_eq!(input.handle_key(PromptKey::Cancel), InputOutcome::Cancelled);
        assert!(input.is_empty());
    }

    #[test]
    fn silent_feedback_never_exposes_plaintext() {
        let mut input = PromptInput::new(32, true, true).expect("input");
        input.handle_key(PromptKey::Character('s'));
        assert_eq!(input.feedback().echo_mode(), EchoMode::Silent);
        assert_eq!(input.feedback().character_count(), 0);
        assert_eq!(
            input.with_visible_text(|text| text.map(str::to_owned)),
            None
        );
        let debug = format!("{:?}", input.feedback());
        assert!(!debug.contains('s'));
    }

    #[test]
    fn rejects_invalid_utf8_and_bounded_overflow() {
        let mut input = PromptInput::new(2, false, false).expect("input");
        assert_eq!(input.feed_byte(0xe2), InputOutcome::Pending);
        assert_eq!(
            input.feed_byte(b'x'),
            InputOutcome::Rejected(InputRejection::InvalidUtf8)
        );
        assert_eq!(
            input.feed_byte(b'a'),
            InputOutcome::Changed(input.feedback())
        );
        assert_eq!(
            input.feed_byte(b'b'),
            InputOutcome::Changed(input.feedback())
        );
        assert_eq!(
            input.feed_byte(b'c'),
            InputOutcome::Rejected(InputRejection::MaximumLength)
        );
    }

    #[test]
    fn finish_guard_zeroes_on_success_and_unwind() {
        let mut input = PromptInput::new(32, false, false).expect("input");
        input.handle_key(PromptKey::Character('x'));
        input.finish_with(|secret| {
            assert_eq!(secret.expose(|bytes| bytes.to_vec()), b"x");
        });
        assert!(input.is_empty());
        assert_eq!(input.feedback().character_count(), 0);

        input.handle_key(PromptKey::Character('y'));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            input.finish_with::<()>(|_| panic!("injected transport failure"));
        }));
        assert!(result.is_err());
        assert!(input.is_empty());
        assert_eq!(input.feedback().character_count(), 0);
    }
}
