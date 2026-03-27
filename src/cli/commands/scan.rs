use clap::Args;
use std::path::PathBuf;

use crate::registry::RegistryManager;

const RESET:   &str = "\x1b[0m";
const BOLD:    &str = "\x1b[1m";
const CYAN:    &str = "\x1b[36m";
const YELLOW:  &str = "\x1b[33m";
const GREEN:   &str = "\x1b[32m";
const MAGENTA: &str = "\x1b[35m";
const DIM:     &str = "\x1b[2m";
const RED:     &str = "\x1b[31m";

/// Scan one or more directories for git repositories.
///
/// Performs a parallel, high-speed directory walk using git-tracker's Scanner.
/// Results include branch, worktree kind, and fingerprint information for every
/// discovered repository. Use `--add` to register found repositories directly
/// into the Ferret registry (skipping duplicates silently).
#[derive(Args, Debug, Clone)]
#[command(
    about = "Scan directories for git repositories",
    long_about = "Scan one or more directories for git repositories at high speed.\n\n\
        Uses a parallel directory walker. By default, scans the current directory.\n\
        Discovered repositories are shown with branch, worktree kind, and path info.\n\n\
        Examples:\n  \
        ferret scan                          # Scan current directory\n  \
        ferret scan --root ~/projects        # Scan a specific root\n  \
        ferret scan --root ~/projects --depth 4\n  \
        ferret scan --root ~/projects --add  # Add all found repos to registry\n  \
        ferret scan --worktrees              # Include linked worktrees in output\n  \
        ferret scan --root /srv --root ~/dev # Scan multiple roots"
)]
pub struct ScanArgs {
    /// Root directories to scan. May be specified multiple times.
    /// Defaults to the current working directory when omitted.
    #[arg(long, value_name = "PATH", num_args = 1..)]
    pub root: Vec<String>,

    /// Maximum directory depth to recurse into (default: 8).
    #[arg(long, default_value_t = 8, value_name = "N")]
    pub depth: usize,

    /// Include linked worktrees in the output.
    /// By default only main / bare repository roots are shown.
    #[arg(long)]
    pub worktrees: bool,

    /// Add all discovered repositories to the Ferret registry.
    /// Repositories already in the registry are silently skipped.
    #[arg(long)]
    pub add: bool,

    /// Show the content-stable fingerprint hash for each discovered repository.
    #[arg(long)]
    pub fingerprint: bool,

    /// Limit output to this many results.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Show scan performance statistics (time, directories visited, etc.).
    #[arg(long)]
    pub stats: bool,
}

