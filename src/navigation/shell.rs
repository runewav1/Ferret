use std::path::Path;
use std::process::Command;

use crate::config::FerretConfig;
use crate::error::FerretError;

/// Open a new shell window at the given path
pub fn spawn_shell(repo_path: &Path, shell: Option<&str>) -> crate::error::Result<()> {
    if !repo_path.exists() {
        return Err(FerretError::PathError(format!(
            "Path does not exist: {}",
            repo_path.display()
        )));
    }

    let config = FerretConfig::load().unwrap_or_default();
    let shell_name = shell.unwrap_or_else(|| config.effective_shell());

    // Strip Windows extended-length path prefix \\?\
    let path_str = repo_path.to_string_lossy();
    let path_str = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);

    #[cfg(target_os = "windows")]
    {
        let terminal_command = if shell_name.contains("pwsh") || shell_name.contains("powershell") {
            "pwsh"
        } else if shell_name.contains("cmd") {
            "cmd"
        } else {
            shell_name
        };

        if terminal_command == "pwsh" || terminal_command == "powershell" {
            Command::new("cmd")
                .args([
                    "/C",
                    "start",
                    terminal_command,
                    "-NoExit",
                    "-Command",
                    &format!("Set-Location '{}'", path_str),
                ])
                .spawn()
                .map_err(|e| {
                    FerretError::IoError(std::io::Error::other(format!(
                        "Failed to spawn shell: {}",
                        e
                    )))
                })?;
        } else if terminal_command == "cmd" {
            Command::new("cmd")
                .args([
                    "/C",
                    "start",
                    "cmd",
                    "/K",
                    &format!("cd /d \"{}\"", path_str),
                ])
                .spawn()
                .map_err(|e| {
                    FerretError::IoError(std::io::Error::other(format!(
                        "Failed to spawn shell: {}",
                        e
                    )))
                })?;
        } else {
            // Generic shell
            Command::new("cmd")
                .args([
                    "/C",
                    "start",
                    terminal_command,
                    "-NoExit",
                    "-Command",
                    &format!("Set-Location '{}'", path_str),
                ])
                .spawn()
                .map_err(|e| {
                    FerretError::IoError(std::io::Error::other(format!(
                        "Failed to spawn shell: {}",
                        e
                    )))
                })?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Use osascript to launch Terminal.app and execute cd command
        let script = format!(
            "tell application \"Terminal\" to do script \"cd {} && clear\"",
            path_str.replace('"', "\\\"")
        );
        Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map_err(|e| {
                FerretError::IoError(std::io::Error::other(format!(
                    "Failed to spawn terminal: {}",
                    e
                )))
            })?;
    }

    #[cfg(target_os = "linux")]
    {
        let terminals = ["gnome-terminal", "konsole", "alacritty", "kitty", "xterm"];
        let mut spawned = false;
        for term in &terminals {
            let result = match *term {
                "gnome-terminal" | "konsole" | "alacritty" => Command::new(term)
                    .args(["--working-directory", path_str.as_ref()])
                    .spawn(),
                "kitty" => Command::new(term)
                    .args(["--directory", path_str.as_ref()])
                    .spawn(),
                "xterm" => {
                    // xterm doesn't support working directory flag, use -e with shell command
                    let cd_cmd = format!("cd '{}' && exec $SHELL", path_str);
                    Command::new(term).args(["-e", "sh", "-c", &cd_cmd]).spawn()
                }
                _ => Command::new(term)
                    .args(["--working-directory", path_str.as_ref()])
                    .spawn(),
            };
            if result.is_ok() {
                spawned = true;
                break;
            }
        }
        if !spawned {
            return Err(FerretError::IoError(std::io::Error::other(
                "No terminal emulator found.",
            )));
        }
    }

    Ok(())
}
