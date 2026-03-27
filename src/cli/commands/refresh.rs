use clap::Args;

use crate::registry::RegistryManager;

const RESET:   &str = "\x1b[0m";
const BOLD:    &str = "\x1b[1m";
const CYAN:    &str = "\x1b[36m";
const YELLOW:  &str = "\x1b[33m";
const GREEN:   &str = "\x1b[32m";
const MAGENTA: &str = "\x1b[35m";
const DIM:     &str = "\x1b[2m";
const RED:     &str = "\x1b[31m";

/// Refresh cached branch and tracker information for one or all registry entries.
///
/// Ferret caches branch names, upstream divergence, worktree kind, and
/// fingerprint hashes at registration time. If you switch branches, pull new
/// commits, or move a repository, the cached data can go stale.
///
/// `ferret refresh` re-reads that data from disk and updates the registry.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Refresh branch and tracker info for registered repositories",
    long_about = "Refresh cached branch and tracker info in the Ferret registry.\n\n\
        Ferret caches the current branch, upstream divergence, worktree kind,\n\
        and fingerprint at registration time. Use this command to bring the\n\
        registry up to date after switching branches, pulling commits, or\n\
        moving repositories on disk.\n\n\
        Examples:\n  \
        ferret refresh                  # Refresh all local entries\n  \
        ferret refresh myapp            # Refresh one entry by name\n  \
        ferret refresh --all            # Explicitly refresh everything\n  \
        ferret refresh --full           # Also re-compute fingerprints + worktree kind\n  \
        ferret refresh myapp --full     # Full refresh for one entry"
)]
pub struct RefreshArgs {
    /// Name of a specific repository to refresh.
    /// When omitted (or combined with --all), all local entries are refreshed.
    pub name: Option<String>,

    /// Refresh all registered local repositories (default when no name given).
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Also re-compute the fingerprint hash and worktree kind, not just the
    /// branch information. Slower but more thorough.
    #[arg(long)]
    pub full: bool,
}

pub fn execute(args: &RefreshArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    match &args.name {
        // ── Single entry ──────────────────────────────────────────────────────
        Some(name) => {
            // Verify the entry exists first so we can give a nice error.
            let entry = manager.get(name)?;

            if entry.local_path.is_none() {
                println!(
                    "  {}'{}'{}  is a lone-remote entry — nothing to refresh.",
                    DIM, entry.name, RESET
                );
                return Ok(());
            }

            print!(
                "  Refreshing {}{}{}{}…  ",
                BOLD, CYAN, entry.name, RESET
            );

            if args.full {
                manager.refresh_tracker(name)?;
            } else {
                manager.refresh_branch(name)?;
            }

            // Re-load to display the updated values.
            let updated = manager.get(name)?;
            println!("{}done{}", GREEN, RESET);
            print_entry_branch_summary(&updated);
        }

        // ── All entries ───────────────────────────────────────────────────────
        None => {
            let entries = manager.get_all()?;
            let local_count = entries.iter().filter(|e| e.local_path.is_some()).count();

            if local_count == 0 {
                println!("  {}No local entries to refresh.{}", DIM, RESET);
                return Ok(());
            }

            println!(
                "  {}{}Refreshing {} local entr{}…{}",
                BOLD, CYAN,
                local_count,
                if local_count == 1 { "y" } else { "ies" },
                RESET,
            );
            println!();

            let updated_count = if args.full {
                manager.refresh_all_tracker()?
            } else {
                manager.refresh_all_branches()?
            };

            // Display updated branch state for each local entry.
            let updated_entries = manager.get_all()?;
            for entry in updated_entries.iter().filter(|e| e.local_path.is_some()) {
                let path_ok = entry
                    .local_path
                    .as_ref()
                    .map(|p| p.exists())
                    .unwrap_or(false);

                if path_ok {
                    print!(
                        "  {}{}{}{}  ",
                        BOLD, CYAN, entry.name, RESET
                    );
                    print_entry_branch_inline(entry);
                } else {
                    println!(
                        "  {}{}{}{}  {}(path missing){}",
                        BOLD, RED, entry.name, RESET,
                        DIM, RESET,
                    );
                }
            }

            println!();
            println!(
                "  {}{}{}{}  entr{} refreshed.",
                BOLD, MAGENTA,
                updated_count,
                RESET,
                if updated_count == 1 { "y" } else { "ies" },
            );
        }
    }

    Ok(())
}

// ── Display helpers ───────────────────────────────────────────────────────────

/// Print a multi-line branch summary block (used for single-entry refresh).
fn print_entry_branch_summary(entry: &crate::registry::entry::RegistryEntry) {
    let branch_label = entry.branch_label();
    let divergence   = entry.divergence_hint();

    println!();
    if entry.head_detached {
        println!(
            "    {}Branch:{}    {}(detached HEAD){}",
            YELLOW, RESET, DIM, RESET
        );
    } else if divergence.is_empty() {
        println!(
            "    {}Branch:{}    {}{}{}",
            YELLOW, RESET, GREEN, branch_label, RESET
        );
    } else {
        let div_color = if entry.ahead > 0 && entry.behind > 0 {
            YELLOW
        } else if entry.ahead > 0 {
            GREEN
        } else {
            MAGENTA
        };
        println!(
            "    {}Branch:{}    {}{}{} {}{}{}",
            YELLOW, RESET,
            GREEN, branch_label, RESET,
            div_color, divergence, RESET,
        );
    }

    if let Some(upstream) = &entry.upstream_branch {
        println!(
            "    {}Tracking:{}  {}→ {}{}",
            YELLOW, RESET, DIM, upstream, RESET
        );
    }

    if let Some(wk) = &entry.worktree_kind {
        println!(
            "    {}Worktree:{}  {}{}{}",
            YELLOW, RESET, DIM, wk, RESET
        );
    }

    if let Some(fp) = &entry.fingerprint_hash {
        let short = &fp[..16.min(fp.len())];
        println!(
            "    {}ID:{}        {}{}{}",
            YELLOW, RESET, DIM, short, RESET
        );
    }
    println!();
}

/// Print a compact one-line branch summary (used in the --all refresh table).
fn print_entry_branch_inline(entry: &crate::registry::entry::RegistryEntry) {
    let branch_label = entry.branch_label();
    let divergence   = entry.divergence_hint();

    if entry.head_detached {
        println!("{}(detached HEAD){}", DIM, RESET);
        return;
    }

    if divergence.is_empty() {
        println!("{}{}{}", GREEN, branch_label, RESET);
    } else {
        let div_color = if entry.ahead > 0 && entry.behind > 0 {
            YELLOW
        } else if entry.ahead > 0 {
            GREEN
        } else {
            MAGENTA
        };
        println!(
            "{}{}{} {}{}{}",
            GREEN, branch_label, RESET,
            div_color, divergence, RESET,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_args_defaults() {
        let args = RefreshArgs {
            name: None,
            all:  false,
            full: false,
        };
        assert!(args.name.is_none());
        assert!(!args.all);
        assert!(!args.full);
    }

    #[test]
    fn refresh_args_single_name() {
        let args = RefreshArgs {
            name: Some("my-repo".into()),
            all:  false,
            full: false,
        };
        assert_eq!(args.name.as_deref(), Some("my-repo"));
    }

    #[test]
    fn refresh_args_full_flag() {
        let args = RefreshArgs {
            name: None,
            all:  true,
            full: true,
        };
        assert!(args.all);
        assert!(args.full);
    }
}
