//! High-speed, parallel git repository scanner.
//!
//! [`Scanner`] walks one or more root directories simultaneously using a
//! work-stealing thread pool (rayon) and identifies every git repository it
//! finds. It is deliberately designed to be *fast*:
//!
//! * Directory traversal happens in parallel across all CPU cores.
//! * The hot path (deciding whether a directory is a git repo) is a single
//!   `metadata()` call on the `.git` entry – **no libgit2 open** needed.
//! * libgit2 is only invoked when richer metadata (fingerprint, branch, …) is
//!   explicitly requested via [`ScanConfig::collect_identity`].
//! * Directories whose names match an exclusion pattern are skipped entirely,
//!   pruning large subtrees (e.g. `node_modules`, `target`) in one step.
//!
//! ## Example
//!
//! ```no_run
//! use git_tracker::scanner::{Scanner, ScanConfig};
//! use std::path::PathBuf;
//!
//! let config = ScanConfig::builder()
//!     .roots(vec![PathBuf::from("/home/user/projects")])
//!     .max_depth(6)
//!     .skip_hidden(true)
//!     .build();
//!
//! let records = Scanner::new(config)
//!     .scan()
//!     .expect("scan failed");
//!
//! for rec in &records {
//!     println!("{} — {}", rec.name, rec.workdir.display());
//! }
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};
use crate::identity::{Fingerprinter, RepoFingerprint};
use crate::worktree::{WorktreeKind, WorktreeResolver};

// ── RepoRecord ────────────────────────────────────────────────────────────────

/// Everything the scanner knows about one discovered git repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRecord {
    /// Human-readable name (working-tree folder name).
    pub name: String,

    /// Absolute path to the working-tree root.
    pub workdir: PathBuf,

    /// Absolute path to the `.git` directory (or file for linked worktrees).
    pub git_dir: PathBuf,

    /// Worktree classification, when resolved.
    ///
    /// `None` when [`ScanConfig::resolve_worktrees`] is `false` (default).
    /// For most use-cases this is fine; call [`WorktreeResolver`] on demand
    /// for any record that needs the full picture.
    pub worktree_kind: Option<WorktreeKind>,

    /// Content-stable fingerprint, when computed.
    ///
    /// `None` when [`ScanConfig::collect_identity`] is `false` (default).
    pub fingerprint: Option<RepoFingerprint>,

    /// Whether the `.git` entry is a *file* (linked worktree) rather than a
    /// directory (main repo or bare).
    pub is_linked_worktree: bool,

    /// Whether this is a bare repository (`.git` is the repository root itself).
    pub is_bare: bool,

    /// Current HEAD branch name, or `None` for detached HEAD / unborn.
    ///
    /// Only populated when [`ScanConfig::collect_identity`] is `true`.
    pub head_branch: Option<String>,

    /// Current branch name from branch-awareness query.
    ///
    /// Only populated when [`ScanConfig::collect_identity`] is `true`.
    /// `None` for bare repositories.
    #[serde(default)]
    pub current_branch: Option<String>,

    /// Upstream tracking branch name (e.g. `"origin/main"`).
    ///
    /// Only populated when [`ScanConfig::collect_identity`] is `true`.
    #[serde(default)]
    pub upstream_branch: Option<String>,

    /// Commits ahead of the upstream tracking branch.
    ///
    /// `0` when there is no upstream or when `collect_identity` is `false`.
    #[serde(default)]
    pub ahead: u32,

    /// Commits behind the upstream tracking branch.
    ///
    /// `0` when there is no upstream or when `collect_identity` is `false`.
    #[serde(default)]
    pub behind: u32,

    /// Depth at which this repository was found relative to its scan root.
    pub depth: usize,

    /// Which of the configured scan roots this repo was found under.
    pub scan_root: PathBuf,
}

impl RepoRecord {
    /// Returns `true` when this record is for a linked worktree.
    ///
    /// Uses the cheap `is_linked_worktree` flag, which is always populated,
    /// regardless of whether full worktree resolution was requested.
    pub fn is_worktree(&self) -> bool {
        self.is_linked_worktree
    }
}

