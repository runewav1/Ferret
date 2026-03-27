//! Repository identity and fingerprinting.
//!
//! A repository's *identity* is intentionally decoupled from its location on
//! disk. Moving or renaming a repository folder does not change its identity.
//!
//! ## Fingerprint derivation
//!
//! The fingerprint is built in priority order:
//!
//! 1. **Root-commit hash** – the SHA-1 of the very first commit in the
//!    repository (the ancestor of all branches). This is stable across clones,
//!    renames, and moves.
//! 2. **HEAD commit hash** – used when there is exactly one commit (the root
//!    *is* HEAD) or when walking to the root is unusually expensive. Still
//!    stable for single-commit repos.
//! 3. **Synthetic fallback** – for brand-new repos with no commits yet, we
//!    derive a BLAKE3 hash from the absolute path of the `.git` directory and
//!    the wall-clock creation time of the `HEAD` file. This is location-dependent
//!    by necessity but is clearly flagged as synthetic.
//!
//! All three variants are captured in [`RepoFingerprint`] so callers can make
//! informed decisions (e.g. distrust a synthetic fingerprint when matching moved
//! repos).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use git2::{Repository, Sort};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};

// ── Fingerprint ───────────────────────────────────────────────────────────────

/// The stability class of a [`RepoFingerprint`].
///
/// Callers should treat [`FingerprintKind::Synthetic`] fingerprints with lower
/// confidence when attempting to match a moved repository, because they are
/// derived from the original disk location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintKind {
    /// Derived from the root (initial) commit SHA. Maximally stable – survives
    /// clones, mirrors, renames, and moves.
    RootCommit,

    /// Derived from the HEAD commit SHA. Stable as long as no new commits land
    /// on the branch that was HEAD at fingerprint time, but still far more
    /// reliable than a synthetic fingerprint.
    HeadCommit,

    /// Derived from the `.git` path and file timestamps. Only generated for
    /// repositories with no commits. Treat as provisional.
    Synthetic,
}

impl fmt::Display for FingerprintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FingerprintKind::RootCommit => f.write_str("root-commit"),
            FingerprintKind::HeadCommit => f.write_str("head-commit"),
            FingerprintKind::Synthetic  => f.write_str("synthetic"),
        }
    }
}

/// A content-stable, location-independent identifier for a git repository.
///
/// Two [`RepoFingerprint`]s are considered equal when their `hash` strings
/// match **and** both are non-synthetic, or when both are synthetic and were
/// derived from the same original location.
///
/// The `hash` is always a 64-character lowercase hex string (BLAKE3 output).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoFingerprint {
    /// 64-character lowercase hex digest.
    pub hash: String,

    /// How the hash was derived.
    pub kind: FingerprintKind,

    /// The raw git SHA (if available) that was fed into the hash.
    /// For [`FingerprintKind::Synthetic`] this is `None`.
    pub source_sha: Option<String>,
}

impl RepoFingerprint {
    /// Returns `true` if this fingerprint was derived from a commit hash and
    /// can therefore be trusted across repository moves.
    pub fn is_stable(&self) -> bool {
        matches!(
            self.kind,
            FingerprintKind::RootCommit | FingerprintKind::HeadCommit
        )
    }

    /// Returns `true` if this fingerprint was synthetically derived and may
    /// not survive a repository move.
    pub fn is_synthetic(&self) -> bool {
        self.kind == FingerprintKind::Synthetic
    }

    /// Short (16-character) version of the hash, useful for human display.
    pub fn short(&self) -> &str {
        &self.hash[..16]
    }

    /// Construct a fingerprint directly from a 64-char hex string (testing /
    /// deserialization helpers).
    pub fn from_raw(hash: String, kind: FingerprintKind, source_sha: Option<String>) -> Self {
        Self { hash, kind, source_sha }
    }
}

impl fmt::Display for RepoFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, &self.hash[..16])
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────

