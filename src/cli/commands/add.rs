use clap::Args;

use crate::error::FerretError;
use crate::registry::RegistryManager;

/// Add a repository to the Ferret registry.
///
/// By default, registers the current working directory. Use `--path` to register
/// a different directory, or `--lone-remote` to track a remote repository without
/// cloning it locally.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Add a repository to the registry",
    long_about = "Add a repository to the Ferret registry.\n\n\
        By default, registers the current working directory as a local repository. \
        The command will auto-detect the git remote URL if available.\n\n\
        Examples:\n  \
        ferret add                     # Add current directory\n  \
        ferret add --path ~/projects/myapp\n  \
        ferret add --lone-remote https://github.com/user/repo\n  \
        ferret add -n myname           # Add with custom name"
)]
pub struct AddArgs {
    /// Register the current directory as a repository (default behavior)
    #[arg(long, default_value_t = true)]
    pub here: bool,

    /// Path to a local repository to register instead of the current directory
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Track a remote repository URL without cloning locally.
    /// Useful for bookmarking repositories you don't need locally
    #[arg(long, value_name = "URL")]
    pub lone_remote: Option<String>,

    /// Link a local repository to an existing remote entry by name.
    /// Creates a connection between local and remote tracking
    #[arg(long, value_name = "REF")]
    pub link_to_remote: Option<String>,

    /// Override the auto-detected name for the repository entry.
    /// By default, uses the folder name or remote repository name
    #[arg(short = 'n', long, value_name = "NAME")]
    pub name: Option<String>,
}

pub fn execute(args: &AddArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    if let Some(url) = &args.lone_remote {
        let entry = manager.add_lone_remote(url, args.name.as_deref())?;
        println!("Added lone remote '{}' ({})", entry.name, url);
    } else if let Some(remote_ref) = &args.link_to_remote {
        let local_name = args.name.as_deref().unwrap_or("");

        if local_name.is_empty() {
            let current = std::env::current_dir().map_err(FerretError::IoError)?;
            let folder_name =
                crate::pathutil::folder_name(&current).unwrap_or_else(|| "unknown".to_string());

            manager.link_to_remote(&folder_name, remote_ref)?;
            println!("Linked '{}' to remote '{}'", folder_name, remote_ref);
        } else {
            manager.link_to_remote(local_name, remote_ref)?;
            println!("Linked '{}' to remote '{}'", local_name, remote_ref);
        }
    } else if let Some(path) = &args.path {
        let path = std::path::PathBuf::from(path);
        let entry = manager.add_local(&path, args.name.as_deref())?;
        print_added_entry(&entry, &crate::pathutil::normalize_path(&path));
    } else {
        let current = std::env::current_dir().map_err(FerretError::IoError)?;
        let entry = manager.add_local(&current, args.name.as_deref())?;
        print_added_entry(&entry, &crate::pathutil::normalize_path(&current));
    }

    Ok(())
}

fn print_added_entry(entry: &crate::registry::entry::RegistryEntry, path_display: &str) {
    println!("Added '{}' ({})", entry.name, path_display);

    // Branch
    let branch_label = entry.branch_label();
    if entry.head_detached {
        println!("  Branch:    (detached HEAD)");
    } else if branch_label != "—" {
        let divergence = entry.divergence_hint();
        if divergence.is_empty() {
            println!("  Branch:    {}", branch_label);
        } else {
            println!("  Branch:    {} {}", branch_label, divergence);
        }
        if let Some(upstream) = &entry.upstream_branch {
            println!("  Tracking:  → {}", upstream);
        }
    }

    // Worktree kind (only show when non-standard)
    if let Some(wk) = &entry.worktree_kind {
        if wk.is_linked() {
            println!("  Worktree:  {}", wk);
            if let Some(main_path) = &entry.main_repo_path {
                println!(
                    "  Main repo: {}",
                    crate::pathutil::normalize_path(main_path)
                );
            }
        }
    }

    // Remote
    if let Some(remote) = &entry.remote_url {
        println!("  Remote:    {}", remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_lone_remote_args() {
        let args = AddArgs {
            here: false,
            path: None,
            lone_remote: Some("https://github.com/user/repo.git".to_string()),
            link_to_remote: None,
            name: Some("my-repo".to_string()),
        };

        assert_eq!(
            args.lone_remote.as_deref(),
            Some("https://github.com/user/repo.git")
        );
        assert_eq!(args.name.as_deref(), Some("my-repo"));
    }

    #[test]
    fn test_link_to_remote_args() {
        let args = AddArgs {
            here: false,
            path: None,
            lone_remote: None,
            link_to_remote: Some("my-remote".to_string()),
            name: Some("my-repo".to_string()),
        };

        assert_eq!(args.link_to_remote.as_deref(), Some("my-remote"));
        assert_eq!(args.name.as_deref(), Some("my-repo"));
    }

    #[test]
    fn test_link_to_remote_args_without_name() {
        let args = AddArgs {
            here: false,
            path: None,
            lone_remote: None,
            link_to_remote: Some("origin-remote".to_string()),
            name: None,
        };

        assert_eq!(args.link_to_remote.as_deref(), Some("origin-remote"));
        assert!(args.name.is_none());
    }
}