// ── ScanConfig ────────────────────────────────────────────────────────────────

/// Configuration for a [`Scanner`] run.
///
/// Build with [`ScanConfig::builder()`] or construct directly.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Directories to scan.  At least one is required.
    pub roots: Vec<PathBuf>,

    /// Maximum recursion depth.  `0` means only inspect the roots themselves.
    /// Default: `8`.
    pub max_depth: usize,

    /// Whether to skip directories whose names start with `.` (e.g.
    /// `.cargo`, `.local`).  The `.git` directory inside a discovered repo
    /// is always skipped regardless of this setting.  Default: `true`.
    pub skip_hidden: bool,

    /// Directory *names* (not full paths) to skip entirely.  Matching is
    /// case-sensitive.  The scanner never descends into these directories.
    ///
    /// Pre-populated defaults (when using [`ScanConfigBuilder`]):
    /// `target`, `node_modules`, `vendor`, `dist`, `build`, `.cache`.
    pub excluded_dirs: Vec<String>,

    /// When `true`, compute a full [`RepoFingerprint`] (via libgit2) for every
    /// discovered repository.  This is the most expensive option.
    /// Default: `false`.
    pub collect_identity: bool,

    /// When `true`, resolve [`WorktreeKind`] for every discovered repository
    /// using [`WorktreeResolver`].  Costs one libgit2 open per repo.
    /// Default: `false`.
    pub resolve_worktrees: bool,

    /// When `false`, linked worktrees (`.git` file entries) are included in
    /// results.  When `true`, they are filtered out – useful when you want
    /// only canonical repository roots.  Default: `false`.
    pub exclude_linked_worktrees: bool,

    /// Maximum number of repositories to return.  `None` = unlimited.
    pub limit: Option<usize>,

    /// Number of rayon threads to use.  `None` = rayon's default (num CPUs).
    pub threads: Option<usize>,

    /// Use the fast (HEAD-commit-based) fingerprint strategy instead of the
    /// root-commit walk.  Only relevant when [`collect_identity`] is `true`.
    /// Default: `true`.
    ///
    /// [`collect_identity`]: ScanConfig::collect_identity
    pub fast_fingerprint: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            roots:                    Vec::new(),
            max_depth:                8,
            skip_hidden:              true,
            excluded_dirs:            default_excluded_dirs(),
            collect_identity:         false,
            resolve_worktrees:        false,
            exclude_linked_worktrees: false,
            limit:                    None,
            threads:                  None,
            fast_fingerprint:         true,
        }
    }
}

impl ScanConfig {
    /// Create a [`ScanConfigBuilder`] for a more ergonomic construction.
    pub fn builder() -> ScanConfigBuilder {
        ScanConfigBuilder::default()
    }
}

fn default_excluded_dirs() -> Vec<String> {
    vec![
        "target".into(),
        "node_modules".into(),
        "vendor".into(),
        "dist".into(),
        "build".into(),
        ".cache".into(),
        "__pycache__".into(),
        ".gradle".into(),
        "Pods".into(),
    ]
}

// ── ScanConfigBuilder ─────────────────────────────────────────────────────────

/// Builder for [`ScanConfig`].
#[derive(Debug, Default)]
pub struct ScanConfigBuilder {
    inner: ScanConfig,
}