/// Full identity record for a discovered git repository.
///
/// Combines the stable [`RepoFingerprint`] with contextual metadata gathered
/// at scan / registration time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIdentity {
    /// Stable content-based fingerprint.
    pub fingerprint: RepoFingerprint,

    /// Absolute path to the working tree root (i.e. the directory that
    /// *contains* `.git`, not the `.git` directory itself).
    ///
    /// For bare repositories this is the path to the `.git` directory itself.
    pub workdir: PathBuf,

    /// Absolute path to the `.git` directory (or file, for worktrees).
    pub git_dir: PathBuf,

    /// Human-readable name, defaulting to the folder name of `workdir`.
    pub name: String,

    /// Whether this is a bare repository (no working tree).
    pub is_bare: bool,

    /// Current branch name, or `None` for detached HEAD.
    pub head_branch: Option<String>,

    /// The HEAD commit SHA at the time of fingerprinting (first 40 hex chars).
    pub head_sha: Option<String>,

    /// Commits ahead of the upstream tracking branch (0 if no upstream).
    #[serde(default)]
    pub ahead: u32,

    /// Commits behind the upstream tracking branch (0 if no upstream).
    #[serde(default)]
    pub behind: u32,

    /// The configured upstream / tracking branch name (e.g. `"origin/main"`),
    /// or `None` when there is no tracking branch configured.
    #[serde(default)]
    pub upstream: Option<String>,
}

impl RepoIdentity {
    /// Returns the folder name of the working tree root.
    pub fn folder_name(&self) -> &str {
        self.workdir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
    }
}

// ── Fingerprinter ─────────────────────────────────────────────────────────────

/// Computes [`RepoIdentity`] values for paths on disk.
///
/// Create one instance and reuse it – it allocates no per-call resources beyond
/// what libgit2 needs.
#[derive(Debug, Default, Clone)]
pub struct Fingerprinter {
    /// When `true`, skip the (potentially expensive) root-commit walk and
    /// use the HEAD commit hash directly. Useful when scanning many repos
    /// where perfect stability is less important than speed.
    pub prefer_head: bool,
}

impl Fingerprinter {
    /// Create a new `Fingerprinter` using the default settings (root-commit
    /// walk enabled).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new `Fingerprinter` that always uses HEAD for speed.
    pub fn fast() -> Self {
        Self { prefer_head: true }
    }

    /// Compute a full [`RepoIdentity`] for the repository at `path`.
    ///
    /// `path` may be:
    /// * The working-tree root (contains `.git` directory or file).
    /// * The `.git` directory itself.
    /// * Any subdirectory of the working tree (libgit2 walks up).
    ///
    /// # Errors
    ///
    /// Returns [`TrackerError::NotARepo`] when `path` is not inside a git
    /// repository, or [`TrackerError::FingerprintFailed`] when the fingerprint
    /// cannot be derived.
    pub fn identify(&self, path: &Path) -> Result<RepoIdentity> {
        let repo = Repository::discover(path).map_err(|_| TrackerError::NotARepo(path.to_owned()))?;

        let workdir = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| repo.path().to_path_buf());

        let git_dir = repo.path().to_path_buf();

        let name = workdir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let is_bare = repo.is_bare();

        // Resolve HEAD metadata
        let (head_branch, head_sha) = resolve_head(&repo);

        // Resolve branch tracking info (silently ignore errors).
        let (upstream, ahead, behind) = crate::branch::get_branch_info(&workdir)
            .map(|bi| (bi.upstream, bi.ahead, bi.behind))
            .unwrap_or((None, 0, 0));

        // Compute fingerprint
        let fingerprint = self.fingerprint_repo(&repo, &workdir)?;

        Ok(RepoIdentity {
            fingerprint,
            workdir,
            git_dir,
            name,
            is_bare,
            head_branch,
            head_sha,
            ahead,
            behind,
            upstream,
        })
    }

    /// Compute only the [`RepoFingerprint`] for an already-opened repository.
    pub fn fingerprint_repo(&self, repo: &Repository, workdir: &Path) -> Result<RepoFingerprint> {
        // Strategy 1: HEAD commit is available
        if let Ok(head) = repo.head() {
            if let Ok(commit) = head.peel_to_commit() {
                let head_sha = commit.id().to_string();

                if self.prefer_head {
                    return Ok(blake3_fingerprint(
                        &head_sha,
                        FingerprintKind::HeadCommit,
                        Some(head_sha.clone()),
                    ));
                }

                // Strategy 1a: walk to root commit
                match find_root_commit(repo) {
                    Ok(root_sha) => {
                        return Ok(blake3_fingerprint(
                            &root_sha,
                            FingerprintKind::RootCommit,
                            Some(root_sha.clone()),
                        ));
                    }
                    Err(_) => {
                        // Fallback: use HEAD if root walk fails
                        return Ok(blake3_fingerprint(
                            &head_sha,
                            FingerprintKind::HeadCommit,
                            Some(head_sha.clone()),
                        ));
                    }
                }
            }
        }

        // Strategy 2: no commits yet – synthetic fingerprint
        synthetic_fingerprint(repo, workdir)
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Walk the commit graph to find the root (parentless) commit.
///
/// Uses a `revwalk` with topological + time sorting, reversed, so the first
/// result is the oldest commit.
fn find_root_commit(repo: &Repository) -> Result<String> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
    walk.push_head()?;

    // The first OID in reverse-topological order is the root commit.
    let root_oid = walk
        .next()
        .ok_or_else(|| {
            TrackerError::FingerprintFailed {
                path: repo.path().to_path_buf(),
                reason: "revwalk produced no commits".into(),
            }
        })?
        .map_err(|e| TrackerError::FingerprintFailed {
            path: repo.path().to_path_buf(),
            reason: e.to_string(),
        })?;

    Ok(root_oid.to_string())
}

