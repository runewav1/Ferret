use clap::Args;
use std::process::Command;

use crate::registry::RegistryManager;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";

/// Pull remote changes for one or more registered repositories.
///
/// Shells out to `git pull` in each repository's directory.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Pull remote changes",
    long_about = "Pull remote changes into registered repositories.\n\n\
        Runs `git pull` in each repository's directory.\n\n\
        Examples:\n  \
        ferret pull --all               # Pull all repos\n  \
        ferret pull --repo myapp        # Pull one repo\n  \
        ferret pull --repo app1 app2    # Pull multiple repos\n  \
        ferret pull --all --dry-run     # Preview what would be pulled"
)]
pub struct PullArgs {
    /// Pull all registered local repositories
    #[arg(long)]
    pub all: bool,

    /// Pull specific repositories by name (accepts multiple)
    #[arg(long, value_name = "NAME", num_args = 1..)]
    pub repo: Vec<String>,

    /// Branch to pull (defaults to current branch)
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,

    /// Show what would be pulled without executing
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompts
    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub fn execute(args: &PullArgs) -> crate::error::Result<()> {
    let manager = RegistryManager::new()?;
    let repos = resolve_repos(&manager, args)?;

    if repos.is_empty() {
        eprintln!("  No repositories specified. Use --all or --repo NAME.");
        return Ok(());
    }

    if !args.yes && repos.len() > 3 {
        eprintln!("  About to pull {} repositories.", repos.len());
        if !confirm("  Continue?") {
            eprintln!("  Cancelled.");
            return Ok(());
        }
    }

    let mut pulled = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    println!();

    for entry in &repos {
        let path = match &entry.local_path {
            Some(p) => p,
            None => {
                println!(
                    "  {}{}{}{}  {}skipped (no local path){}",
                    BOLD, CYAN, entry.name, RESET, DIM, RESET
                );
                skipped += 1;
                continue;
            }
        };

        print!("  {}{}{}{}  ", BOLD, CYAN, entry.name, RESET);

        if args.dry_run {
            println!("{}(dry run){}", DIM, RESET);
            pulled += 1;
            continue;
        }

        match run_git_pull(path, args.branch.as_deref()) {
            Ok(output) => {
                if output.contains("Already up to date") || output.contains("Already up-to-date") {
                    println!("{}up to date{}", DIM, RESET);
                    skipped += 1;
                } else {
                    let summary = output
                        .lines()
                        .find(|l| l.contains("file") && l.contains("changed"))
                        .or_else(|| output.lines().find(|l| l.contains("Updating")))
                        .unwrap_or("pulled");
                    println!("{}", summary.trim());
                    pulled += 1;
                }
            }
            Err(e) => {
                println!("{}failed{}: {}", RED, RESET, e);
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "  {}{}{}{} pulled  {}{}{} skipped  {}{}{} failed",
        BOLD,
        GREEN,
        pulled,
        RESET,
        DIM,
        skipped,
        RESET,
        if failed > 0 { RED } else { DIM },
        failed,
        RESET,
    );

    Ok(())
}

fn resolve_repos(
    manager: &RegistryManager,
    args: &PullArgs,
) -> crate::error::Result<Vec<crate::registry::entry::RegistryEntry>> {
    if args.all {
        let all = manager.get_all()?;
        Ok(all.into_iter().filter(|e| e.local_path.is_some()).collect())
    } else if !args.repo.is_empty() {
        let mut repos = Vec::new();
        for name in &args.repo {
            match manager.get(name) {
                Ok(entry) => repos.push(entry),
                Err(e) => eprintln!("  Warning: '{}': {}", name, e),
            }
        }
        Ok(repos)
    } else {
        Ok(Vec::new())
    }
}

fn run_git_pull(path: &std::path::Path, branch: Option<&str>) -> crate::error::Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("pull");

    if let Some(b) = branch {
        cmd.args(["origin", b]);
    }

    let output = cmd
        .current_dir(path)
        .output()
        .map_err(crate::error::FerretError::IoError)?;

    if output.status.success() {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        Ok(combined)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(crate::error::FerretError::GitError(stderr.to_string()))
    }
}

fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    eprint!("{} [y/N] ", prompt);
    io::stderr().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pull_args_all() {
        let args = PullArgs {
            all: true,
            repo: vec![],
            branch: None,
            dry_run: false,
            yes: false,
        };
        assert!(args.all);
        assert!(args.repo.is_empty());
    }

    #[test]
    fn test_pull_args_repos() {
        let args = PullArgs {
            all: false,
            repo: vec!["app1".into(), "app2".into()],
            branch: None,
            dry_run: true,
            yes: true,
        };
        assert_eq!(args.repo.len(), 2);
        assert!(args.dry_run);
    }

    #[test]
    fn test_pull_args_with_branch() {
        let args = PullArgs {
            all: false,
            repo: vec!["myapp".into()],
            branch: Some("main".into()),
            dry_run: false,
            yes: false,
        };
        assert_eq!(args.branch.as_deref(), Some("main"));
    }
}