impl ScanConfigBuilder {
    /// Set the root directories to scan (required).
    pub fn roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.inner.roots = roots;
        self
    }

    /// Append a single root directory.
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.inner.roots.push(root.into());
        self
    }

    /// Set the maximum recursion depth (default: 8).
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.inner.max_depth = depth;
        self
    }

    /// Whether to skip hidden directories (default: `true`).
    pub fn skip_hidden(mut self, skip: bool) -> Self {
        self.inner.skip_hidden = skip;
        self
    }

    /// Override the excluded directory name list entirely.
    pub fn excluded_dirs(mut self, dirs: Vec<String>) -> Self {
        self.inner.excluded_dirs = dirs;
        self
    }

    /// Append additional directory names to exclude.
    pub fn also_exclude(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.inner.excluded_dirs.extend(names.into_iter().map(Into::into));
        self
    }

    /// Whether to compute fingerprints (default: `false`).
    pub fn collect_identity(mut self, yes: bool) -> Self {
        self.inner.collect_identity = yes;
        self
    }

    /// Whether to resolve worktree kind (default: `false`).
    pub fn resolve_worktrees(mut self, yes: bool) -> Self {
        self.inner.resolve_worktrees = yes;
        self
    }

    /// Whether to exclude linked worktrees from results (default: `false`).
    pub fn exclude_linked_worktrees(mut self, yes: bool) -> Self {
        self.inner.exclude_linked_worktrees = yes;
        self
    }

    /// Limit the total number of results returned.
    pub fn limit(mut self, n: usize) -> Self {
        self.inner.limit = Some(n);
        self
    }

    /// Number of rayon worker threads (default: logical CPU count).
    pub fn threads(mut self, n: usize) -> Self {
        self.inner.threads = Some(n);
        self
    }

    /// Use the fast (HEAD-based) fingerprint (default: `true`).
    pub fn fast_fingerprint(mut self, fast: bool) -> Self {
        self.inner.fast_fingerprint = fast;
        self
    }

    /// Consume the builder and produce a [`ScanConfig`].
    pub fn build(self) -> ScanConfig {
        self.inner
    }
}

// ── ScanResult ────────────────────────────────────────────────────────────────

/// Summary statistics from a completed scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanStats {
    /// Total wall-clock time for the entire scan.
    pub elapsed: Duration,

    /// Number of directories visited (including skipped subtrees counted once).
    pub dirs_visited: usize,

    /// Number of repositories found before any `limit` is applied.
    pub repos_found: usize,

    /// Number of repositories returned after applying filters and `limit`.
    pub repos_returned: usize,

    /// Number of errors encountered and silently skipped.
    pub errors_skipped: usize,
}

/// Combined output from [`Scanner::scan_with_stats`].
#[derive(Debug)]
pub struct ScanResult {
    /// All discovered [`RepoRecord`]s (post-filter).
    pub records: Vec<RepoRecord>,

    /// Summary statistics for this run.
    pub stats: ScanStats,
}

// ── Scanner ───────────────────────────────────────────────────────────────────

/// Parallel, multi-root git repository scanner.
///
/// Construct with a [`ScanConfig`] and call [`Scanner::scan`] or
/// [`Scanner::scan_with_stats`].
pub struct Scanner {
    config: ScanConfig,
}

impl Scanner {
    /// Create a new `Scanner` with the given configuration.
    pub fn new(config: ScanConfig) -> Self {
        Self { config }
    }

    /// Run the scan and return discovered repositories.
    ///
    /// This is the most ergonomic entry point. For timing / diagnostic
    /// information use [`Scanner::scan_with_stats`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`TrackerError::ScanRootMissing`] if any configured root does
    /// not exist.  Per-directory errors during traversal are silently skipped.
    pub fn scan(self) -> Result<Vec<RepoRecord>> {
        Ok(self.scan_with_stats()?.records)
    }

    /// Run the scan and return both records and summary statistics.
    pub fn scan_with_stats(self) -> Result<ScanResult> {
        let start = Instant::now();

        // Validate all roots up-front.
        for root in &self.config.roots {
            if !root.exists() {
                return Err(TrackerError::ScanRootMissing(root.clone()));
            }
        }

        // Shared counters (behind a cheap mutex – written rarely).
        let dirs_counter  = Arc::new(Mutex::new(0usize));
        let error_counter = Arc::new(Mutex::new(0usize));

        // Build a rayon thread pool if a custom thread count was requested.
        let pool = if let Some(n) = self.config.threads {
            Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .map_err(|e| TrackerError::WatcherInit(e.to_string()))?,
            )
        } else {
            None
        };

        let config  = Arc::new(self.config);
        let results = Arc::new(Mutex::new(Vec::<RepoRecord>::new()));

