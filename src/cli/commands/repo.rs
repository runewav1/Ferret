use chrono::Utc;
use clap::Args;

use crate::git;
use crate::registry::RegistryManager;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const MAGENTA: &str = "\x1b[35m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";

/// Show detailed information about a specific repository.
///
/// Query various aspects of a registered repository including access times,
/// commit history, diff statistics, and staged changes. Without flags,
/// shows a quick overview with path and remote info.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Show info about a specific repository",
    long_about = "Display detailed information about a registered repository.\n\n\
        Without flags, shows a quick overview. Use flags to query specific info.\n\n\
        Examples:\n  \
        ferret repo myapp              # Quick overview\n  \
        ferret repo myapp --sum        # Full summary with git status\n  \
        ferret repo myapp --when last_commit  # When was last commit\n  \
        ferret repo myapp --what last_commit  # Details of last commit\n  \
        ferret repo myapp --stat       # Show diff statistics"
)]
pub struct RepoArgs {
    /// Name or ID of the repository to inspect.
    /// Use `ferret list` to see available repositories
    pub name: String,

    /// Show when something happened. Values: "last_access", "last_commit".
    /// Displays the timestamp and relative time (e.g., "3 days ago")
    #[arg(long, value_name = "TYPE")]
    pub when: Option<String>,

    /// Show what happened. Values: "last_commit".
    /// Displays commit hash, author, message, and files changed
    #[arg(long, value_name = "TYPE")]
    pub what: Option<String>,

    /// Show a comprehensive summary including type, path, remote,
    /// timestamps, languages, last commit, and working tree status
    #[arg(long)]
    pub sum: bool,

    /// Show the diff summary of uncommitted working directory changes
    #[arg(long)]
    pub diff: bool,

    /// Show detailed diff statistics (insertions/deletions per file)
    #[arg(long)]
    pub stat: bool,

    /// Show statistics for staged (index) changes
    #[arg(long)]
    pub stage: bool,
}

