//! Linux DRM/KMS display backend.
//!
//! This currently provides only the backend lifecycle scaffold and a bounded
//! card-path presence probe. It does not open DRM, modeset, allocate a buffer,
//! present pixels, restore a mode, or implement the text-VT fallback yet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Dimensions, DisplayBackend, DisplayError, DisplayState, InputEvent, RestoreMode, Scene,
};

const DRM_CARD_PATH: &str = "/dev/dri/card0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmKmsConfig {
    card_path: PathBuf,
}

impl DrmKmsConfig {
    pub fn new() -> Self {
        Self {
            card_path: PathBuf::from(DRM_CARD_PATH),
        }
    }

    pub fn with_card_path(path: impl Into<PathBuf>) -> Self {
        Self {
            card_path: path.into(),
        }
    }
}

impl Default for DrmKmsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// DRM/KMS lifecycle scaffold; not yet a modesetting display owner.
#[derive(Debug)]
pub struct DrmKmsBackend {
    config: DrmKmsConfig,
    state: DisplayState,
    dimensions: Option<Dimensions>,
}

impl DrmKmsBackend {
    pub fn new(config: DrmKmsConfig) -> Self {
        Self {
            config,
            state: DisplayState::Unacquired,
            dimensions: None,
        }
    }

    pub fn card_path(&self) -> &Path {
        &self.config.card_path
    }
}

impl DisplayBackend for DrmKmsBackend {
    fn state(&self) -> DisplayState {
        self.state
    }

    fn dimensions(&self) -> Option<Dimensions> {
        self.dimensions
    }

    fn acquire(&mut self) -> Result<(), DisplayError> {
        if self.state != DisplayState::Unacquired {
            return Err(DisplayError::InvalidState {
                operation: "acquire",
                state: self.state,
            });
        }

        self.state = DisplayState::Acquiring;

        // Bounded DRM device probe: if card0 is not present or inaccessible,
        // fallback diagnostics remain fail-open for text-VT.
        if !self.config.card_path.exists() {
            self.state = DisplayState::FailedOpen;
            return Err(DisplayError::Backend {
                backend: "drm-kms",
                operation: "acquire",
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "DRM card device not found",
                ),
            });
        }

        // Default initial dimensions (80x24 standard VT overlay grid)
        let dims = Dimensions::new(80, 24)?;
        self.dimensions = Some(dims);
        self.state = DisplayState::Hidden;
        Ok(())
    }

    fn show(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Hidden => {
                self.state = DisplayState::Splash;
                Ok(())
            }
            DisplayState::Splash | DisplayState::Details => Ok(()),
            _ => Err(DisplayError::InvalidState {
                operation: "show",
                state: self.state,
            }),
        }
    }

    fn hide(&mut self) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Splash | DisplayState::Details => {
                self.state = DisplayState::Hidden;
                Ok(())
            }
            DisplayState::Hidden => Ok(()),
            _ => Err(DisplayError::InvalidState {
                operation: "hide",
                state: self.state,
            }),
        }
    }

    fn render(&mut self, scene: &Scene) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Splash | DisplayState::Details => {}
            _ => {
                return Err(DisplayError::InvalidState {
                    operation: "render",
                    state: self.state,
                });
            }
        }

        let current_dims = self.dimensions.ok_or(DisplayError::InvalidState {
            operation: "render",
            state: self.state,
        })?;

        if scene.dimensions() != current_dims {
            return Err(DisplayError::SizeMismatch {
                expected: current_dims,
                actual: scene.dimensions(),
            });
        }

        Ok(())
    }

    fn poll_input(&mut self, _timeout: Duration) -> Result<Option<InputEvent>, DisplayError> {
        match self.state {
            DisplayState::Splash | DisplayState::Details => Ok(None),
            _ => Err(DisplayError::InvalidState {
                operation: "poll_input",
                state: self.state,
            }),
        }
    }

    fn details(&mut self, visible: bool) -> Result<(), DisplayError> {
        match self.state {
            DisplayState::Splash | DisplayState::Details => {
                self.state = if visible {
                    DisplayState::Details
                } else {
                    DisplayState::Splash
                };
                Ok(())
            }
            _ => Err(DisplayError::InvalidState {
                operation: "details",
                state: self.state,
            }),
        }
    }

    fn restore(&mut self) -> Result<(), DisplayError> {
        if self.state == DisplayState::Restored {
            return Ok(());
        }
        self.state = DisplayState::Restored;
        Ok(())
    }

    fn restore_with_mode(&mut self, mode: RestoreMode) -> Result<(), DisplayError> {
        let _ = mode;
        self.restore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drm_kms_backend_lifecycle() {
        let config = DrmKmsConfig::new();
        let mut backend = DrmKmsBackend::new(config);
        assert_eq!(backend.state(), DisplayState::Unacquired);
        assert_eq!(backend.dimensions(), None);

        let res = backend.acquire();
        if !Path::new(DRM_CARD_PATH).exists() {
            assert!(res.is_err());
            assert_eq!(backend.state(), DisplayState::FailedOpen);
        }
    }
}