        let scan_roots: Vec<PathBuf> = config.roots.clone();

        let run_scan = || {
            scan_roots.par_iter().for_each(|root| {
                let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
                walk_dir(
                    &root_canonical,
                    &root_canonical,
                    0,
                    &config,
                    &results,
                    &dirs_counter,
                    &error_counter,
                );
            });
        };

        if let Some(pool) = pool {
            pool.install(run_scan);
        } else {
            run_scan();
        }

        let mut records = Arc::try_unwrap(results)
            .expect("scan: results Arc still held")
            .into_inner()
            .expect("scan: results mutex poisoned");

        let dirs_visited  = *dirs_counter.lock().unwrap();
        let errors_skipped = *error_counter.lock().unwrap();
        let repos_found   = records.len();

        // Sort by workdir for deterministic output.
        records.sort_by(|a, b| a.workdir.cmp(&b.workdir));

        // Apply limit.
        let limit = config.limit;
        if let Some(n) = limit {
            records.truncate(n);
        }

        let repos_returned = records.len();
        let elapsed = start.elapsed();

        Ok(ScanResult {
            records,
            stats: ScanStats {
                elapsed,
                dirs_visited,
                repos_found,
                repos_returned,
                errors_skipped,
            },
        })
    }
}

// ── Internal walker ───────────────────────────────────────────────────────────

/// Recursive parallel directory walker.
///
/// Each call processes `dir`, decides whether it is a git repository, then
/// fans out to its children in parallel (via rayon's `par_iter`).
fn walk_dir(
    dir:           &Path,
    scan_root:     &Path,
    depth:         usize,
    config:        &Arc<ScanConfig>,
    results:       &Arc<Mutex<Vec<RepoRecord>>>,
    dirs_counter:  &Arc<Mutex<usize>>,
    error_counter: &Arc<Mutex<usize>>,
) {
    // Increment visited counter.
    {
        let mut c = dirs_counter.lock().unwrap();
        *c += 1;
    }

    // Check for a `.git` entry in `dir`.
    let dot_git = dir.join(".git");
    let git_entry_meta = std::fs::metadata(&dot_git);

    if let Ok(meta) = git_entry_meta {
        // Found a .git entry – this directory is a git repository root.
        let is_file  = meta.is_file();   // linked worktree
        let is_dir   = meta.is_dir();    // main worktree or regular repo
        let is_bare  = false;            // handled separately below

        if is_file || is_dir {
            // Early filter: skip linked worktrees if requested.
            if is_file && config.exclude_linked_worktrees {
                // Still descend? No – a linked worktree root won't contain
                // nested repos, and descending into it would re-visit files
                // that the main repo already owns.
                return;
            }

            // Attempt to build a record for this repository.
            match build_record(dir, &dot_git, is_file, is_bare, depth, scan_root, config) {
                Ok(record) => {
                    let mut guard = results.lock().unwrap();
                    guard.push(record);
                }
                Err(_) => {
                    let mut c = error_counter.lock().unwrap();
                    *c += 1;
                }
            }

            // Do NOT descend into `.git` subdirectories, but also do not
            // descend *into* the working tree – nested repos (git submodules
            // or independent repos inside this one) should still be found.
            // We fall through to the descent logic below.
        }
    } else {
        // No .git entry – check if this *itself* is a bare repo.
        // Bare repos have `HEAD`, `config`, `objects/`, and `refs/` directly
        // in the directory (no working tree, no `.git` wrapper).
        if looks_like_bare_repo(dir) {
            match build_bare_record(dir, depth, scan_root, config) {
                Ok(record) => {
                    let mut guard = results.lock().unwrap();
                    guard.push(record);
                }
                Err(_) => {
                    let mut c = error_counter.lock().unwrap();
                    *c += 1;
                }
            }
            // Bare repo: no working tree to descend into.
            return;
        }
    }

    // Depth guard – don't recurse past the configured maximum.
    if depth >= config.max_depth {
        return;
    }

    // Read child entries.
    let children: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if !path.is_dir() {
                    return None;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                // Always skip the .git directory itself.
                if name == ".git" {
                    return None;
                }

                // Skip hidden directories if configured.
                if config.skip_hidden && name.starts_with('.') {
                    return None;
                }

                // Skip explicitly excluded names.
                if config.excluded_dirs.iter().any(|ex| ex == name) {
                    return None;
                }

                Some(path)
            })
            .collect(),
        Err(_) => {
            let mut c = error_counter.lock().unwrap();
            *c += 1;
            return;
        }
    };

    // Fan out in parallel.
    children.par_iter().for_each(|child| {
        walk_dir(child, scan_root, depth + 1, config, results, dirs_counter, error_counter);
    });
}

