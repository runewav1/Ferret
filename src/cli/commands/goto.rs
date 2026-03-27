use clap::Args;

use crate::navigation;
use crate::registry::RegistryManager;

/// Navigate to a repository by opening it in an editor, file explorer, shell, or browser.
///
/// The default action outputs a shell command to navigate to the repository.
/// Use `ferret init {shell}` to enable the `fg` function for directory changes.
/// Use flags to open in different applications, or combine flags to open multiple at once.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Navigate to a repository (open editor, explorer, shell, or remote)",
    long_about = "Navigate to a registered repository.\n\n\
        By default, outputs a shell command (cd/Set-Location) to navigate to the repository.\n\
        Use `ferret init {shell}` to install the `fg` function for actual directory changes.\n\n\
        Examples:\n  \
        ferret goto myapp              # Output cd/Set-Location command\n  \
        ferret init pwsh               # Install fg function for PowerShell\n  \
        ferret goto myapp --editor code  # Open in VS Code\n  \
        ferret goto myapp --remote     # Open remote URL in browser\n  \
        ferret goto myapp --explorer   # Open in file manager\n  \
        ferret goto myapp --sep-shell pwsh  # Open new terminal"
)]
pub struct GotoArgs {
    /// Name or ID of the repository to navigate to.
    /// Use `ferret list` to see available repositories
    pub target: String,

    /// Open the repository's remote URL in your default web browser.
    /// Requires the repository to have a remote URL configured
    #[arg(long)]
    pub remote: bool,

    /// Open the repository in a specific editor by name.
    /// Examples: "code" (VS Code), "cursor", "vim", "nvim", "zed"
    #[arg(long, value_name = "NAME")]
    pub editor: Option<String>,

    /// Open the repository folder in the system file explorer.
    /// On Windows: Explorer, macOS: Finder, Linux: default file manager
    #[arg(long)]
    pub explorer: bool,

    /// Spawn a new terminal window at the repository path.
    /// Specify the shell: "pwsh", "powershell", "cmd", "bash", "zsh"
    #[arg(long, value_name = "SHELL")]
    pub sep_shell: Option<String>,
}

pub fn execute(args: &GotoArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;
    let entry = manager.get(&args.target)?;

    // Update access time
    manager.touch_access(&args.target)?;

    if args.remote {
        let remote_url = entry.remote_url.as_ref().ok_or_else(|| {
            crate::error::FerretError::RemoteError(format!("'{}' has no remote URL", entry.name))
        })?;
        navigation::browser::open_remote(remote_url)?;
        println!("Opening remote for '{}' in browser", entry.name);
        return Ok(());
    }

    let local_path = entry.local_path.as_ref().ok_or_else(|| {
        crate::error::FerretError::PathError(format!(
            "'{}' has no local path (lone remote)",
            entry.name
        ))
    })?;

    // Explicit flags only
    if args.editor.is_some() {
        navigation::editor::launch_editor(local_path, args.editor.as_deref())?;
        println!("Opening '{}' in editor", entry.name);
        return Ok(());
    }

    if args.explorer {
        navigation::explorer::open_explorer(local_path)?;
        println!("Opening '{}' in file explorer", entry.name);
        return Ok(());
    }

    if let Some(shell) = &args.sep_shell {
        navigation::shell::spawn_shell(local_path, Some(shell.as_str()))?;
        println!("Opening {} shell at '{}'", shell, entry.name);
        return Ok(());
    }

    // Default: output only the shell cd command (no extra text)
    let path_display = crate::pathutil::normalize_path(local_path);
    #[cfg(target_os = "windows")]
    {
        if std::env::var("PSModulePath").is_ok() {
            println!("Set-Location \"{}\"", path_display);
        } else {
            println!("cd /d \"{}\"", path_display);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        println!("cd \"{}\"", path_display);
    }

    Ok(())
}
