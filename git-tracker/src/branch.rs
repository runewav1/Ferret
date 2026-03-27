//! Branch awareness for git repositories.
//!
//! This module provides the [`BranchInfo`] struct and the [`get_branch_info`]
//! free function for querying the current branch state of a repository,
//! including upstream tracking information and ahead/behind counts.

use std::path::Path;

use git2::{BranchType, Repository};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};

// ── BranchInfo ────────────────────────────────────────────────────────────────

/// Full branch metadata for a git repository.
///
/// Obtained by calling [`get_branch_info`] with a path inside the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    /// Current branch name (e.g. `"main"`, `"feat/my-feature"`).
    ///
    /// When [`is_detached`] is `true` this contains either the first 8
    /// characters of the HEAD SHA or the string `"detached"` when no commit
    /// exists yet.
    pub name: String,

    /// `true` when HEAD is detached (not pointing at a named branch).
    pub is_detached: bool,

    /// The configured upstream / tracking branch name (e.g. `"origin/main"`),
    /// or `None` when there is no tracking branch configured.
    pub upstream: Option<String>,

    /// Number of commits the local branch is **ahead** of the upstream.
    /// Always `0` when there is no upstream.
    pub ahead: u32,

    /// Number of commits the local branch is **behind** the upstream.
    /// Always `0` when there is no upstream.
    pub behind: u32,

    /// Full 40-character SHA of the HEAD commit, or `None` for an unborn HEAD
    /// (a repository that has no commits yet).
    pub head_sha: Option<String>,
}

impl BranchInfo {
    /// Returns the branch name for display purposes.
    ///
    /// When HEAD is detached this returns the fixed string `"(detached HEAD)"`
    /// instead of the raw SHA fragment stored in `name`.
    pub fn display_name(&self) -> &str {
        if self.is_detached {
            "(detached HEAD)"
        } else {
            &self.name
        }
    }

    /// Returns `true` when the branch is in sync with its upstream (neither
    /// ahead nor behind) **and** an upstream is configured.
    pub fn is_clean_tracking(&self) -> bool {
        self.upstream.is_some() && self.ahead == 0 && self.behind == 0
    }

    /// Returns a human-readable summary of the divergence from upstream.
    ///
    /// | Situation | Output |
    /// |---|---|
    /// | No upstream configured | `"no upstream"` |
    /// | In sync | `"up to date"` |
    /// | Ahead only | `"↑N"` (e.g. `"↑3"`) |
    /// | Behind only | `"↓N"` (e.g. `"↓4"`) |
    /// | Diverged | `"↑A ↓B"` (e.g. `"↑3 ↓1"`) |
    pub fn divergence_summary(&self) -> String {
        if self.upstream.is_none() {
            return "no upstream".to_string();
        }
        match (self.ahead, self.behind) {
            (0, 0) => "up to date".to_string(),
            (a, 0) => format!("↑{a}"),
            (0, b) => format!("↓{b}"),
            (a, b) => format!("↑{a} ↓{b}"),
        }
    }
}

// ── BranchSummary ─────────────────────────────────────────────────────────────

/// Lightweight branch summary suitable for quick display without needing to
/// open a `git2::Repository`.
///
/// Construct via `BranchSummary::from(&branch_info)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummary {
    /// Branch name (or short SHA fragment when detached).
    pub name: String,

    /// Whether HEAD is detached.
    pub is_detached: bool,

    /// Commits ahead of upstream.
    pub ahead: u32,

    /// Commits behind upstream.
    pub behind: u32,
}

impl From<&BranchInfo> for BranchSummary {
    fn from(info: &BranchInfo) -> Self {
        Self {
            name:        info.name.clone(),
            is_detached: info.is_detached,
            ahead:       info.ahead,
            behind:      info.behind,
        }
    }
}

// ── get_branch_info ───────────────────────────────────────────────────────────