/// Heuristic check for a bare git repository.
///
/// A bare repo has `HEAD`, `config`, `objects/`, and `refs/` directly inside
/// the target directory, with no `.git` subdirectory.
fn looks_like_bare_repo(dir: &Path) -> bool {
    dir.join("HEAD").is_file()
        && dir.join("config").is_file()
        && dir.join("objects").is_dir()
        && dir.join("refs").is_dir()
}

/// Build a [`RepoRecord`] for a non-bare repository at `dir`.
fn build_record(
    dir:        &Path,
    dot_git:    &Path,
    is_file:    bool,
    is_bare:    bool,
    depth:      usize,
    scan_root:  &Path,
    config:     &ScanConfig,
) -> Result<RepoRecord> {
    let git_dir = if is_file {
        // For a linked worktree, the real git-dir is inside the main repo.
        // We keep the .git *file* path here as a placeholder; full resolution
        // is deferred to WorktreeResolver when requested.
        dot_git.to_path_buf()
    } else {
        dot_git.to_path_buf()
    };

    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let workdir = dir.to_path_buf();

    // Optionally resolve the worktree kind.
    let (worktree_kind, effective_is_linked) = if config.resolve_worktrees {
        let resolver = WorktreeResolver::new();
        match resolver.resolve(dir) {
            Ok(info) => {
                let linked = info.kind.is_linked();
                (Some(info.kind), linked)
            }
            Err(_) => (None, is_file),
        }
    } else {
        (None, is_file)
    };

    // Optionally compute the fingerprint and branch.
    let (fingerprint, head_branch, current_branch, upstream_branch, ahead, behind) =
        if config.collect_identity {
            let fp = if config.fast_fingerprint {
                Fingerprinter::fast()
            } else {
                Fingerprinter::new()
            };
            let (fingerprint, head_branch) = match fp.identify(dir) {
                Ok(identity) => (Some(identity.fingerprint), identity.head_branch),
                Err(_)       => (None, None),
            };
            let (current_branch, upstream_branch, ahead, behind) =
                match crate::branch::get_branch_info(dir) {
                    Ok(bi) => (Some(bi.name), bi.upstream, bi.ahead, bi.behind),
                    Err(_) => (None, None, 0, 0),
                };
            (fingerprint, head_branch, current_branch, upstream_branch, ahead, behind)
        } else {
            (None, None, None, None, 0, 0)
        };

    Ok(RepoRecord {
        name,
        workdir,
        git_dir,
        worktree_kind,
        fingerprint,
        is_linked_worktree: effective_is_linked,
        is_bare,
        head_branch,
        current_branch,
        upstream_branch,
        ahead,
        behind,
        depth,
        scan_root: scan_root.to_path_buf(),
    })
}

