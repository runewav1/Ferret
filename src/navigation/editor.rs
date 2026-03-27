use crate::config::FerretConfig;
use crate::error::FerretError;
use std::path::Path;
use std::process::Command;

/// Launch an editor at the given repository path.
/// Uses the system shell to resolve editor commands via PATH.
pub fn launch_editor(repo_path: &Path, editor: Option<&str>) -> crate::error::Result<()> {
    let config = FerretConfig::load().unwrap_or_default();
    let editor_name = editor.unwrap_or_else(|| config.effective_editor());

    if !repo_path.exists() {
        return Err(FerretError::PathError(format!(
            "Repository path does not exist: {}",
            repo_path.display()
        )));
    }

    let path_str = repo_path.to_string_lossy();

    #[cfg(target_os = "windows")]
    {
        // Use cmd /C start to launch through shell (resolves PATH, PATHEXT)
        // The empty "" is the window title for start
        Command::new("cmd")
            .args(["/C", "start", "", editor_name, path_str.as_ref()])
            .spawn()
            .map_err(|e| {
                FerretError::IoError(std::io::Error::other(
                    format!("Failed to launch editor '{}': {}", editor_name, e),
                ))
            })?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Try direct spawn first, fall back to shell
        let result = Command::new(editor_name).arg(path_str.as_ref()).spawn();
        if result.is_err() {
            // Fall back to shell
            Command::new("sh")
                .args(["-c", &format!("{} \"{}\"", editor_name, path_str)])
                .spawn()
                .map_err(|e| {
                    FerretError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to launch editor '{}': {}", editor_name, e),
                    ))
                })?;
        }
    }

    Ok(())
}

/// Launch editor and wait for it to close
pub fn launch_editor_wait(repo_path: &Path, editor: Option<&str>) -> crate::error::Result<()> {
    let config = FerretConfig::load().unwrap_or_default();
    let editor_name = editor.unwrap_or_else(|| config.effective_editor());

    if !repo_path.exists() {
        return Err(FerretError::PathError(format!(
            "Repository path does not exist: {}",
            repo_path.display()
        )));
    }

    let path_str = repo_path.to_string_lossy();

    #[cfg(target_os = "windows")]
    {
        let status = Command::new("cmd")
            .args(["/C", editor_name, path_str.as_ref()])
            .status()
            .map_err(|e| {
                FerretError::IoError(std::io::Error::other(
                    format!("Failed to launch editor '{}': {}", editor_name, e),
                ))
            })?;

        if !status.success() {
            return Err(FerretError::IoError(std::io::Error::other(
                format!("Editor '{}' exited with error", editor_name),
            )));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let result = Command::new(editor_name).arg(path_str.as_ref()).status();

        match result {
            Ok(status) if status.success() => {}
            Ok(_) => {
                // Try via shell
                let status = Command::new("sh")
                    .args(["-c", &format!("{} \"{}\"", editor_name, path_str)])
                    .status()
                    .map_err(|e| {
                        FerretError::IoError(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Failed to launch editor '{}': {}", editor_name, e),
                        ))
                    })?;
                if !status.success() {
                    return Err(FerretError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Editor '{}' exited with error", editor_name),
                    )));
                }
            }
            Err(e) => {
                return Err(FerretError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to launch editor '{}': {}", editor_name, e),
                )));
            }
        }
    }

    Ok(())
}
