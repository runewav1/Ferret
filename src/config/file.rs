use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pathutil;

/// Configuration for the automatic refresh interval behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshIntervalConfig {
    /// Whether automatic periodic refresh is enabled.
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// The refresh interval as a human-readable duration string.
    /// Examples: "30s", "5m", "1h", "24h"
    #[serde(default = "default_interval")]
    pub interval: String,

    /// Which fields to refresh automatically.
    /// A list of field names: ["branch", "remote", "languages", ...]
    /// Or empty to mean "all".
    #[serde(default)]
    pub fields: Vec<String>,

    /// Repository names excluded from automatic refresh.
    /// These entries are only refreshed when the user runs `ferret refresh` explicitly.
    #[serde(default)]
    pub excluded: Vec<String>,
}

impl Default for RefreshIntervalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: default_interval(),
            fields: Vec::new(),
            excluded: Vec::new(),
        }
    }
}

impl RefreshIntervalConfig {
    /// Parse the interval string into seconds.
    /// Supports: "30s", "5m", "1h", "24h", or plain number of seconds.
    pub fn interval_seconds(&self) -> crate::error::Result<u64> {
        parse_duration(&self.interval)
    }

    /// Check if a repository is excluded from auto-refresh.
    pub fn is_excluded(&self, name: &str) -> bool {
        self.excluded.iter().any(|e| e.eq_ignore_ascii_case(name))
    }

    /// Check if a specific field should be auto-refreshed.
    /// If `fields` is empty, all fields are included.
    pub fn includes_field(&self, field: &str) -> bool {
        if self.fields.is_empty() {
            return true; // empty means "all"
        }
        self.fields.iter().any(|f| f.eq_ignore_ascii_case(field))
    }
}

fn default_false() -> bool {
    false
}

fn default_interval() -> String {
    "5m".to_string()
}

/// Parse a human-readable duration string into seconds.
pub fn parse_duration(s: &str) -> crate::error::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(crate::error::FerretError::ConfigError(
            "empty duration string".to_string(),
        ));
    }

    // Try plain number (seconds)
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(secs);
    }

    // Try suffixed format: "30s", "5m", "1h", "24h"
    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| crate::error::FerretError::ConfigError(format!("invalid duration: {}", s)))?;

    match suffix {
        "s" | "S" => Ok(num),
        "m" | "M" => Ok(num * 60),
        "h" | "H" => Ok(num * 3600),
        "d" | "D" => Ok(num * 86400),
        _ => Err(crate::error::FerretError::ConfigError(format!(
            "unknown duration suffix '{}' (use s, m, h, d)",
            suffix
        ))),
    }
}

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

    /// Automatic refresh interval configuration
    #[serde(default)]
    pub refresh_interval: RefreshIntervalConfig,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30").unwrap(), 30);
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("5m").unwrap(), 300);
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("24h").unwrap(), 86400);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
    }

    #[test]
    fn test_parse_duration_case_insensitive() {
        assert_eq!(parse_duration("5M").unwrap(), 300);
        assert_eq!(parse_duration("1H").unwrap(), 3600);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn test_refresh_interval_defaults() {
        let cfg = RefreshIntervalConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.interval, "5m");
        assert!(cfg.fields.is_empty());
        assert!(cfg.excluded.is_empty());
    }

    #[test]
    fn test_is_excluded() {
        let cfg = RefreshIntervalConfig {
            excluded: vec!["Lime".to_string(), "temp-repo".to_string()],
            ..Default::default()
        };
        assert!(cfg.is_excluded("lime")); // case insensitive
        assert!(cfg.is_excluded("Lime"));
        assert!(cfg.is_excluded("TEMP-REPO"));
        assert!(!cfg.is_excluded("other"));
    }

    #[test]
    fn test_includes_field() {
        let cfg_all = RefreshIntervalConfig::default();
        assert!(cfg_all.includes_field("branch")); // empty = all

        let cfg_specific = RefreshIntervalConfig {
            fields: vec!["branch".to_string(), "remote".to_string()],
            ..Default::default()
        };
        assert!(cfg_specific.includes_field("branch"));
        assert!(cfg_specific.includes_field("BRANCH")); // case insensitive
        assert!(cfg_specific.includes_field("remote"));
        assert!(!cfg_specific.includes_field("fingerprint"));
    }
}