/// Build a [`RepoRecord`] for a bare repository at `dir`.
fn build_bare_record(
    dir:       &Path,
    depth:     usize,
    scan_root: &Path,
    config:    &ScanConfig,
) -> Result<RepoRecord> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (fingerprint, head_branch) = if config.collect_identity {
        let fp = if config.fast_fingerprint {
            Fingerprinter::fast()
        } else {
            Fingerprinter::new()
        };
        match fp.identify(dir) {
            Ok(identity) => (Some(identity.fingerprint), identity.head_branch),
            Err(_)       => (None, None),
        }
    } else {
        (None, None)
    };

    Ok(RepoRecord {
        name,
        workdir:            dir.to_path_buf(),
        git_dir:            dir.to_path_buf(), // bare: gitdir == workdir
        worktree_kind:      if config.resolve_worktrees {
            Some(WorktreeKind::Bare)
        } else {
            None
        },
        fingerprint,
        is_linked_worktree: false,
        is_bare:            true,
        head_branch,
        current_branch:     None, // bare repos have no working branch
        upstream_branch:    None,
        ahead:              0,
        behind:             0,
        depth,
        scan_root: scan_root.to_path_buf(),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    // ── helpers ──

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn make_repo(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init"]);
        git(&path, &["config", "user.email", "t@t.com"]);
        git(&path, &["config", "user.name", "T"]);
        fs::write(path.join("file.txt"), b"x").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "init"]);
        path
    }

    // ── Basic discovery ──

    #[test]
    fn finds_single_repo() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "my-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "my-repo");
    }

    #[test]
    fn finds_multiple_repos_at_same_depth() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "alpha");
        make_repo(tmp.path(), "beta");
        make_repo(tmp.path(), "gamma");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .build();

        let mut records = Scanner::new(config).scan().unwrap();
        records.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(records.len(), 3);
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn finds_nested_repo() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(&inner).unwrap();
        make_repo(&outer, "outer-repo");
        make_repo(&inner, "inner-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(5)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        // Should find both outer-repo and inner-repo.
        assert!(records.len() >= 2, "expected >=2 repos, got {}", records.len());
    }

    // ── Depth limiting ──

    #[test]
    fn depth_limit_excludes_deep_repos() {
        let tmp = TempDir::new().unwrap();
        // repo at depth 1
        make_repo(tmp.path(), "shallow");
        // repo at depth 3
        let deep_dir = tmp.path().join("a").join("b");
        fs::create_dir_all(&deep_dir).unwrap();
        make_repo(&deep_dir, "deep");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(1)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "shallow");
    }

    // ── Exclusions ──

    #[test]
    fn excluded_dir_is_skipped() {
        let tmp = TempDir::new().unwrap();
        // Repo inside an excluded directory name.
        let excluded = tmp.path().join("node_modules").join("some-pkg");
        fs::create_dir_all(&excluded).unwrap();
        make_repo(&excluded, "hidden-repo");

        // Visible repo alongside it.
        make_repo(tmp.path(), "visible-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(5)
            .build(); // node_modules is in default exclusions

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "visible-repo");
    }

    #[test]
    fn custom_exclusion_is_respected() {
        let tmp = TempDir::new().unwrap();
        let skip_me = tmp.path().join("skip-me");
        fs::create_dir_all(&skip_me).unwrap();
        make_repo(&skip_me, "hidden");
        make_repo(tmp.path(), "visible");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .also_exclude(["skip-me"])
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "visible");
    }

    #[test]
    fn hidden_dirs_skipped_when_enabled() {
        let tmp = TempDir::new().unwrap();
        let hidden = tmp.path().join(".hidden");
        fs::create_dir_all(&hidden).unwrap();
        make_repo(&hidden, "secret");
        make_repo(tmp.path(), "visible");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .skip_hidden(true)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "visible");
    }

    #[test]
    fn hidden_dirs_found_when_skip_hidden_false() {
        let tmp = TempDir::new().unwrap();
        let hidden = tmp.path().join(".hidden");
        fs::create_dir_all(&hidden).unwrap();
        make_repo(&hidden, "secret");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .skip_hidden(false)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "secret");
    }

    // ── Limit ──

    #[test]
    fn limit_caps_results() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            make_repo(tmp.path(), &format!("repo-{i}"));
        }

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(2)
            .limit(2)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 2);
    }

    // ── Identity / fingerprint ──

    #[test]
    fn collect_identity_populates_fingerprint() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "fp-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(2)
            .collect_identity(true)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            records[0].fingerprint.is_some(),
            "fingerprint should be populated"
        );
        assert!(
            records[0].head_branch.is_some(),
            "head_branch should be populated"
        );
    }

    #[test]
    fn no_fingerprint_by_default() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "plain-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(2)
            .build(); // collect_identity defaults to false

        let records = Scanner::new(config).scan().unwrap();
        assert!(records[0].fingerprint.is_none());
        assert!(records[0].head_branch.is_none());
    }

    // ── Worktree resolution ──

    #[test]
    fn resolve_worktrees_populates_kind() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "wt-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(2)
            .resolve_worktrees(true)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            records[0].worktree_kind.is_some(),
            "worktree_kind should be populated"
        );
        assert!(
            matches!(records[0].worktree_kind, Some(WorktreeKind::Main)),
            "a freshly-made repo should be Main"
        );
    }

    #[test]
    fn linked_worktree_exclusion() {
        let tmp      = TempDir::new().unwrap();
        let main_dir = make_repo(tmp.path(), "main-repo");
        let wt_dir   = tmp.path().join("linked-wt");

        let ok = git(
            &main_dir,
            &["worktree", "add", wt_dir.to_str().unwrap(), "-b", "wt-branch"],
        );
        if !ok {
            eprintln!("git worktree add not supported – skipping");
            return;
        }

        // With exclusion: should only see the main repo.
        let config_excl = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .exclude_linked_worktrees(true)
            .build();

        let records_excl = Scanner::new(config_excl).scan().unwrap();
        assert_eq!(
            records_excl.len(), 1,
            "only the main repo should be present when linked worktrees are excluded"
        );
        assert_eq!(records_excl[0].name, "main-repo");

        // Without exclusion: both main and linked worktree should appear.
        let config_incl = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .exclude_linked_worktrees(false)
            .build();

        let records_incl = Scanner::new(config_incl).scan().unwrap();
        assert_eq!(
            records_incl.len(), 2,
            "both main repo and linked worktree should be present"
        );
    }

    // ── Multiple roots ──

    #[test]
    fn multiple_scan_roots() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        make_repo(tmp_a.path(), "repo-a");
        make_repo(tmp_b.path(), "repo-b");

        let config = ScanConfig::builder()
            .roots(vec![tmp_a.path().into(), tmp_b.path().into()])
            .max_depth(2)
            .build();

        let mut records = Scanner::new(config).scan().unwrap();
        records.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "repo-a");
        assert_eq!(records[1].name, "repo-b");
    }

    // ── Stats ──

    #[test]
    fn scan_stats_are_populated() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "stat-repo");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .build();

        let result = Scanner::new(config).scan_with_stats().unwrap();
        assert_eq!(result.records.len(), 1);
        assert!(result.stats.dirs_visited > 0);
        assert_eq!(result.stats.repos_found, 1);
        assert_eq!(result.stats.repos_returned, 1);
        assert!(result.stats.elapsed > Duration::ZERO);
    }

    // ── Error cases ──

    #[test]
    fn missing_root_returns_error() {
        let config = ScanConfig::builder()
            .root("/absolutely/does/not/exist/12345")
            .build();

        let result = Scanner::new(config).scan();
        assert!(
            matches!(result, Err(TrackerError::ScanRootMissing(_))),
            "expected ScanRootMissing, got {:?}",
            result
        );
    }

    // ── Record helpers ──

    #[test]
    fn is_worktree_false_for_main_repo() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "main");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(2)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert!(!records[0].is_worktree());
    }

    #[test]
    fn scan_depth_field_is_correct() {
        let tmp = TempDir::new().unwrap();
        // direct child → depth 1
        make_repo(tmp.path(), "depth-one");

        let config = ScanConfig::builder()
            .root(tmp.path())
            .max_depth(3)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        assert_eq!(records[0].depth, 1);
    }

    #[test]
    fn scan_root_field_matches_configured_root() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "my-repo");

        let root = tmp.path().to_path_buf();
        let config = ScanConfig::builder()
            .root(root.clone())
            .max_depth(3)
            .build();

        let records = Scanner::new(config).scan().unwrap();
        let canonical_root = root.canonicalize().unwrap_or(root);
        assert_eq!(records[0].scan_root, canonical_root);
    }
}
