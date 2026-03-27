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

/// List all registered repositories with optional filtering and sorting.
///
/// Displays a table of repositories with their names, types, paths, remotes,
/// and detected languages. Supports sorting by access time, change time, or
/// commit time, and filtering by programming language.
#[derive(Args, Debug, Clone)]
#[command(
    about = "List registered repositories",
    long_about = "List all repositories in the Ferret registry.\n\n\
        By default, shows all repos sorted by last access time (most recent first).\n\n\
        Examples:\n  \
        ferret list                    # All repos, by access time\n  \
        ferret list --by-commit        # Sort by last commit\n  \
        ferret list --lang rust        # Filter to Rust projects\n  \
        ferret list --lang rust ts --or  # Rust OR TypeScript\n  \
        ferret list --last-commit --stat # Show git info columns"
)]
pub struct ListArgs {
    /// Show all repositories (default behavior)
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Sort by last Ferret access time (when you last used `ferret goto`)
    #[arg(long)]
    pub by_access: bool,

    /// Sort by last filesystem modification time
    #[arg(long)]
    pub by_change: bool,

    /// Sort by last git commit timestamp
    #[arg(long)]
    pub by_commit: bool,

    /// Reverse the sort order to show oldest first.
    /// Useful for finding stale/dusty repositories
    #[arg(long)]
    pub inverse: bool,

    /// Filter repositories by programming language.
    /// Accepts multiple values (e.g., --lang rust typescript)
    #[arg(long, value_name = "LANG", num_args = 1..)]
    pub lang: Option<Vec<String>>,

    /// Match ANY specified language instead of ALL.
    /// Without this flag, repos must contain all specified languages
    #[arg(long)]
    pub or: bool,

    /// Add a column showing the last commit message (truncated)
    #[arg(long)]
    pub last_commit: bool,

    /// Add a column showing working directory diff statistics
    #[arg(long)]
    pub stat: bool,

    /// Add a column showing count of staged files
    #[arg(long)]
    pub stage: bool,
}

fn format_duration_since(time: chrono::DateTime<chrono::Utc>) -> String {
    let duration = chrono::Utc::now().signed_duration_since(time);
    let total_secs = duration.num_seconds();
    if total_secs < 0 {
        return "just now".to_string();
    }
    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    if days > 0 {
        format!("{}d ago", days)
    } else if hours > 0 {
        format!("{}h ago", hours)
    } else if minutes > 0 {
        format!("{}m ago", minutes)
    } else {
        "just now".to_string()
    }
}