/// Build a `RepoFingerprint` by feeding a source string through BLAKE3.
fn blake3_fingerprint(source: &str, kind: FingerprintKind, sha: Option<String>) -> RepoFingerprint {
    let hash = blake3::hash(source.as_bytes());
    RepoFingerprint {
        hash:       format!("{}", hash),
        kind,
        source_sha: sha,
    }
}

/// Produce a synthetic fingerprint from the `.git` directory path + the mtime
/// of the `HEAD` file inside it.
fn synthetic_fingerprint(repo: &Repository, _workdir: &Path) -> Result<RepoFingerprint> {
    let git_dir = repo.path();
    let head_file = git_dir.join("HEAD");

    // Gather the mtime of HEAD (nanosecond precision where available).
    let mtime_nanos: u128 = std::fs::metadata(&head_file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // Mix the git_dir path and the mtime.
    let mut hasher = blake3::Hasher::new();
    hasher.update(git_dir.to_string_lossy().as_bytes());
    hasher.update(b"\x00");
    hasher.update(&mtime_nanos.to_le_bytes());

    let hash = format!("{}", hasher.finalize());

    Ok(RepoFingerprint {
        hash,
        kind:       FingerprintKind::Synthetic,
        source_sha: None,
    })
}

/// Resolve the HEAD branch name and commit SHA from an open repository.
///
/// Returns `(branch_name, commit_sha)`. Either value may be `None`:
/// * `branch_name` is `None` for a detached HEAD.
/// * `commit_sha` is `None` for an unborn HEAD (no commits yet).
fn resolve_head(repo: &Repository) -> (Option<String>, Option<String>) {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return (None, None),
    };

    let branch = if head.is_branch() {
        head.shorthand().map(str::to_owned)
    } else {
        None
    };

    let sha = head
        .peel_to_commit()
        .ok()
        .map(|c| c.id().to_string());

    (branch, sha)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    // ── helpers ──

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git not found on PATH");
        assert!(status.success(), "git {:?} failed", args);
    }

    /// Create a temporary git repo with one commit.
    fn make_repo_with_commit() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        git(&path, &["init"]);
        git(&path, &["config", "user.email", "test@test.com"]);
        git(&path, &["config", "user.name", "Test"]);
        // Create a file and commit it
        std::fs::write(path.join("readme.txt"), b"hello").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "initial"]);
        (tmp, path)
    }

    /// Create a temporary git repo with NO commits.
    fn make_empty_repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        git(&path, &["init"]);
        (tmp, path)
    }

    // ── fingerprint kind ──

    #[test]
    fn root_commit_fingerprint_for_repo_with_commits() {
        let (_tmp, path) = make_repo_with_commit();
        let fp = Fingerprinter::new();
        let identity = fp.identify(&path).expect("identify failed");
        assert_eq!(identity.fingerprint.kind, FingerprintKind::RootCommit);
        assert!(identity.fingerprint.is_stable());
        assert!(!identity.fingerprint.is_synthetic());
    }

    #[test]
    fn head_commit_fingerprint_when_prefer_head() {
        let (_tmp, path) = make_repo_with_commit();
        let fp = Fingerprinter::fast();
        let identity = fp.identify(&path).expect("identify failed");
        assert_eq!(identity.fingerprint.kind, FingerprintKind::HeadCommit);
        assert!(identity.fingerprint.is_stable());
    }

    #[test]
    fn synthetic_fingerprint_for_empty_repo() {
        let (_tmp, path) = make_empty_repo();
        let fp = Fingerprinter::new();
        let identity = fp.identify(&path).expect("identify failed");
        assert_eq!(identity.fingerprint.kind, FingerprintKind::Synthetic);
        assert!(identity.fingerprint.is_synthetic());
        assert!(!identity.fingerprint.is_stable());
    }

    // ── stability across moves ──

    #[test]
    fn fingerprint_stable_after_rename() {
        let (_tmp, path) = make_repo_with_commit();
        let fp = Fingerprinter::new();

        let before = fp.identify(&path).expect("identify before").fingerprint;

        // Rename the directory.
        // On Windows, std::fs::rename refuses to move a non-empty directory if
        // the source and destination are on different logical roots; but since
        // both paths share the same TempDir parent they are on the same drive,
        // so we can use robocopy (copy + delete) as a reliable fallback.
        let new_path = path.parent().unwrap().join("renamed_repo");
        #[cfg(windows)]
        {
            // robocopy: copy entire tree, then remove the original.
            let rob = std::process::Command::new("robocopy")
                .args([
                    path.to_str().unwrap(),
                    new_path.to_str().unwrap(),
                    "/E",   // include all subdirectories (even empty)
                    "/NFL", // no file list
                    "/NDL", // no dir list
                    "/NJH", // no job header
                    "/NJS", // no job summary
                ])
                .status()
                .expect("robocopy failed");
            // robocopy exits 0–7 for success/no-error conditions.
            assert!(
                rob.code().unwrap_or(99) < 8,
                "robocopy exited with error code {:?}",
                rob.code()
            );
            // Remove original tree.
            std::process::Command::new("cmd")
                .args(["/C", "rd", "/S", "/Q", path.to_str().unwrap()])
                .status()
                .expect("rd failed");
        }
        #[cfg(not(windows))]
        {
            std::fs::rename(&path, &new_path).expect("rename failed");
        }

        let after = fp
            .identify(&new_path)
            .expect("identify after rename")
            .fingerprint;

        assert_eq!(before.hash, after.hash, "fingerprint must not change after rename");
        assert_eq!(before.kind, after.kind);
    }

    #[test]
    fn fingerprint_stable_for_clone() {
        let (_tmp, path) = make_repo_with_commit();
        let clone_dir = TempDir::new().unwrap();
        let clone_path = clone_dir.path().join("clone");

        git(
            path.parent().unwrap(),
            &["clone", path.to_str().unwrap(), clone_path.to_str().unwrap()],
        );

        let fp = Fingerprinter::new();
        let original = fp.identify(&path).expect("identify original").fingerprint;
        let cloned   = fp.identify(&clone_path).expect("identify clone").fingerprint;

        assert_eq!(
            original.hash, cloned.hash,
            "cloned repo must have the same root-commit fingerprint"
        );
    }

    // ── identity metadata ──

    #[test]
    fn head_branch_is_populated() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        assert!(
            identity.head_branch.is_some(),
            "expected a branch name for a freshly-committed repo"
        );
    }

    #[test]
    fn head_sha_is_populated() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        let sha = identity.head_sha.expect("expected a HEAD sha");
        assert_eq!(sha.len(), 40, "git SHA should be 40 hex chars");
    }

    #[test]
    fn error_on_non_repo_path() {
        let tmp = TempDir::new().unwrap();
        let result = Fingerprinter::new().identify(tmp.path());
        assert!(
            matches!(result, Err(TrackerError::NotARepo(_))),
            "expected NotARepo, got {:?}",
            result
        );
    }

    // ── display / short ──

    #[test]
    fn short_hash_is_16_chars() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        assert_eq!(identity.fingerprint.short().len(), 16);
    }

    #[test]
    fn display_includes_kind_and_short_hash() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        let display = identity.fingerprint.to_string();
        assert!(display.starts_with("root-commit:"), "got: {display}");
        assert_eq!(display.len(), "root-commit:".len() + 16);
    }

    // ── serde roundtrip ──

    #[test]
    fn fingerprint_serde_roundtrip() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        let json = serde_json::to_string(&identity.fingerprint).expect("serialize");
        let back: RepoFingerprint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(identity.fingerprint, back);
    }

    #[test]
    fn identity_serde_roundtrip() {
        let (_tmp, path) = make_repo_with_commit();
        let identity = Fingerprinter::new().identify(&path).unwrap();
        let json = serde_json::to_string(&identity).expect("serialize");
        let back: RepoIdentity = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(identity.fingerprint, back.fingerprint);
        assert_eq!(identity.workdir, back.workdir);
    }
}