pub fn execute(args: &ScanArgs) -> crate::error::Result<()> {
    // ── Resolve scan roots ────────────────────────────────────────────────────
    let roots: Vec<PathBuf> = if args.root.is_empty() {
        let cwd = std::env::current_dir().map_err(crate::error::FerretError::IoError)?;
        vec![cwd]
    } else {
        args.root.iter().map(PathBuf::from).collect()
    };

    // ── Build scanner config ──────────────────────────────────────────────────
    let mut config_builder = git_tracker::scanner::ScanConfig::builder()
        .roots(roots.clone())
        .max_depth(args.depth)
        // Always collect identity: branch + fingerprint are the key value-adds.
        .collect_identity(true)
        .fast_fingerprint(true)
        // Worktree kind resolution is lightweight relative to the identity pass
        // we're already doing, so always enable it.
        .resolve_worktrees(true)
        .exclude_linked_worktrees(!args.worktrees);

    if let Some(n) = args.limit {
        config_builder = config_builder.limit(n);
    }

    let config = config_builder.build();

    // ── Run the scan ──────────────────────────────────────────────────────────
    println!(
        "  {}{}Scanning…{}",
        BOLD, CYAN, RESET
    );

    let result = git_tracker::scanner::Scanner::new(config)
        .scan_with_stats()
        .map_err(|e| crate::error::FerretError::GitError(e.to_string()))?;

    let records   = &result.records;
    let scan_stats = &result.stats;

    if records.is_empty() {
        println!("  {}No git repositories found.{}", DIM, RESET);
        println!();
        if args.stats {
            print_scan_stats(scan_stats);
        }
        return Ok(());
    }

    // ── Display results ───────────────────────────────────────────────────────
    for (i, record) in records.iter().enumerate() {
        if i > 0 {
            println!();
        }

        // ── Name + worktree kind badge ────────────────────────────────────────
        let kind_badge = match &record.worktree_kind {
            Some(git_tracker::WorktreeKind::Main)         => "main",
            Some(git_tracker::WorktreeKind::Linked { .. }) => "linked-worktree",
            Some(git_tracker::WorktreeKind::Bare)          => "bare",
            None => "repo",
        };

        println!(
            "  {}{}{}{} {}({}){} ",
            BOLD, CYAN, record.name, RESET,
            DIM, kind_badge, RESET,
        );

        // ── Path ──────────────────────────────────────────────────────────────
        println!(
            "    {}Path:{}      {}",
            YELLOW, RESET,
            crate::pathutil::normalize_path(&record.workdir),
        );

        // ── Branch ───────────────────────────────────────────────────────────
        if let Some(branch) = &record.current_branch {
            let divergence = match (record.ahead, record.behind) {
                (0, 0) => String::new(),
                (a, 0) => format!(" {}↑{}{}", GREEN, a, RESET),
                (0, b) => format!(" {}↓{}{}", MAGENTA, b, RESET),
                (a, b) => format!(" {}↑{} ↓{}{}", YELLOW, a, b, RESET),
            };
            let upstream_hint = record
                .upstream_branch
                .as_deref()
                .map(|u| format!(" {}→ {}{}", DIM, u, RESET))
                .unwrap_or_default();
            println!(
                "    {}Branch:{}    {}{}{}{}",
                YELLOW, RESET,
                GREEN, branch, RESET,
                format!("{}{}", divergence, upstream_hint),
            );
        } else if record.is_bare {
            println!(
                "    {}Branch:{}    {}(bare — no working branch){}",
                YELLOW, RESET, DIM, RESET,
            );
        } else {
            println!(
                "    {}Branch:{}    {}(detached HEAD){}",
                YELLOW, RESET, DIM, RESET,
            );
        }

        // ── Linked-worktree extra info ─────────────────────────────────────
        if let Some(git_tracker::WorktreeKind::Linked { name }) = &record.worktree_kind {
            println!(
                "    {}Worktree:{}  linked as \"{}\"",
                YELLOW, RESET, name,
            );
        }

        // ── Fingerprint (opt-in) ──────────────────────────────────────────────
        if args.fingerprint {
            if let Some(fp) = &record.fingerprint {
                println!(
                    "    {}FP:{}        {}{}{}",
                    YELLOW, RESET, DIM, fp.short(), RESET,
                );
            }
        }

        // ── Depth ─────────────────────────────────────────────────────────────
        println!(
            "    {}Depth:{}     {}at depth {}{}",
            YELLOW, RESET, DIM, record.depth, RESET,
        );
    }

    println!();
    println!(
        "  {}{}{}  {} repositor{} found{}",
        BOLD, MAGENTA,
        records.len(),
        RESET,
        if records.len() == 1 { "y" } else { "ies" },
        RESET,
    );

    // ── Stats (opt-in) ────────────────────────────────────────────────────────
    if args.stats {
        println!();
        print_scan_stats(scan_stats);
    }

    // ── Add to registry (opt-in) ──────────────────────────────────────────────
    if args.add {
        println!();
        add_to_registry(records)?;
    }

    Ok(())
}

// ── Registry-add helper ───────────────────────────────────────────────────────

fn add_to_registry(records: &[git_tracker::RepoRecord]) -> crate::error::Result<()> {
    let mut manager = RegistryManager::new()?;

    let mut added   = 0usize;
    let mut skipped = 0usize;
    let mut failed  = 0usize;

    for record in records {
        // Skip linked worktrees when adding — only add canonical repo roots.
        if record.is_linked_worktree {
            skipped += 1;
            continue;
        }

        match manager.add_local(&record.workdir, None) {
            Ok(entry) => {
                println!(
                    "  {}+{} Added   {}{}{}",
                    GREEN, RESET, BOLD, entry.name, RESET,
                );
                added += 1;
            }
            Err(crate::error::FerretError::DuplicateEntry(_)) => {
                // Already registered — not an error.
                skipped += 1;
            }
            Err(e) => {
                eprintln!(
                    "  {}✗{} Failed  {} — {}{}{}",
                    RED, RESET,
                    record.name,
                    DIM, e, RESET,
                );
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "  Registry: {}{}+{}{}  added  {}{}{}{}  skipped  {}{}{}{} failed",
        BOLD, GREEN,  added,   RESET,
        BOLD, DIM,    skipped, RESET,
        if failed > 0 { RED } else { DIM },
        BOLD, failed, RESET,
    );

    Ok(())
}

// ── Stats printer ─────────────────────────────────────────────────────────────

fn print_scan_stats(stats: &git_tracker::scanner::ScanStats) {
    let elapsed_ms = stats.elapsed.as_millis();

    println!("  {}{}Scan statistics{}", BOLD, YELLOW, RESET);
    println!(
        "    {}Elapsed:{}     {}{:.1} ms{}",
        YELLOW, RESET, DIM, elapsed_ms, RESET,
    );
    println!(
        "    {}Dirs visited:{} {}{}{}",
        YELLOW, RESET, DIM, stats.dirs_visited, RESET,
    );
    println!(
        "    {}Repos found:{}  {}{}{}",
        YELLOW, RESET, DIM, stats.repos_found, RESET,
    );
    if stats.errors_skipped > 0 {
        println!(
            "    {}Errors skipped:{} {}{}{}",
            YELLOW, RESET, DIM, stats.errors_skipped, RESET,
        );
    }
}
