use clap::Args;
use std::process::Command;

use crate::registry::RegistryManager;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";

/// Push local commits to remote for one or more registered repositories.
///
/// Shells out to `git push` in each repository's directory. Use `--all` to
/// push all repos, or `--repo` to specify individual ones.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Push local commits to remote",
    long_about = "Push local commits to the remote for registered repositories.\n\n\
        Runs `git push` in each repository's directory.\n\n\
        Examples:\n  \
        ferret push --all                # Push all repos\n  \
        ferret push --repo myapp         # Push one repo\n  \
        ferret push --repo app1 app2     # Push multiple repos\n  \
        ferret push --all --dry-run      # Preview what would be pushed\n  \
        ferret push --repo myapp --branch feat/foo"
)]
pub struct PushArgs {
    /// Push all registered local repositories
    #[arg(long)]
    pub all: bool,

    /// Push specific repositories by name (accepts multiple)
    #[arg(long, value_name = "NAME", num_args = 1..)]
    pub repo: Vec<String>,

    /// Branch to push (defaults to current branch)
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,

    /// Show what would be pushed without executing
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompts
    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub fn execute(args: &PushArgs) -> crate::error::Result<()> {
    let manager = RegistryManager::new()?;
    let repos = resolve_repos(&manager, args)?;

    if repos.is_empty() {
        eprintln!("  No repositories specified. Use --all or --repo NAME.");
        return Ok(());
    }

    if !args.yes && repos.len() > 3 {
        eprintln!("  About to push {} repositories.", repos.len());
        if !confirm("  Continue?") {
            eprintln!("  Cancelled.");
            return Ok(());
        }
    }

    let mut pushed = 0usize;
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
            pushed += 1;
            continue;
        }

        match run_git_push(path, args.branch.as_deref()) {
            Ok(output) => {
                if output.trim().is_empty() || output.contains("Everything up-to-date") {
                    println!("{}up to date{}", DIM, RESET);
                    skipped += 1;
                } else {
                    let summary = output
                        .lines()
                        .find(|l| l.contains("->"))
                        .unwrap_or("pushed");
                    println!("{}", summary.trim());
                    pushed += 1;
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
        "  {}{}{}{} pushed  {}{}{} skipped  {}{}{} failed",
        BOLD,
        GREEN,
        pushed,
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
    args: &PushArgs,
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

fn run_git_push(path: &std::path::Path, branch: Option<&str>) -> crate::error::Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("push");

    if let Some(b) = branch {
        cmd.args(["origin", b]);
    }

    let output = cmd
        .current_dir(path)
        .output()
        .map_err(crate::error::FerretError::IoError)?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(format!("{}{}", stdout, stderr))
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
    fn test_push_args_all() {
        let args = PushArgs {
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
    fn test_push_args_repos() {
        let args = PushArgs {
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
    fn test_push_args_with_branch() {
        let args = PushArgs {
            all: false,
            repo: vec!["myapp".into()],
            branch: Some("feat/foo".into()),
            dry_run: false,
            yes: false,
        };
        assert_eq!(args.branch.as_deref(), Some("feat/foo"));
    }
}