pub fn execute(args: &ListArgs) -> crate::error::Result<()> {
    let manager = RegistryManager::new()?;
    let mut entries = manager.get_all()?;

    if entries.is_empty() {
        println!("No repositories registered. Use 'ferret add' to add one.");
        return Ok(());
    }

    // Sort by last accessed by default
    if args.by_access || (!args.by_change && !args.by_commit) {
        entries.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        if args.inverse {
            entries.reverse();
        }
    } else if args.by_change {
        entries.sort_by(|a, b| {
            let a_time = a.last_changed.unwrap_or(a.created_at);
            let b_time = b.last_changed.unwrap_or(b.created_at);
            b_time.cmp(&a_time)
        });
        if args.inverse {
            entries.reverse();
        }
    } else if args.by_commit {
        entries.sort_by(|a, b| {
            let a_time = a.last_commit_time.unwrap_or(a.created_at);
            let b_time = b.last_commit_time.unwrap_or(b.created_at);
            b_time.cmp(&a_time)
        });
        if args.inverse {
            entries.reverse();
        }
    }

    // Filter by language (resolve aliases to canonical names)
    if let Some(langs) = &args.lang {
        let alias_map = crate::language::aliases::LanguageAliasMap::new();
        let canonical_langs: Vec<String> = langs
            .iter()
            .filter_map(|l| alias_map.canonical_name(l).map(|s| s.to_string()))
            .collect();

        if canonical_langs.is_empty() {
            println!("No valid language filters provided.");
            return Ok(());
        }

        if args.or {
            entries.retain(|e| canonical_langs.iter().any(|lang| e.has_language(lang)));
        } else {
            entries.retain(|e| canonical_langs.iter().all(|lang| e.has_language(lang)));
        }
    }

    // Display in git-log style
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }

        let entry_type = match entry.entry_type {
            crate::registry::entry::EntryType::Local => "local",
            crate::registry::entry::EntryType::LoneRemote => "remote",
            crate::registry::entry::EntryType::Linked => "linked",
        };

        println!(
            "  {}{}{}{} ({}{}{}){}",
            BOLD, CYAN, entry.name, RESET, DIM, entry_type, RESET, RESET
        );

        // Path
        if let Some(path) = &entry.local_path {
            println!(
                "    {}Path:{}      {}",
                YELLOW,
                RESET,
                crate::pathutil::normalize_path(path)
            );
        }

        // Remote
        if let Some(url) = &entry.remote_url {
            println!("    {}Remote:{}    {}", YELLOW, RESET, url);
        }

        // Languages
        if !entry.languages.is_empty() {
            println!(
                "    {}Languages:{} {}",
                YELLOW,
                RESET,
                entry.languages.join(", ")
            );
        }

        // Branch (only for local entries that have branch info)
        if entry.local_path.is_some() {
            let branch_label = entry.branch_label();
            let divergence   = entry.divergence_hint();

            if !divergence.is_empty() {
                let (div_color, div_text) = if entry.ahead > 0 && entry.behind > 0 {
                    (YELLOW, divergence.clone())
                } else if entry.ahead > 0 {
                    (GREEN, divergence.clone())
                } else {
                    (MAGENTA, divergence.clone())
                };
                println!(
                    "    {}Branch:{}    {}{}{} {}{}{}",
                    YELLOW, RESET,
                    GREEN, branch_label, RESET,
                    div_color, div_text, RESET,
                );
            } else {
                println!(
                    "    {}Branch:{}    {}{}{}",
                    YELLOW, RESET,
                    GREEN, branch_label, RESET,
                );
            }

            // Upstream hint (dim, only when set)
            if let Some(upstream) = &entry.upstream_branch {
                println!(
                    "    {}Tracking:{}  {}→ {}{}",
                    YELLOW, RESET,
                    DIM, upstream, RESET,
                );
            }

            // Worktree kind badge when it's something other than plain main
            if let Some(wk) = &entry.worktree_kind {
                if wk.is_linked() {
                    println!(
                        "    {}Worktree:{}  {}{}{}",
                        YELLOW, RESET,
                        DIM, wk, RESET,
                    );
                }
            }
        }

        // Time column (based on sort mode)
        if args.by_change {
            if let Some(t) = entry.last_changed {
                println!(
                    "    {}Changed:{}   {}",
                    YELLOW,
                    RESET,
                    format_duration_since(t)
                );
            }
        } else if args.by_commit {
            if let Some(t) = entry.last_commit_time {
                println!(
                    "    {}Committed:{} {}",
                    YELLOW,
                    RESET,
                    format_duration_since(t)
                );
            }
        } else {
            println!(
                "    {}Accessed:{}  {}",
                YELLOW,
                RESET,
                format_duration_since(entry.last_accessed)
            );
        }

        // Last commit
        if args.last_commit {
            if let Some(path) = &entry.local_path {
                if let Ok(Some(commit)) = git::commit::get_last_commit(path) {
                    let msg = if commit.message.len() > 50 {
                        format!("{}...", commit.message.chars().take(50).collect::<String>())
                    } else {
                        commit.message.clone()
                    };
                    println!(
                        "    {}Commit:{}    {}{}{} {}",
                        YELLOW, RESET, GREEN, commit.short_hash, RESET, msg
                    );
                }
            }
        }

        // Diff stat
        if args.stat {
            if let Some(path) = &entry.local_path {
                if let Ok(diff) = git::diff::get_working_diff(path) {
                    if diff.files_changed > 0 {
                        println!(
                            "    {}Diff:{}      {} files, {}+{}{}, {}-{}{}",
                            YELLOW,
                            RESET,
                            diff.files_changed,
                            GREEN,
                            diff.insertions,
                            RESET,
                            MAGENTA,
                            diff.deletions,
                            RESET
                        );
                    } else {
                        println!("    {}Diff:{}      {}clean{}", YELLOW, RESET, DIM, RESET);
                    }
                }
            }
        }

        // Staged
        if args.stage {
            if let Some(path) = &entry.local_path {
                if let Ok(diff) = git::diff::get_staged_diff(path) {
                    if diff.files_changed > 0 {
                        println!(
                            "    {}Staged:{}    {} files",
                            YELLOW, RESET, diff.files_changed
                        );
                    } else {
                        println!("    {}Staged:{}    {}none{}", YELLOW, RESET, DIM, RESET);
                    }
                }
            }
        }
    }

    println!();
    println!(
        "  {}{}{} repositories{}",
        DIM,
        MAGENTA,
        entries.len(),
        RESET
    );

    Ok(())
}
