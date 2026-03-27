use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pathutil;

/// Ferret configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FerretConfig {
    /// Default editor to open repositories with (e.g., "code", "cursor", "vim")
    pub default_editor: Option<String>,

    /// Default file explorer command (uses system default if not set)
    pub default_explorer: Option<String>,

    /// Default shell for --sep-shell (e.g., "pwsh", "bash", "zsh")
    pub default_shell: Option<String>,

    /// Terminal application paths keyed by terminal name
    pub terminal_apps: Option<std::collections::HashMap<String, TerminalConfig>>,

    /// Language detection: whether to rescan on every access or use cache
    pub always_rescan_languages: Option<bool>,
}

/// Configuration for a specific terminal application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Path to the terminal binary
    pub binary_path: String,

    /// Arguments to pass the command to the terminal
    pub command_args: Vec<String>,
}

impl FerretConfig {
    /// Load config from the default config directory, or return defaults
    pub fn load() -> crate::error::Result<Self> {
        let config_path = Self::config_file_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(crate::error::FerretError::IoError)?;
            let config: FerretConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to the default config directory
    pub fn save(&self) -> crate::error::Result<()> {
        let config_path = Self::config_file_path()?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::FerretError::IoError)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| {
            crate::error::FerretError::ConfigError(format!("TOML serialize: {}", e))
        })?;
        std::fs::write(&config_path, content).map_err(crate::error::FerretError::IoError)?;
        Ok(())
    }

    /// Get the path to the config file
    pub fn config_file_path() -> crate::error::Result<PathBuf> {
        let config_dir = pathutil::ferret_config_dir().ok_or_else(|| {
            crate::error::FerretError::ConfigError("Cannot determine config directory".to_string())
        })?;
        Ok(config_dir.join("config.toml"))
    }

    /// Get the effective editor name
    pub fn effective_editor(&self) -> &str {
        self.default_editor.as_deref().unwrap_or("code")
    }

    /// Get the effective shell name
    pub fn effective_shell(&self) -> &str {
        self.default_shell.as_deref().unwrap_or("bash")
    }
}
