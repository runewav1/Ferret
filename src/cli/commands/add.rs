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
        Use --all to recursively scan the current directory and add all git repositories at once.\n\n\
        Examples:\n  \
        ferret add                     # Add current directory\n  \
        ferret add --all               # Scan and add all repos under current dir\n  \
        ferret add --all --yes         # Skip confirmation prompts\n  \
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

    /// Recursively scan the current directory for git repositories and add all
    /// of them to the registry. Repos already in the registry are skipped.
    #[arg(long)]
    pub all: bool,

    /// Skip safety confirmation prompts (use with --all)
    #[arg(long)]
    pub yes: bool,

    /// Maximum scan depth for --all (default: 8)
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,
}

pub fn execute(args: &AddArgs) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    if args.all {
        return execute_add_all(&mut manager, args);
    }

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

// ── --all mode helpers ─────────────────────────────────────────────────────────

const DANGEROUS_PATHS: &[&str] = &[
    // Unix roots
    "/",
    "/home",
    "/Users",
    "/root",
    "/opt",
    "/usr",
    "/var",
    "/srv",
    "/mnt",
    "/media",
    "/tmp",
    // Windows roots (normalized to forward slashes)
    "C:/",
    "C:/Users",
    "C:/Windows",
    "C:/Program Files",
    "C:/ProgramData",
    "D:/",
    "E:/",
];

fn is_dangerous_path(path: &std::path::Path) -> bool {
    let path_str = crate::pathutil::normalize_path(path);
    let path_lower = path_str.to_lowercase();

    for dangerous in DANGEROUS_PATHS {
        let d_lower = dangerous.to_lowercase();
        let d_lower_slashed = format!("{}/", d_lower.trim_end_matches('/'));
        if path_lower == d_lower || path_lower == d_lower_slashed {
            return true;
        }
    }

    if path.has_root() && path.parent().is_none() {
        return true;
    }

    false
}

fn estimate_directory_scale(path: &std::path::Path) -> (usize, bool) {
    let mut count = 0usize;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                count += 1;
                if count > 50 {
                    return (count, true);
                }
            }
        }
    }
    (count, count > 20)
}

fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    eprint!("{} [y/N] ", prompt);
    io::stderr().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn execute_add_all(manager: &mut RegistryManager, args: &AddArgs) -> crate::error::Result<()> {
    let current = std::env::current_dir().map_err(FerretError::IoError)?;
    let path_display = crate::pathutil::normalize_path(&current);

    // ── Safety check 1: dangerous root paths ────────────────────────────────
    if !args.yes && is_dangerous_path(&current) {
        eprintln!(
            "\x1b[33mWarning:\x1b[0m  The current directory is a system/root path: {}",
            path_display
        );
        eprintln!("  Scanning here may traverse your entire filesystem.");
        if !confirm("  Continue?") {
            eprintln!("  Cancelled.");
            return Ok(());
        }
    }

    // ── Safety check 2: large directory ─────────────────────────────────────
    if !args.yes {
        let (subdir_count, is_large) = estimate_directory_scale(&current);
        if is_large {
            eprintln!(
                "\x1b[33mWarning:\x1b[0m  Current directory has {} immediate subdirectories.",
                subdir_count
            );
            eprintln!("  This may take a while.");
            if !confirm("  Continue?") {
                eprintln!("  Cancelled.");
                return Ok(());
            }
        }
    }

    // ── Scan ────────────────────────────────────────────────────────────────
    let depth = args.depth.unwrap_or(8);

    let config = git_tracker::scanner::ScanConfig::builder()
        .roots(vec![current.clone()])
        .max_depth(depth)
        .collect_identity(false)
        .fast_fingerprint(false)
        .resolve_worktrees(false)
        .exclude_linked_worktrees(true)
        .build();

    eprintln!(
        "  \x1b[1m\x1b[36mScanning…\x1b[0m  {}  (depth {})",
        path_display, depth
    );

    let records = git_tracker::scanner::Scanner::new(config)
        .scan()
        .map_err(|e| FerretError::GitError(e.to_string()))?;

    if records.is_empty() {
        eprintln!("  \x1b[2mNo git repositories found.\x1b[0m");
        return Ok(());
    }

    // ── Confirm large result set ────────────────────────────────────────────
    if !args.yes && records.len() > 10 {
        eprintln!(
            "  \x1b[33mWarning:\x1b[0m  Found {} repositories.",
            records.len()
        );
        if !confirm("  Add all to registry?") {
            eprintln!("  Cancelled.");
            return Ok(());
        }
    }

    // ── Register ────────────────────────────────────────────────────────────
    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for record in &records {
        match manager.add_local(&record.workdir, None) {
            Ok(entry) => {
                eprintln!(
                    "  \x1b[32m+\x1b[0m {}  {}",
                    entry.name,
                    crate::pathutil::normalize_path(&record.workdir),
                );
                added += 1;
            }
            Err(FerretError::DuplicateEntry(_)) => {
                skipped += 1;
            }
            Err(e) => {
                eprintln!(
                    "  \x1b[31mError:\x1b[0m {}  \x1b[2m{}\x1b[0m",
                    crate::pathutil::normalize_path(&record.workdir),
                    e,
                );
                failed += 1;
            }
        }
    }

    eprintln!();
    eprintln!(
        "  \x1b[1m\x1b[35m{}\x1b[0m  added  \x1b[2m{}\x1b[0m  skipped  \x1b[2m{}\x1b[0m  failed",
        added, skipped, failed,
    );

    Ok(())
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
            all: false,
            yes: false,
            depth: None,
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
            all: false,
            yes: false,
            depth: None,
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
            all: false,
            yes: false,
            depth: None,
        };

        assert_eq!(args.link_to_remote.as_deref(), Some("origin-remote"));
        assert!(args.name.is_none());
    }

    #[test]
    fn test_add_all_args() {
        let args = AddArgs {
            here: true,
            path: None,
            lone_remote: None,
            link_to_remote: None,
            name: None,
            all: true,
            yes: false,
            depth: Some(4),
        };
        assert!(args.all);
        assert!(!args.yes);
        assert_eq!(args.depth, Some(4));
    }

    #[test]
    fn test_dangerous_path_detection() {
        use std::path::Path;
        // Windows paths (these get normalized to forward slashes)
        assert!(is_dangerous_path(Path::new("C:/")));
        assert!(is_dangerous_path(Path::new("C:/Users")));
        // Unix paths
        assert!(is_dangerous_path(Path::new("/")));
        assert!(is_dangerous_path(Path::new("/home")));
        // Safe paths
        assert!(!is_dangerous_path(Path::new("C:/Users/me/projects")));
        assert!(!is_dangerous_path(Path::new("/home/me/repos")));
    }
}
