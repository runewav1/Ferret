use crate::error::FerretError;
use std::path::Path;
use std::process::Command;

/// Open the system file explorer at the given path
pub fn open_explorer(repo_path: &Path) -> crate::error::Result<()> {
    if !repo_path.exists() {
        return Err(FerretError::PathError(format!(
            "Path does not exist: {}",
            repo_path.display()
        )));
    }

    let path_str = repo_path.to_string_lossy();

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(path_str.as_ref())
            .spawn()
            .map_err(|e| {
                FerretError::IoError(std::io::Error::other(
                    format!("Failed to open explorer: {}", e),
                ))
            })?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path_str.as_ref())
            .spawn()
            .map_err(|e| {
                FerretError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to open finder: {}", e),
                ))
            })?;
    }

    #[cfg(target_os = "linux")]
    {
        let managers = ["xdg-open", "nautilus", "thunar", "dolphin"];
        let mut opened = false;
        for manager in &managers {
            if Command::new(manager).arg(path_str.as_ref()).spawn().is_ok() {
                opened = true;
                break;
            }
        }
        if !opened {
            return Err(FerretError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "No file manager found. Install xdg-open, nautilus, thunar, or dolphin.",
            )));
        }
    }

    Ok(())
}