pub fn execute(args: &RepoArgs) -> crate::error::Result<()> {
    let manager = RegistryManager::new()?;
    let entry = manager.get(&args.name)?;

    if let Some(when_type) = &args.when {
        match when_type.as_str() {
            "last_access" => {
                let age = Utc::now().signed_duration_since(entry.last_accessed);
                println!(
                    "  {}{}{}{} — {}last_access{}",
                    BOLD, CYAN, entry.name, RESET, DIM, RESET
                );
                println!(
                    "    {}When:{}      {}",
                    YELLOW,
                    RESET,
                    entry.last_accessed.format("%Y-%m-%d %H:%M:%S UTC")
                );
                println!("    {}Ago:{}       {}", YELLOW, RESET, format_duration(age));
            }
            "last_commit" => {
                if let Some(path) = &entry.local_path {
                    match git::commit::get_last_commit(path) {
                        Ok(Some(commit)) => {
                            let age = Utc::now().signed_duration_since(commit.timestamp);
                            println!(
                                "  {}{}{}{} — {}last_commit{}",
                                BOLD, CYAN, entry.name, RESET, DIM, RESET
                            );
                            println!(
                                "    {}When:{}      {}",
                                YELLOW,
                                RESET,
                                commit.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                            );
                            println!("    {}Ago:{}       {}", YELLOW, RESET, format_duration(age));
                        }
                        Ok(None) => {
                            println!(
                                "  {}{}{}{} — {}no commits{}",
                                BOLD, CYAN, entry.name, RESET, DIM, RESET
                            );
                        }
                        Err(e) => {
                            eprintln!("{}Error:{} {}", RED, RESET, e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    println!(
                        "  {}{}{}{} — {}lone remote{}",
                        BOLD, CYAN, entry.name, RESET, DIM, RESET
                    );
                    println!(
                        "    {}Info:{}      no local commit info available",
                        YELLOW, RESET
                    );
                }
            }
            _ => {
                eprintln!(
                    "{}Error:{} Unknown 'when' type '{}'. Use 'last_access' or 'last_commit'.",
                    RED, RESET, when_type
                );
                std::process::exit(1);
            }
        }
    } else if let Some(what_type) = &args.what {
        match what_type.as_str() {
            "last_commit" => {
                if let Some(path) = &entry.local_path {
                    match git::commit::get_last_commit(path) {
                        Ok(Some(commit)) => {
                            println!(
                                "  {}{}{}{} — {}last_commit{}",
                                BOLD, CYAN, entry.name, RESET, DIM, RESET
                            );
                            println!(
                                "    {}Hash:{}      {}{}{}",
                                YELLOW, RESET, GREEN, commit.short_hash, RESET
                            );
                            println!(
                                "    {}Author:{}    {} <{}>",
                                YELLOW, RESET, commit.author, commit.author_email
                            );
                            println!(
                                "    {}Date:{}      {}",
                                YELLOW,
                                RESET,
                                commit.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                            );
                            println!("    {}Message:{}   {}", YELLOW, RESET, commit.message);
                            println!(
                                "    {}Files:{}     {} changed",
                                YELLOW, RESET, commit.files_changed
                            );
                        }
                        Ok(None) => {
                            println!(
                                "  {}{}{}{} — {}no commits{}",
                                BOLD, CYAN, entry.name, RESET, DIM, RESET
                            );
                        }
                        Err(e) => {
                            eprintln!("{}Error:{} {}", RED, RESET, e);
                        }
                    }
                } else {
                    println!(
                        "  {}{}{}{} — {}no local path{}",
                        BOLD, CYAN, entry.name, RESET, DIM, RESET
                    );
                }
            }
            _ => {
                eprintln!(
                    "{}Error:{} Unknown 'what' type '{}'. Use 'last_commit'.",
                    RED, RESET, what_type
                );
                std::process::exit(1);
            }
        }
    } else if args.sum {
        let entry_type = match entry.entry_type {
            crate::registry::entry::EntryType::Local => "local",
            crate::registry::entry::EntryType::LoneRemote => "remote",
            crate::registry::entry::EntryType::Linked => "linked",
        };
        println!(
            "  {}{}{}{} ({}{}{}){}",
            BOLD, CYAN, entry.name, RESET, DIM, entry_type, RESET, RESET
        );
        println!("    {}Type:{}      {}", YELLOW, RESET, entry_type);
        if let Some(path) = &entry.local_path {
            println!(
                "    {}Path:{}      {}",
                YELLOW,
                RESET,
                crate::pathutil::normalize_path(path)
            );
        }
        if let Some(remote) = &entry.remote_url {
            println!("    {}Remote:{}    {}", YELLOW, RESET, remote);
        }
        println!(
            "    {}Created:{}   {}",
            YELLOW,
            RESET,
            entry.created_at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!(
            "    {}Accessed:{}  {}",
            YELLOW,
            RESET,
            entry.last_accessed.format("%Y-%m-%d %H:%M:%S UTC")
        );
        if !entry.languages.is_empty() {
            println!(
                "    {}Languages:{} {}",
                YELLOW,
                RESET,
                entry.languages.join(", ")
            );
        }

        if let Some(path) = &entry.local_path {
            // Branch
            let branch_label = entry.branch_label();
            let divergence   = entry.divergence_hint();
            if divergence.is_empty() {
                println!(
                    "    {}Branch:{}    {}{}{}",
                    YELLOW, RESET, GREEN, branch_label, RESET,
                );
            } else {
                println!(
                    "    {}Branch:{}    {}{}{} {}{}{}",
                    YELLOW, RESET,
                    GREEN, branch_label, RESET,
                    YELLOW, divergence, RESET,
                );
            }
            if let Some(upstream) = &entry.upstream_branch {
                println!(
                    "    {}Tracking:{}  {}→ {}{}",
                    YELLOW, RESET, DIM, upstream, RESET,
                );
            }

            // Worktree kind
            if let Some(wk) = &entry.worktree_kind {
                println!(
                    "    {}Worktree:{}  {}{}{}",
                    YELLOW, RESET, DIM, wk, RESET,
                );
                if let Some(main_path) = &entry.main_repo_path {
                    println!(
                        "    {}Main repo:{} {}{}{}",
                        YELLOW, RESET, DIM,
                        crate::pathutil::normalize_path(main_path),
                        RESET,
                    );
                }
            }

            // Fingerprint (short)
            if let Some(fp) = &entry.fingerprint_hash {
                println!(
                    "    {}ID:{}        {}{}{}",
                    YELLOW, RESET, DIM, &fp[..16.min(fp.len())], RESET,
                );
            }

            if let Ok(Some(commit)) = git::commit::get_last_commit(path) {
                println!(
                    "    {}Commit:{}    {}{}{} {}",
                    YELLOW, RESET, GREEN, commit.short_hash, RESET, commit.message
                );
            }
            if let Ok(status) = git::status::get_repo_status(path) {
                if status.is_clean {
                    println!("    {}Status:{}    {}clean{}", YELLOW, RESET, DIM, RESET);
                } else {
                    println!(
                        "    {}Status:{}    {} modified, {} staged, {} untracked",
                        YELLOW, RESET, status.modified, status.staged, status.untracked
                    );
                }
            }
        }
    } else if args.stat {
        if let Some(path) = &entry.local_path {
            match git::diff::get_working_diff(path) {
                Ok(diff_stats) => {
                    println!(
                        "  {}{}{}{} — {}diff stat{}",
                        BOLD, CYAN, entry.name, RESET, DIM, RESET
                    );
                    println!("{}", git::diff::format_diff_stat_detailed(&diff_stats));
                }
                Err(e) => eprintln!("{}Error:{} {}", RED, RESET, e),
            }
        } else {
            println!(
                "  {}{}{}{} — {}no local path{}",
                BOLD, CYAN, entry.name, RESET, DIM, RESET
            );
            println!("    {}Info:{}      cannot show diff stat", YELLOW, RESET);
        }
    } else if args.stage {
        if let Some(path) = &entry.local_path {
            match git::diff::get_staged_diff(path) {
                Ok(diff_stats) => {
                    println!(
                        "  {}{}{}{} — {}staged{}",
                        BOLD, CYAN, entry.name, RESET, DIM, RESET
                    );
                    println!("{}", git::diff::format_diff_stat_detailed(&diff_stats));
                }
                Err(e) => eprintln!("{}Error:{} {}", RED, RESET, e),
            }
        } else {
            println!(
                "  {}{}{}{} — {}no local path{}",
                BOLD, CYAN, entry.name, RESET, DIM, RESET
            );
            println!(
                "    {}Info:{}      cannot show staged changes",
                YELLOW, RESET
            );
        }
    } else if args.diff {
        if let Some(path) = &entry.local_path {
            match git::diff::get_working_diff(path) {
                Ok(diff_stats) => {
                    println!(
                        "  {}{}{}{} — {}diff{}",
                        BOLD, CYAN, entry.name, RESET, DIM, RESET
                    );
                    if diff_stats.files_changed > 0 {
                        println!(
                            "    {}Changes:{}   {} files, {}{}+{}, {}{}-{}",
                            YELLOW,
                            RESET,
                            diff_stats.files_changed,
                            GREEN,
                            diff_stats.insertions,
                            RESET,
                            MAGENTA,
                            diff_stats.deletions,
                            RESET
                        );
                    } else {
                        println!("    {}Changes:{}   {}clean{}", YELLOW, RESET, DIM, RESET);
                    }
                }
                Err(e) => eprintln!("{}Error:{} {}", RED, RESET, e),
            }
        } else {
            println!(
                "  {}{}{}{} — {}no local path{}",
                BOLD, CYAN, entry.name, RESET, DIM, RESET
            );
            println!("    {}Info:{}      cannot show diff", YELLOW, RESET);
        }
    } else {
        // Default: show quick summary
        let entry_type = match entry.entry_type {
            crate::registry::entry::EntryType::Local => "local",
            crate::registry::entry::EntryType::LoneRemote => "remote",
            crate::registry::entry::EntryType::Linked => "linked",
        };
        println!(
            "  {}{}{}{} ({}{}{}){}",
            BOLD, CYAN, entry.name, RESET, DIM, entry_type, RESET, RESET
        );
        if let Some(path) = &entry.local_path {
            println!(
                "    {}Path:{}      {}",
                YELLOW,
                RESET,
                crate::pathutil::normalize_path(path)
            );

            // Branch + divergence
            let branch_label = entry.branch_label();
            let divergence   = entry.divergence_hint();
            if divergence.is_empty() {
                println!(
                    "    {}Branch:{}    {}{}{}",
                    YELLOW, RESET, GREEN, branch_label, RESET,
                );
            } else {
                println!(
                    "    {}Branch:{}    {}{}{} {}{}{}",
                    YELLOW, RESET,
                    GREEN, branch_label, RESET,
                    YELLOW, divergence, RESET,
                );
            }

            // Worktree kind (only show when non-standard)
            if let Some(wk) = &entry.worktree_kind {
                if wk.is_linked() {
                    println!(
                        "    {}Worktree:{}  {}{}{}",
                        YELLOW, RESET, DIM, wk, RESET,
                    );
                }
            }
        }
        if let Some(remote) = &entry.remote_url {
            println!("    {}Remote:{}    {}", YELLOW, RESET, remote);
        }
        println!();
        println!(
            "    {}{}Use --sum for full summary, --diff for changes, --stat for details{}",
            DIM, MAGENTA, RESET
        );
    }

    Ok(())
}

fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    if total_secs < 0 {
        return "in the future".to_string();
    }

    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    if days > 0 {
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{} days ago", days)
        }
    } else if hours > 0 {
        if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{} hours ago", hours)
        }
    } else if minutes > 0 {
        if minutes == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{} minutes ago", minutes)
        }
    } else {
        "just now".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_what_args() {
        let args = RepoArgs {
            name: "my-repo".to_string(),
            when: None,
            what: Some("last_commit".to_string()),
            sum: false,
            diff: false,
            stat: false,
            stage: false,
        };
        assert_eq!(args.what.as_deref(), Some("last_commit"));
    }

    #[test]
    fn test_repo_sum_args() {
        let args = RepoArgs {
            name: "my-repo".to_string(),
            when: None,
            what: None,
            sum: true,
            diff: false,
            stat: false,
            stage: false,
        };
        assert!(args.sum);
    }
}
