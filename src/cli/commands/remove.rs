use crate::registry::RegistryManager;
use clap::Args;

/// Remove a repository from the Ferret registry.
///
/// This removes the registry entry only — it does not delete any files from disk.
/// Use `--link-to-remote` to unlink a remote association while keeping the local entry.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Remove a repository from the registry",
    long_about = "Remove one or more repositories from the Ferret registry.\n\n\
        This only removes the tracking entry — no files are deleted from disk.\n\n\
        Examples:\n  \
        ferret remove myrepo                    # Remove one\n  \
        ferret remove repo1 repo2 repo3         # Remove multiple\n  \
        ferret remove myrepo --link-to-remote   # Just unlink remote"
)]
pub struct RemoveArgs {
    /// Name(s) of the repository to remove.
    /// Accepts multiple names: `ferret remove repo1 repo2 repo3`
    #[arg(required = true, value_name = "NAME")]
    pub targets: Vec<String>,

    /// Only unlink the remote URL association, keeping the local entry.
    /// The repository stays registered but loses its remote tracking
    #[arg(long)]
    pub link_to_remote: bool,
}

pub fn execute(args: &RemoveArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    let mut removed = 0usize;
    let mut unlinked = 0usize;
    let mut failed = 0usize;

    for target in &args.targets {
        if args.link_to_remote {
            match manager.unlink_remote(target) {
                Ok(()) => {
                    println!("  Unlinked remote from '{}'", target);
                    unlinked += 1;
                }
                Err(e) => {
                    eprintln!("  \x1b[31mError:\x1b[0m '{}': {}", target, e);
                    failed += 1;
                }
            }
        } else {
            match manager.remove(target) {
                Ok(()) => {
                    println!("  Removed '{}'", target);
                    removed += 1;
                }
                Err(e) => {
                    eprintln!("  \x1b[31mError:\x1b[0m '{}': {}", target, e);
                    failed += 1;
                }
            }
        }
    }

    if args.targets.len() > 1 {
        if args.link_to_remote {
            println!();
            println!("  {} unlinked, {} failed", unlinked, failed);
        } else {
            println!();
            println!("  {} removed, {} failed", removed, failed);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_single_target() {
        let args = RemoveArgs {
            targets: vec!["my-repo".to_string()],
            link_to_remote: false,
        };
        assert_eq!(args.targets.len(), 1);
        assert_eq!(args.targets[0], "my-repo");
        assert!(!args.link_to_remote);
    }

    #[test]
    fn test_remove_multiple_targets() {
        let args = RemoveArgs {
            targets: vec![
                "repo-a".to_string(),
                "repo-b".to_string(),
                "repo-c".to_string(),
            ],
            link_to_remote: false,
        };
        assert_eq!(args.targets.len(), 3);
    }

    #[test]
    fn test_unlink_multiple() {
        let args = RemoveArgs {
            targets: vec!["my-repo".to_string()],
            link_to_remote: true,
        };
        assert!(args.link_to_remote);
    }
}
