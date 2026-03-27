use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Expand a path that may contain ~ or environment variables
pub fn expand_path(path: &str) -> PathBuf {
    let path = path.trim();

    if path.starts_with('~') {
        if let Some(home) = dirs_home() {
            if path == "~" {
                return home;
            } else if path.starts_with("~/") || path.starts_with("~\\") {
                return home.join(&path[2..]);
            }
        }
    }

    PathBuf::from(path)
}

/// Get the user's home directory cross-platform
pub fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Get the Ferret data directory (for registry, config, cache)
pub fn ferret_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "ferret", "ferret").map(|p| p.data_dir().to_path_buf())
}

/// Get the Ferret config directory
pub fn ferret_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "ferret", "ferret").map(|p| p.config_dir().to_path_buf())
}

/// Normalize a path for display: forward slashes, strip Windows extended-length prefix
pub fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // Strip Windows extended-length path prefix: //?/  or //./
    if let Some(stripped) = s.strip_prefix("//?/") {
        stripped.to_string()
    } else if let Some(stripped) = s.strip_prefix("//./") {
        stripped.to_string()
    } else {
        s
    }
}

/// Get the canonical form of a path, resolving symlinks and relative parts
pub fn canonicalize_path(path: &Path) -> crate::error::Result<PathBuf> {
    path.canonicalize().map_err(|e| {
        crate::error::FerretError::PathError(format!(
            "Cannot canonicalize {}: {}",
            path.display(),
            e
        ))
    })
}

/// Check if a path exists and is a directory
pub fn is_valid_directory(path: &Path) -> bool {
    path.is_dir()
}

/// Get the folder name from a path (last component)
pub fn folder_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_path("~/projects");
        assert!(!expanded.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_normalize_path() {
        let path = Path::new("C:\\Users\\test\\project");
        let normalized = normalize_path(path);
        assert!(normalized.contains('/'));
        assert!(!normalized.contains('\\'));
    }

    #[test]
    fn test_normalize_strips_extended_prefix() {
        let path = Path::new(r"\\?\C:\Users\test\project");
        let normalized = normalize_path(path);
        assert!(!normalized.contains("//?/"));
        assert!(!normalized.contains("\\"));
        assert!(normalized.starts_with("C:/"));
    }

    #[test]
    fn test_folder_name() {
        let path = Path::new("/home/user/my-project");
        assert_eq!(folder_name(path), Some("my-project".to_string()));
    }
}
