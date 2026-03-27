use crate::registry::RegistryManager;
use clap::Args;

/// Remove a repository from the Ferret registry.
///
/// This removes the registry entry only — it does not delete any files from disk.
/// Use `--link-to-remote` to unlink a remote association while keeping the local entry.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Remove a repository from the registry",
    long_about = "Remove a repository from the Ferret registry.\n\n\
        This only removes the tracking entry — no files are deleted from disk.\n\n\
        Examples:\n  \
        ferret remove myrepo           # Remove by name\n  \
        ferret remove myrepo --link-to-remote  # Just unlink remote"
)]
pub struct RemoveArgs {
    /// Name or ID of the repository to remove.
    /// Use `ferret list` to see available repositories
    pub target: String,

    /// Only unlink the remote URL association, keeping the local entry.
    /// The repository stays registered but loses its remote tracking
    #[arg(long)]
    pub link_to_remote: bool,
}

pub fn execute(args: &RemoveArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    if args.link_to_remote {
        manager.unlink_remote(&args.target)?;
        println!("Unlinked remote from '{}'", args.target);
    } else {
        manager.remove(&args.target)?;
        println!("Removed '{}' from registry", args.target);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_args() {
        let args = RemoveArgs {
            target: "my-repo".to_string(),
            link_to_remote: false,
        };
        assert_eq!(args.target, "my-repo");
        assert!(!args.link_to_remote);
    }

    #[test]
    fn test_unlink_args() {
        let args = RemoveArgs {
            target: "my-repo".to_string(),
            link_to_remote: true,
        };
        assert!(args.link_to_remote);
    }
}