/// Query branch information for the repository that contains `repo_path`.
///
/// `repo_path` may be the working-tree root, the `.git` directory, or any
/// subdirectory inside the repository – libgit2 will walk up to find the repo.
///
/// # Errors
///
/// Returns [`TrackerError::NotARepo`] when `repo_path` is not inside a git
/// repository.
pub fn get_branch_info(repo_path: &Path) -> Result<BranchInfo> {
    let repo = Repository::discover(repo_path)
        .map_err(|_| TrackerError::NotARepo(repo_path.to_owned()))?;

    // ── HEAD resolution ───────────────────────────────────────────────────────

    let (name, is_detached, head_sha) = resolve_head_info(&repo);

    // ── Upstream resolution ───────────────────────────────────────────────────

    let (upstream, ahead, behind) = if !is_detached {
        resolve_upstream(&repo, &name)
    } else {
        (None, 0, 0)
    };

    Ok(BranchInfo {
        name,
        is_detached,
        upstream,
        ahead,
        behind,
        head_sha,
    })
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Resolve HEAD to a `(name, is_detached, head_sha)` triple.
///
/// * `name` – branch shorthand when attached; first-8-chars of SHA (or
///   `"detached"`) when detached.
/// * `is_detached` – `true` when HEAD is not on a named branch.
/// * `head_sha` – full 40-char commit SHA, or `None` for unborn HEAD.
fn resolve_head_info(repo: &Repository) -> (String, bool, Option<String>) {
    // Attempt to read HEAD as a reference.
    match repo.head() {
        Ok(head_ref) => {
            // Peel to commit for the SHA (may fail on an empty branch).
            let head_sha = head_ref
                .peel_to_commit()
                .ok()
                .map(|c| c.id().to_string());

            if head_ref.is_branch() {
                // Normal attached HEAD.
                let name = head_ref
                    .shorthand()
                    .unwrap_or("(unknown)")
                    .to_string();
                (name, false, head_sha)
            } else {
                // Detached HEAD – use first 8 chars of the SHA as the name.
                let name = head_sha
                    .as_deref()
                    .map(|s| s[..s.len().min(8)].to_string())
                    .unwrap_or_else(|| "detached".to_string());
                (name, true, head_sha)
            }
        }
        Err(_) => {
            // Unborn HEAD (no commits at all).
            // Try to get the branch name from the symbolic reference.
            let name = repo
                .find_reference("HEAD")
                .ok()
                .and_then(|r| r.symbolic_target().map(str::to_owned))
                .and_then(|sym| {
                    // "refs/heads/main" → "main"
                    sym.strip_prefix("refs/heads/").map(str::to_owned)
                })
                .unwrap_or_else(|| "HEAD".to_string());

            (name, false, None)
        }
    }
}

/// Attempt to resolve upstream tracking information for the branch named
/// `branch_name`.
///
/// Returns `(upstream_shorthand, ahead, behind)`.  All errors are silently
/// swallowed and result in `(None, 0, 0)`.
fn resolve_upstream(repo: &Repository, branch_name: &str) -> (Option<String>, u32, u32) {
    // Look up the local branch object.
    let local_branch = match repo.find_branch(branch_name, BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (None, 0, 0),
    };

    // Try to find the upstream (tracking) branch.
    let upstream_branch = match local_branch.upstream() {
        Ok(b) => b,
        Err(_) => return (None, 0, 0),
    };

    // Get the upstream shorthand name (e.g. "origin/main").
    let upstream_name = upstream_branch
        .name()
        .ok()
        .flatten()
        .map(str::to_owned);

    // We need the OIDs of both tips to compute ahead/behind.
    let local_oid = match local_branch.get().peel_to_commit() {
        Ok(c) => c.id(),
        Err(_) => return (upstream_name, 0, 0),
    };

    let upstream_oid = match upstream_branch.get().peel_to_commit() {
        Ok(c) => c.id(),
        Err(_) => return (upstream_name, 0, 0),
    };

    let (ahead, behind) = repo
        .graph_ahead_behind(local_oid, upstream_oid)
        .map(|(a, b)| (a as u32, b as u32))
        .unwrap_or((0, 0));

    (upstream_name, ahead, behind)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    // ── Git helpers ──

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git not found on PATH");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git not found on PATH");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Create a repo with one commit on `main`.
    fn make_repo_with_commit(tmp: &TempDir) -> std::path::PathBuf {
        let path = tmp.path().to_path_buf();
        git(&path, &["init", "-b", "main"]);
        git(&path, &["config", "user.email", "test@test.com"]);
        git(&path, &["config", "user.name", "Test"]);
        std::fs::write(path.join("readme.txt"), b"hello").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "initial"]);
        path
    }

    /// Create an empty repo (no commits, unborn HEAD).
    fn make_empty_repo(tmp: &TempDir) -> std::path::PathBuf {
        let path = tmp.path().to_path_buf();
        git(&path, &["init", "-b", "main"]);
        git(&path, &["config", "user.email", "test@test.com"]);
        git(&path, &["config", "user.name", "Test"]);
        path
    }

    // ── Normal branch ──

    #[test]
    fn normal_branch_has_correct_name() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert_eq!(info.name, "main");
        assert!(!info.is_detached);
        assert_eq!(info.display_name(), "main");
    }

    #[test]
    fn normal_branch_has_head_sha() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        let sha = info.head_sha.expect("expected a HEAD SHA");
        assert_eq!(sha.len(), 40, "HEAD SHA should be 40 hex chars");
    }

    #[test]
    fn normal_branch_no_upstream_by_default() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert!(info.upstream.is_none());
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn no_upstream_divergence_summary_says_no_upstream() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert_eq!(info.divergence_summary(), "no upstream");
    }

    // ── Detached HEAD ──

    #[test]
    fn detached_head_is_detected() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        // Detach HEAD to the current commit.
        let sha = git_output(&path, &["rev-parse", "HEAD"]);
        git(&path, &["checkout", "--detach", &sha]);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert!(info.is_detached, "expected detached HEAD");
        assert_eq!(info.display_name(), "(detached HEAD)");
        // The name should be the first 8 chars of the SHA.
        assert_eq!(info.name, &sha[..8]);
    }

    #[test]
    fn detached_head_has_sha() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let expected_sha = git_output(&path, &["rev-parse", "HEAD"]);
        git(&path, &["checkout", "--detach", &expected_sha]);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert_eq!(info.head_sha.as_deref(), Some(expected_sha.as_str()));
    }

    #[test]
    fn detached_head_has_no_upstream() {
        let tmp = TempDir::new().unwrap();
        let path = make_repo_with_commit(&tmp);

        let sha = git_output(&path, &["rev-parse", "HEAD"]);
        git(&path, &["checkout", "--detach", &sha]);

        let info = get_branch_info(&path).expect("get_branch_info failed");
        assert!(info.upstream.is_none());
        assert_eq!(info.divergence_summary(), "no upstream");
    }

    // ── Unborn HEAD ──

    #[test]
    fn unborn_head_has_no_sha() {
        let tmp = TempDir::new().unwrap();
        let path = make_empty_repo(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info on empty repo failed");
        assert!(info.head_sha.is_none(), "unborn HEAD should have no SHA");
    }

    #[test]
    fn unborn_head_is_not_detached() {
        let tmp = TempDir::new().unwrap();
        let path = make_empty_repo(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info on empty repo failed");
        assert!(!info.is_detached);
    }

    #[test]
    fn unborn_head_branch_name_is_main() {
        let tmp = TempDir::new().unwrap();
        let path = make_empty_repo(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info on empty repo failed");
        assert_eq!(info.name, "main");
    }

    #[test]
    fn unborn_head_has_no_upstream() {
        let tmp = TempDir::new().unwrap();
        let path = make_empty_repo(&tmp);

        let info = get_branch_info(&path).expect("get_branch_info on empty repo failed");
        assert!(info.upstream.is_none());
        assert_eq!(info.divergence_summary(), "no upstream");
    }

    // ── Upstream / ahead-behind ──

    /// Creates an origin repo, clones it, then returns `(origin_tmp, clone_tmp, clone_path)`.
    fn make_origin_and_clone() -> (TempDir, TempDir, std::path::PathBuf) {
        let origin_tmp = TempDir::new().unwrap();
        let origin_path = origin_tmp.path().to_path_buf();
        git(&origin_path, &["init", "-b", "main"]);
        git(&origin_path, &["config", "user.email", "test@test.com"]);
        git(&origin_path, &["config", "user.name", "Test"]);
        std::fs::write(origin_path.join("readme.txt"), b"hello").unwrap();
        git(&origin_path, &["add", "."]);
        git(&origin_path, &["commit", "-m", "initial"]);

        let clone_tmp = TempDir::new().unwrap();
        let clone_path = clone_tmp.path().join("clone");
        git(
            origin_tmp.path(),
            &[
                "clone",
                origin_path.to_str().unwrap(),
                clone_path.to_str().unwrap(),
            ],
        );

        (origin_tmp, clone_tmp, clone_path)
    }

    #[test]
    fn clone_has_upstream_set() {
        let (_origin, _clone_tmp, clone_path) = make_origin_and_clone();

        let info = get_branch_info(&clone_path).expect("get_branch_info on clone");
        assert!(
            info.upstream.is_some(),
            "cloned repo should have upstream set"
        );
        // Upstream should look like "origin/main".
        let upstream = info.upstream.unwrap();
        assert!(
            upstream.contains("main"),
            "expected upstream to reference main, got: {upstream}"
        );
    }

    #[test]
    fn clone_in_sync_is_clean_tracking() {
        let (_origin, _clone_tmp, clone_path) = make_origin_and_clone();

        let info = get_branch_info(&clone_path).expect("get_branch_info on fresh clone");
        assert!(info.is_clean_tracking(), "fresh clone should be in sync");
        assert_eq!(info.divergence_summary(), "up to date");
    }

    #[test]
    fn clone_ahead_after_local_commit() {
        let (_origin, _clone_tmp, clone_path) = make_origin_and_clone();

        // Add a local commit without pushing.
        git(&clone_path, &["config", "user.email", "test@test.com"]);
        git(&clone_path, &["config", "user.name", "Test"]);
        std::fs::write(clone_path.join("extra.txt"), b"new").unwrap();
        git(&clone_path, &["add", "."]);
        git(&clone_path, &["commit", "-m", "local only"]);

        let info = get_branch_info(&clone_path).expect("get_branch_info after local commit");
        assert_eq!(info.ahead, 1, "should be 1 ahead");
        assert_eq!(info.behind, 0, "should not be behind");
        assert_eq!(info.divergence_summary(), "↑1");
        assert!(!info.is_clean_tracking());
    }

    #[test]
    fn clone_behind_after_origin_commit() {
        let (_origin_tmp, _clone_tmp, clone_path) = make_origin_and_clone();

        // Add a commit to origin directly (simulated by committing into the
        // origin via a second clone, then fetching from the first clone).
        let origin_path = {
            // The "origin" remote URL is the origin_tmp path – get it from git.
            let url = git_output(&clone_path, &["remote", "get-url", "origin"]);
            std::path::PathBuf::from(url)
        };

        // Commit something to the origin.
        git(&origin_path, &["config", "user.email", "test@test.com"]);
        git(&origin_path, &["config", "user.name", "Test"]);
        std::fs::write(origin_path.join("upstream.txt"), b"from upstream").unwrap();
        git(&origin_path, &["add", "."]);
        git(&origin_path, &["commit", "-m", "upstream commit"]);

        // Fetch without merging so local is behind.
        git(&clone_path, &["fetch", "origin"]);

        let info = get_branch_info(&clone_path).expect("get_branch_info after fetch");
        assert_eq!(info.behind, 1, "should be 1 behind");
        assert_eq!(info.ahead, 0, "should not be ahead");
        assert_eq!(info.divergence_summary(), "↓1");
        assert!(!info.is_clean_tracking());
    }

    #[test]
    fn diverged_branch_shows_both_directions() {
        let (_origin_tmp, _clone_tmp, clone_path) = make_origin_and_clone();

        let origin_path = {
            let url = git_output(&clone_path, &["remote", "get-url", "origin"]);
            std::path::PathBuf::from(url)
        };

        // Commit to origin.
        git(&origin_path, &["config", "user.email", "test@test.com"]);
        git(&origin_path, &["config", "user.name", "Test"]);
        std::fs::write(origin_path.join("upstream.txt"), b"upstream").unwrap();
        git(&origin_path, &["add", "."]);
        git(&origin_path, &["commit", "-m", "upstream"]);

        // Commit locally.
        git(&clone_path, &["config", "user.email", "test@test.com"]);
        git(&clone_path, &["config", "user.name", "Test"]);
        std::fs::write(clone_path.join("local.txt"), b"local").unwrap();
        git(&clone_path, &["add", "."]);
        git(&clone_path, &["commit", "-m", "local"]);

        // Fetch without merging.
        git(&clone_path, &["fetch", "origin"]);

        let info = get_branch_info(&clone_path).expect("get_branch_info diverged");
        assert_eq!(info.ahead, 1);
        assert_eq!(info.behind, 1);
        assert_eq!(info.divergence_summary(), "↑1 ↓1");
    }

    // ── display_name and divergence_summary correctness ──

    #[test]
    fn display_name_returns_name_when_not_detached() {
        let info = BranchInfo {
            name:        "feature/foo".to_string(),
            is_detached: false,
            upstream:    None,
            ahead:       0,
            behind:      0,
            head_sha:    None,
        };
        assert_eq!(info.display_name(), "feature/foo");
    }

    #[test]
    fn display_name_returns_detached_head_when_detached() {
        let info = BranchInfo {
            name:        "abc12345".to_string(),
            is_detached: true,
            upstream:    None,
            ahead:       0,
            behind:      0,
            head_sha:    None,
        };
        assert_eq!(info.display_name(), "(detached HEAD)");
    }

    #[test]
    fn divergence_summary_various_cases() {
        let base = BranchInfo {
            name:        "main".into(),
            is_detached: false,
            upstream:    Some("origin/main".into()),
            ahead:       0,
            behind:      0,
            head_sha:    None,
        };

        assert_eq!(base.divergence_summary(), "up to date");

        let ahead_only = BranchInfo { ahead: 3, ..base.clone() };
        assert_eq!(ahead_only.divergence_summary(), "↑3");

        let behind_only = BranchInfo { behind: 4, ..base.clone() };
        assert_eq!(behind_only.divergence_summary(), "↓4");

        let diverged = BranchInfo { ahead: 3, behind: 1, ..base.clone() };
        assert_eq!(diverged.divergence_summary(), "↑3 ↓1");

        let no_upstream = BranchInfo { upstream: None, ..base.clone() };
        assert_eq!(no_upstream.divergence_summary(), "no upstream");
    }

    #[test]
    fn is_clean_tracking_requires_upstream_and_zero_counts() {
        let synced = BranchInfo {
            name:        "main".into(),
            is_detached: false,
            upstream:    Some("origin/main".into()),
            ahead:       0,
            behind:      0,
            head_sha:    None,
        };
        assert!(synced.is_clean_tracking());

        assert!(!BranchInfo { ahead: 1, ..synced.clone() }.is_clean_tracking());
        assert!(!BranchInfo { behind: 1, ..synced.clone() }.is_clean_tracking());
        assert!(!BranchInfo { upstream: None, ..synced.clone() }.is_clean_tracking());
    }

    // ── Serde round-trips ──

    #[test]
    fn branch_info_serde_roundtrip() {
        let info = BranchInfo {
            name:        "feat/serde".into(),
            is_detached: false,
            upstream:    Some("origin/feat/serde".into()),
            ahead:       2,
            behind:      1,
            head_sha:    Some("a".repeat(40)),
        };

        let json = serde_json::to_string(&info).expect("serialize BranchInfo");
        let back: BranchInfo = serde_json::from_str(&json).expect("deserialize BranchInfo");

        assert_eq!(info.name, back.name);
        assert_eq!(info.is_detached, back.is_detached);
        assert_eq!(info.upstream, back.upstream);
        assert_eq!(info.ahead, back.ahead);
        assert_eq!(info.behind, back.behind);
        assert_eq!(info.head_sha, back.head_sha);
    }

    #[test]
    fn branch_summary_serde_roundtrip() {
        let summary = BranchSummary {
            name:        "main".into(),
            is_detached: false,
            ahead:       5,
            behind:      3,
        };

        let json = serde_json::to_string(&summary).expect("serialize BranchSummary");
        let back: BranchSummary = serde_json::from_str(&json).expect("deserialize BranchSummary");

        assert_eq!(summary.name, back.name);
        assert_eq!(summary.is_detached, back.is_detached);
        assert_eq!(summary.ahead, back.ahead);
        assert_eq!(summary.behind, back.behind);
    }

    #[test]
    fn branch_summary_from_branch_info() {
        let info = BranchInfo {
            name:        "develop".into(),
            is_detached: false,
            upstream:    Some("origin/develop".into()),
            ahead:       7,
            behind:      2,
            head_sha:    Some("b".repeat(40)),
        };

        let summary = BranchSummary::from(&info);
        assert_eq!(summary.name, "develop");
        assert_eq!(summary.is_detached, false);
        assert_eq!(summary.ahead, 7);
        assert_eq!(summary.behind, 2);
    }

    // ── Not a repo ──

    #[test]
    fn not_a_repo_returns_tracker_error() {
        let tmp = TempDir::new().unwrap();
        // tmp has no .git directory, so it is not a repo.
        let result = get_branch_info(tmp.path());
        assert!(
            matches!(result, Err(TrackerError::NotARepo(_))),
            "expected NotARepo, got {:?}",
            result
        );
    }
}
