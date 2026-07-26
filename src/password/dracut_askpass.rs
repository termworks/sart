//! Classic-dracut `ask_for_password` metadata foundation.
//!
//! Upstream classic dracut has separate Plymouth and TTY prompt/try settings,
//! plus command strings for each path. Bootart models the non-secret prompt
//! split only. It deliberately has no command-string field and never evaluates
//! or executes the framework's cryptsetup command. The experimental dracut
//! override invokes the same `bootart` ELF with a dedicated inherited pipe and
//! retains the stock console path as the fallback.

use super::pipe_askpass::{
    PipeAskpassError, PipeAskpassMetadata, SAME_ELF_CLIENT, validate_attempts, validate_prompt,
};
use std::num::NonZeroU16;

/// Non-secret settings for the framework-owned TTY fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutConsoleFallback {
    prompt: String,
    attempts: NonZeroU16,
    echo_off: bool,
}

impl DracutConsoleFallback {
    pub fn new(
        prompt: impl Into<String>,
        attempts: u16,
        echo_off: bool,
    ) -> Result<Self, PipeAskpassError> {
        let prompt = prompt.into();
        validate_prompt(&prompt)?;
        Ok(Self {
            prompt,
            attempts: validate_attempts(attempts)?,
            echo_off,
        })
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn attempts(&self) -> u16 {
        self.attempts.get()
    }

    pub fn echo_off(&self) -> bool {
        self.echo_off
    }
}

/// Safe subset of classic dracut's `ask_for_password` invocation.
///
/// `bootart_prompt` corresponds to dracut's Plymouth prompt/try path, but uses
/// [`PipeAskpassMetadata`] rather than Plymouth's `--command` argument. The
/// console data corresponds to the `--tty-*` settings and stays owned by the
/// adapter so it can fail open without asking bootart to execute anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutAskpassMetadata {
    bootart_prompt: PipeAskpassMetadata,
    console_fallback: DracutConsoleFallback,
}

impl DracutAskpassMetadata {
    pub fn new(
        bootart_prompt: PipeAskpassMetadata,
        console_fallback: DracutConsoleFallback,
    ) -> Self {
        Self {
            bootart_prompt,
            console_fallback,
        }
    }

    pub fn bootart_prompt(&self) -> &PipeAskpassMetadata {
        &self.bootart_prompt
    }

    pub fn console_fallback(&self) -> &DracutConsoleFallback {
        &self.console_fallback
    }

    /// The bridge always invokes the already-installed `bootart` ELF.
    pub const fn client_program(&self) -> &'static str {
        SAME_ELF_CLIENT
    }

    /// Framework command text is deliberately outside this contract.
    pub const fn accepts_command_text(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::PipeSecretFraming;

    fn model() -> DracutAskpassMetadata {
        let bootart = PipeAskpassMetadata::new(
            "Password (/dev/vda2)",
            5,
            1024,
            PipeSecretFraming::NewlineTerminated,
        )
        .expect("bootart metadata");
        let console =
            DracutConsoleFallback::new("Password (/dev/vda2)", 1, true).expect("TTY metadata");
        DracutAskpassMetadata::new(bootart, console)
    }

    #[test]
    fn models_the_classic_prompt_and_try_split_without_commands() {
        let request = model();
        assert_eq!(request.bootart_prompt().attempts(), 5);
        assert_eq!(request.console_fallback().attempts(), 1);
        assert!(request.console_fallback().echo_off());
        assert_eq!(request.client_program(), "bootart");
        assert!(!request.accepts_command_text());
    }

    #[test]
    fn component_contract_cannot_claim_pair_support() {
        let request = model();
        assert_eq!(request.client_program(), SAME_ELF_CLIENT);
        assert!(!request.accepts_command_text());
    }

    #[test]
    fn tty_fallback_metadata_rejects_terminal_control_text() {
        assert!(matches!(
            DracutConsoleFallback::new("Password\nfor root", 1, true),
            Err(PipeAskpassError::UnsafePrompt)
        ));
    }
}
