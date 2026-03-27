//! Repository relocator – find where a git repository went after a move.
//!
//! The [`Relocator`] is the high-level orchestrator that answers the question:
//! *"I know this repository existed at path X; it's no longer there – where
//! did it go?"*
//!
//! ## Algorithm
//!
//! 1. **Collect candidates** – scan one or more search roots (directories) for
//!    git repositories using the fast [`Scanner`].
//! 2. **Score each candidate** against the missing repository's
//!    [`RepoSnapshot`] using a multi-factor heuristic:
//!    - **Fingerprint match** (highest weight) – the content-stable hash must
//!      match exactly for a candidate to be considered at all.
//!    - **Name similarity** – the folder name of the candidate vs the
//!      last-known folder name.
//!    - **Path proximity** – how closely the candidate path resembles the
//!      original (common ancestor depth).
//!    - **Recency** – prefer candidates whose `.git` directory was modified
//!      more recently (they are more likely to be the live repo and not a
//!      stale clone).
//! 3. **Rank and disambiguate** – return the highest-scoring candidate as
//!    [`MoveCandidate`], or surface [`TrackerError::AmbiguousRelocation`] when
//!    two candidates score identically.
//!
//! ## Confidence tiers
//!
//! | Score range   | [`CandidateConfidence`] | Meaning |
//! |---|---|---|
//! | ≥ 90          | `Definitive`            | Fingerprint + name both match |
//! | 60 – 89       | `Likely`                | Fingerprint matches, name differs |
//! | 30 – 59       | `Possible`              | Partial evidence only |
//! | < 30          | `Speculative`           | Weak signal, treat with care |
//!
//! ## Example
//!
//! ```no_run
//! use git_tracker::relocator::{Relocator, RelocatorConfig};
//! use git_tracker::snapshot::RepoSnapshot;
//! use std::path::PathBuf;
//!
//! // We know "my-repo" used to live at /old/path/my-repo.
//! // Load its last snapshot from the SnapshotStore, then:
//!
//! # fn get_snapshot() -> RepoSnapshot { unimplemented!() }
//! let snapshot = get_snapshot();
//!
//! let config = RelocatorConfig::builder()
//!     .search_roots(vec![PathBuf::from("/home/user")])
//!     .max_depth(6)
//!     .build();
//!
//! let relocator = Relocator::new(config);
//! match relocator.locate(&snapshot) {
//!     Ok(candidate) => println!("Found at: {}", candidate.new_path.display()),
//!     Err(e)        => eprintln!("Could not locate: {e}"),
//! }
//! ```

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};
use crate::identity::Fingerprinter;
use crate::scanner::{RepoRecord, ScanConfig, Scanner};
use crate::snapshot::RepoSnapshot;

// ── CandidateConfidence ───────────────────────────────────────────────────────

/// Confidence tier for a relocation candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateConfidence {
    /// Very weak evidence – treat result with caution.
    Speculative,
    /// Some evidence; worth presenting to the user but requires confirmation.
    Possible,
    /// Fingerprint matches; name or path differs – almost certainly the same repo.
    Likely,
    /// Fingerprint + name both match – extremely high confidence.
    Definitive,
}

impl CandidateConfidence {
    /// Derive a confidence tier from a raw score (0–100).
    pub fn from_score(score: u32) -> Self {
        match score {
            90..=u32::MAX => CandidateConfidence::Definitive,
            60..=89       => CandidateConfidence::Likely,
            30..=59       => CandidateConfidence::Possible,
            _             => CandidateConfidence::Speculative,
        }
    }

    /// Returns `true` for `Likely` or `Definitive`.
    pub fn is_high(&self) -> bool {
        matches!(self, CandidateConfidence::Likely | CandidateConfidence::Definitive)
    }
}

impl std::fmt::Display for CandidateConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CandidateConfidence::Speculative => f.write_str("speculative"),
            CandidateConfidence::Possible    => f.write_str("possible"),
            CandidateConfidence::Likely      => f.write_str("likely"),
            CandidateConfidence::Definitive  => f.write_str("definitive"),
        }
    }
}

// ── ScoreBreakdown ────────────────────────────────────────────────────────────

/// Per-factor score breakdown for a single candidate.
///
/// Each factor contributes a value in the range `[0, its max weight]`.
/// The total is the sum of all factors, capped at 100.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// **40 pts max** – fingerprint hash match (all-or-nothing).
    pub fingerprint:   u32,
    /// **30 pts max** – folder-name similarity (exact = 30, prefix = 15, none = 0).
    pub name:          u32,
    /// **20 pts max** – path-ancestry similarity (common prefix depth).
    pub path_proximity: u32,
    /// **10 pts max** – recency of the `.git` directory mtime.
    pub recency:       u32,
    /// Total score (sum, capped at 100).
    pub total:         u32,
}

impl ScoreBreakdown {
    /// Compute a [`ScoreBreakdown`] for `candidate` against `snapshot`.
    fn compute(snapshot: &RepoSnapshot, candidate: &RepoRecord, candidate_fp_hash: &str) -> Self {
        // ── Fingerprint ──────────────────────────────────────────────────────
        let fingerprint: u32 = if candidate_fp_hash == snapshot.fingerprint.hash {
            40
        } else {
            // A candidate that doesn't match the fingerprint is only included
            // when fingerprint computation failed or is disabled; give it 0.
            0
        };

        // ── Name ─────────────────────────────────────────────────────────────
        let old_name = snapshot.name.to_lowercase();
        let new_name = candidate.name.to_lowercase();

        let name: u32 = if new_name == old_name {
            30
        } else if new_name.starts_with(&old_name) || old_name.starts_with(&new_name) {
            15
        } else if longest_common_subsequence_ratio(&old_name, &new_name) >= 0.7 {
            8
        } else {
            0
        };

        // ── Path proximity ───────────────────────────────────────────────────
        let old_components: Vec<_> = snapshot.workdir.components().collect();
        let new_components: Vec<_> = candidate.workdir.components().collect();

        let common_depth = old_components
            .iter()
            .zip(new_components.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // Normalise against the shorter of the two paths.
        let shorter = old_components.len().min(new_components.len()).max(1);
        let ratio   = common_depth as f64 / shorter as f64;

        let path_proximity: u32 = (ratio * 20.0).round() as u32;

        // ── Recency ──────────────────────────────────────────────────────────
        // The repo whose .git dir was modified most recently among all
        // candidates scores highest; here we just reward those modified in the
        // last 24 h and give partial credit for the last week.
        let recency: u32 = {
            let git_dir_path = if candidate.git_dir.is_dir() {
                candidate.git_dir.clone()
            } else {
                // For linked worktrees the git_dir field may be the .git file;
                // check the parent directory instead.
                candidate.workdir.join(".git")
            };

            let mtime_secs = std::fs::metadata(&git_dir_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let now_secs = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let age_secs = now_secs.saturating_sub(mtime_secs);

            if age_secs < 86_400 {
                10         // modified in the last 24 hours
            } else if age_secs < 7 * 86_400 {
                6          // modified in the last week
            } else if age_secs < 30 * 86_400 {
                3          // modified in the last month
            } else {
                0
            }
        };

        let total = (fingerprint + name + path_proximity + recency).min(100);

        Self { fingerprint, name, path_proximity, recency, total }
    }
}

// ── MoveCandidate ─────────────────────────────────────────────────────────────

/// A scored relocation candidate.
///
/// Returned by [`Relocator::locate`] (the best candidate) and
/// [`Relocator::locate_all`] (all candidates above the minimum threshold,
/// in descending score order).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCandidate {
    /// The new working-tree root path where the repository was found.
    pub new_path: PathBuf,

    /// The new `.git` directory path.
    pub new_git_dir: PathBuf,

    /// The fingerprint hash that was matched (mirrors `snapshot.fingerprint.hash`).
    pub fingerprint_hash: String,

    /// Detailed per-factor score breakdown.
    pub score: ScoreBreakdown,

    /// Overall confidence tier derived from `score.total`.
    pub confidence: CandidateConfidence,

    /// The last-known snapshot that was used as the search target.
    pub matched_snapshot: RepoSnapshot,
}

impl MoveCandidate {
    /// Returns `true` when `new_path` is different from the snapshot's
    /// `workdir` (i.e. this is a genuine relocation, not a false alarm).
    pub fn is_actual_move(&self) -> bool {
        self.new_path != self.matched_snapshot.workdir
    }
}

// ── RelocatorConfig ───────────────────────────────────────────────────────────

/// Configuration for a [`Relocator`] instance.
#[derive(Debug, Clone)]
pub struct RelocatorConfig {
    /// Directories to search for the moved repository.
    ///
    /// If empty, the parent directory of the last-known path is used as a
    /// single-root fallback (see [`Relocator::locate`]).
    pub search_roots: Vec<PathBuf>,

    /// Maximum scan depth within each search root.  Default: `8`.
    pub max_depth: usize,

    /// Minimum total score for a candidate to be included in results.
    /// Candidates below this threshold are silently discarded.  Default: `40`.
    pub min_score: u32,

    /// When `true`, skip linked worktrees when scanning for candidates.
    /// Default: `true` (a moved *main* repo is the target, not its worktrees).
    pub exclude_linked_worktrees: bool,

    /// Use the fast (HEAD-commit) fingerprint strategy.  Default: `true`.
    pub fast_fingerprint: bool,

    /// Maximum number of candidates to evaluate before stopping.
    /// `None` = evaluate all.  Default: `None`.
    pub candidate_limit: Option<usize>,

    /// Number of rayon threads to use for scanning.  `None` = auto.
    pub threads: Option<usize>,
}

impl Default for RelocatorConfig {
    fn default() -> Self {
        Self {
            search_roots:             Vec::new(),
            max_depth:                8,
            min_score:                40,
            exclude_linked_worktrees: true,
            fast_fingerprint:         true,
            candidate_limit:          None,
            threads:                  None,
        }
    }
}

impl RelocatorConfig {
    /// Create a [`RelocatorConfigBuilder`].
    pub fn builder() -> RelocatorConfigBuilder {
        RelocatorConfigBuilder::default()
    }
}

// ── RelocatorConfigBuilder ────────────────────────────────────────────────────

/// Builder for [`RelocatorConfig`].
#[derive(Debug, Default)]
pub struct RelocatorConfigBuilder {
    inner: RelocatorConfig,
}

impl RelocatorConfigBuilder {
    /// Set the search roots.
    pub fn search_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.inner.search_roots = roots;
        self
    }

    /// Append a single search root.
    pub fn search_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.inner.search_roots.push(root.into());
        self
    }

    /// Maximum scan depth (default: 8).
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.inner.max_depth = depth;
        self
    }

    /// Minimum score threshold (default: 40).
    pub fn min_score(mut self, score: u32) -> Self {
        self.inner.min_score = score;
        self
    }

    /// Whether to exclude linked worktrees from candidates (default: `true`).
    pub fn exclude_linked_worktrees(mut self, yes: bool) -> Self {
        self.inner.exclude_linked_worktrees = yes;
        self
    }

    /// Use fast fingerprint strategy (default: `true`).
    pub fn fast_fingerprint(mut self, fast: bool) -> Self {
        self.inner.fast_fingerprint = fast;
        self
    }

    /// Limit the number of candidates evaluated.
    pub fn candidate_limit(mut self, n: usize) -> Self {
        self.inner.candidate_limit = Some(n);
        self
    }

    /// Number of rayon worker threads.
    pub fn threads(mut self, n: usize) -> Self {
        self.inner.threads = Some(n);
        self
    }

    /// Consume the builder and produce a [`RelocatorConfig`].
    pub fn build(self) -> RelocatorConfig {
        self.inner
    }
}

// ── Relocator ─────────────────────────────────────────────────────────────────

/// Finds the new location of a moved git repository.
///
/// See the [module-level documentation](self) for the full algorithm.
pub struct Relocator {
    config: RelocatorConfig,
}

impl Relocator {
    /// Create a new `Relocator` with the given configuration.
    pub fn new(config: RelocatorConfig) -> Self {
        Self { config }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Locate the best candidate for a moved repository described by `snapshot`.
    ///
    /// Returns the single best [`MoveCandidate`] above the configured minimum
    /// score threshold.
    ///
    /// # Errors
    ///
    /// * [`TrackerError::RepoNotFound`] – no candidate found above the threshold.
    /// * [`TrackerError::AmbiguousRelocation`] – two candidates share the same
    ///   top score; caller must disambiguate (use [`Relocator::locate_all`]).
    pub fn locate(&self, snapshot: &RepoSnapshot) -> Result<MoveCandidate> {
        let mut candidates = self.locate_all(snapshot)?;

        if candidates.is_empty() {
            return Err(TrackerError::RepoNotFound {
                name:        snapshot.name.clone(),
                fingerprint: snapshot.fingerprint.hash[..16].to_string(),
            });
        }

        // Check for a tie at the top.
        let top_score = candidates[0].score.total;
        let tied: Vec<_> = candidates
            .iter()
            .take_while(|c| c.score.total == top_score)
            .collect();

        if tied.len() > 1 {
            return Err(TrackerError::AmbiguousRelocation {
                name:  snapshot.name.clone(),
                count: tied.len(),
            });
        }

        Ok(candidates.remove(0))
    }

    /// Locate **all** candidates above the minimum score threshold, in
    /// descending score order.
    ///
    /// Returns an empty `Vec` (not an error) when nothing is found.
    /// Use this when you want to present a ranked list to the user.
    pub fn locate_all(&self, snapshot: &RepoSnapshot) -> Result<Vec<MoveCandidate>> {
        let roots = self.effective_search_roots(snapshot);

        // Run the scanner.
        let scan_config = self.build_scan_config(roots);
        let records = Scanner::new(scan_config).scan()?;

        // Score and filter.
        let mut candidates: Vec<MoveCandidate> = records
            .into_iter()
            .filter_map(|record| self.score_candidate(snapshot, record))
            .filter(|c| c.score.total >= self.config.min_score)
            .collect();

        // Sort descending by total score, then ascending by path for stability.
        candidates.sort_by(|a, b| {
            b.score.total
                .cmp(&a.score.total)
                .then_with(|| a.new_path.cmp(&b.new_path))
        });

        // Apply candidate limit.
        if let Some(limit) = self.config.candidate_limit {
            candidates.truncate(limit);
        }

        Ok(candidates)
    }

    /// Quick check: does the repository described by `snapshot` still exist at
    /// its last-known location?
    ///
    /// Returns `true` when the path exists **and** the fingerprint still
    /// matches.  Returns `false` on any error (e.g. I/O failure).
    pub fn is_still_present(&self, snapshot: &RepoSnapshot) -> bool {
        if !snapshot.workdir.exists() {
            return false;
        }

        // Verify fingerprint if possible.
        let fp_result = if self.config.fast_fingerprint {
            Fingerprinter::fast()
        } else {
            Fingerprinter::new()
        }
        .identify(&snapshot.workdir);

        match fp_result {
            Ok(identity) => identity.fingerprint.hash == snapshot.fingerprint.hash,
            Err(_)       => false,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Determine the effective list of search roots.
    ///
    /// If [`RelocatorConfig::search_roots`] is empty, fall back to:
    /// 1. The parent of the last-known workdir (most likely location for a
    ///    same-directory rename).
    /// 2. The grandparent (one level up, for moves within the same project
    ///    area).
    fn effective_search_roots(&self, snapshot: &RepoSnapshot) -> Vec<PathBuf> {
        if !self.config.search_roots.is_empty() {
            return self.config.search_roots.clone();
        }

        let mut roots = Vec::new();

        // Parent of the old workdir.
        if let Some(parent) = snapshot.workdir.parent() {
            if parent.exists() {
                roots.push(parent.to_path_buf());
            }
        }

        // Grandparent – often the common ancestor for project moves.
        if let Some(grandparent) = snapshot
            .workdir
            .parent()
            .and_then(|p| p.parent())
        {
            if grandparent.exists() && !roots.contains(&grandparent.to_path_buf()) {
                roots.push(grandparent.to_path_buf());
            }
        }

        // Ultimate fallback: current working directory.
        if roots.is_empty() {
            if let Ok(cwd) = std::env::current_dir() {
                roots.push(cwd);
            }
        }

        roots
    }

    /// Build a [`ScanConfig`] from the relocator settings.
    fn build_scan_config(&self, roots: Vec<PathBuf>) -> ScanConfig {
        ScanConfig::builder()
            .roots(roots)
            .max_depth(self.config.max_depth)
            // Always collect identity – fingerprint matching is our primary
            // signal and is required for any meaningful scoring.
            .collect_identity(true)
            .fast_fingerprint(self.config.fast_fingerprint)
            .exclude_linked_worktrees(self.config.exclude_linked_worktrees)
            .build()
    }

    /// Score a single scan record against the target snapshot.
    ///
    /// Returns `None` when the record's fingerprint is missing (can happen if
    /// the repo couldn't be opened during scanning) or when the path is
    /// identical to the snapshot's workdir (it hasn't moved).
    fn score_candidate(
        &self,
        snapshot: &RepoSnapshot,
        record:   RepoRecord,
    ) -> Option<MoveCandidate> {
        // Skip if the fingerprint is absent (scan couldn't open the repo).
        let record_fp = record.fingerprint.as_ref()?;

        // We only score repos whose fingerprint matches the target.
        // (Candidates with no fingerprint match are useless for relocation.)
        if record_fp.hash != snapshot.fingerprint.hash {
            return None;
        }

        let breakdown = ScoreBreakdown::compute(snapshot, &record, &record_fp.hash);
        let confidence = CandidateConfidence::from_score(breakdown.total);

        Some(MoveCandidate {
            new_path:         record.workdir.clone(),
            new_git_dir:      record.git_dir.clone(),
            fingerprint_hash: record_fp.hash.clone(),
            score:            breakdown,
            confidence,
            matched_snapshot: snapshot.clone(),
        })
    }
}

// ── Utility functions ─────────────────────────────────────────────────────────

/// Compute the ratio of longest-common-subsequence length to the length of the
/// longer input string.  Returns a value in `[0.0, 1.0]`.
///
/// Used for fuzzy name matching.  O(m·n) time and space, but inputs are
/// always short folder names so this is fine.
fn longest_common_subsequence_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    if m == 0 || n == 0 {
        return 0.0;
    }

    // Use a rolling two-row DP table to keep memory O(n).
    let mut prev = vec![0usize; n + 1];
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        for j in 1..=n {
            curr[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1] + 1
            } else {
                curr[j - 1].max(prev[j])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }

    let lcs = prev[n];
    lcs as f64 / m.max(n) as f64
}

/// Count the number of path components that two paths share from the root.
///
/// e.g. `/home/user/projects` vs `/home/user/dev` → 2 common components.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn common_ancestor_depth(a: &Path, b: &Path) -> usize {
    a.components()
        .zip(b.components())
        .take_while(|(ca, cb)| ca == cb)
        .count()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{FingerprintKind, RepoFingerprint};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    // ── Helpers ──

    fn make_fp(hash: &str) -> RepoFingerprint {
        RepoFingerprint::from_raw(
            format!("{:0<64}", hash),
            FingerprintKind::RootCommit,
            None,
        )
    }

    fn make_snapshot(name: &str, workdir: &str, hash: &str) -> RepoSnapshot {
        RepoSnapshot {
            fingerprint:         make_fp(hash),
            workdir:             PathBuf::from(workdir),
            git_dir:             PathBuf::from(format!("{workdir}/.git")),
            name:                name.into(),
            is_bare:             false,
            is_linked_worktree:  false,
            snapshotted_at:      0,
            scan_root:           PathBuf::from("/projects"),
            current_branch:      None,
        }
    }

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

    fn make_git_repo(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init"]);
        git(&path, &["config", "user.email", "t@t.com"]);
        git(&path, &["config", "user.name", "T"]);
        fs::write(path.join("file.txt"), b"hello").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "init"]);
        path
    }

    // ── CandidateConfidence ──

    #[test]
    fn confidence_tiers_from_score() {
        assert_eq!(CandidateConfidence::from_score(95),  CandidateConfidence::Definitive);
        assert_eq!(CandidateConfidence::from_score(90),  CandidateConfidence::Definitive);
        assert_eq!(CandidateConfidence::from_score(89),  CandidateConfidence::Likely);
        assert_eq!(CandidateConfidence::from_score(60),  CandidateConfidence::Likely);
        assert_eq!(CandidateConfidence::from_score(59),  CandidateConfidence::Possible);
        assert_eq!(CandidateConfidence::from_score(30),  CandidateConfidence::Possible);
        assert_eq!(CandidateConfidence::from_score(29),  CandidateConfidence::Speculative);
        assert_eq!(CandidateConfidence::from_score(0),   CandidateConfidence::Speculative);
    }

    #[test]
    fn confidence_is_high_predicate() {
        assert!(CandidateConfidence::Definitive.is_high());
        assert!(CandidateConfidence::Likely.is_high());
        assert!(!CandidateConfidence::Possible.is_high());
        assert!(!CandidateConfidence::Speculative.is_high());
    }

    #[test]
    fn confidence_ordering() {
        assert!(CandidateConfidence::Definitive  > CandidateConfidence::Likely);
        assert!(CandidateConfidence::Likely      > CandidateConfidence::Possible);
        assert!(CandidateConfidence::Possible    > CandidateConfidence::Speculative);
    }

    #[test]
    fn confidence_display() {
        assert_eq!(CandidateConfidence::Speculative.to_string(), "speculative");
        assert_eq!(CandidateConfidence::Possible.to_string(),    "possible");
        assert_eq!(CandidateConfidence::Likely.to_string(),      "likely");
        assert_eq!(CandidateConfidence::Definitive.to_string(),  "definitive");
    }

    // ── LCS ratio ──

    #[test]
    fn lcs_ratio_identical_strings() {
        let r = longest_common_subsequence_ratio("my-repo", "my-repo");
        assert!((r - 1.0).abs() < 1e-9, "identical strings should give ratio 1.0, got {r}");
    }

    #[test]
    fn lcs_ratio_completely_different() {
        let r = longest_common_subsequence_ratio("abc", "xyz");
        assert_eq!(r, 0.0, "no common chars should give ratio 0.0");
    }

    #[test]
    fn lcs_ratio_partial_overlap() {
        let r = longest_common_subsequence_ratio("my-repo", "my-repository");
        assert!(r > 0.5, "partial overlap should give ratio > 0.5, got {r}");
    }

    #[test]
    fn lcs_ratio_empty_string() {
        let r = longest_common_subsequence_ratio("", "something");
        assert_eq!(r, 0.0);
    }

    // ── common_ancestor_depth ──

    #[test]
    fn common_ancestor_depth_same_path() {
        let a = Path::new("/home/user/projects/repo");
        let depth = common_ancestor_depth(a, a);
        // Component count varies by platform (Unix root "/" is a component, Windows has no
        // equivalent separator component). Just assert that a path matches itself fully.
        let expected = a.components().count();
        assert_eq!(depth, expected, "same path should share all components");
    }

    #[test]
    fn common_ancestor_depth_sibling_dirs() {
        let a = Path::new("/home/user/projects/alpha");
        let b = Path::new("/home/user/projects/beta");
        let depth = common_ancestor_depth(a, b);
        // '/', 'home', 'user', 'projects' are common → 4 on Unix, but may
        // vary; just check it's > 0 and < 5.
        assert!(depth > 0 && depth < 5, "got depth {depth}");
    }

    #[test]
    fn common_ancestor_depth_no_overlap() {
        // On Windows paths may share no components with Unix-style paths.
        // Use paths that definitely share nothing.
        let a = Path::new("alpha/beta");
        let b = Path::new("gamma/delta");
        let depth = common_ancestor_depth(a, b);
        assert_eq!(depth, 0);
    }

    // ── ScoreBreakdown ──

    fn make_record(name: &str, workdir: &str) -> RepoRecord {
        RepoRecord {
            name:                name.into(),
            workdir:             PathBuf::from(workdir),
            git_dir:             PathBuf::from(format!("{workdir}/.git")),
            worktree_kind:       None,
            fingerprint:         None, // not used in ScoreBreakdown::compute
            is_linked_worktree:  false,
            is_bare:             false,
            head_branch:         None,
            current_branch:      None,
            upstream_branch:     None,
            ahead:               0,
            behind:              0,
            depth:               1,
            scan_root:           PathBuf::from("/"),
        }
    }

    #[test]
    fn score_breakdown_exact_name_match_adds_30() {
        let snap = make_snapshot("my-repo", "/old/my-repo", "aaaa");
        let rec  = make_record("my-repo", "/new/my-repo");
        let fp   = format!("{:0<64}", "aaaa");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        assert_eq!(score.name, 30, "exact name match should give 30 pts");
    }

    #[test]
    fn score_breakdown_prefix_name_match_adds_15() {
        let snap = make_snapshot("repo", "/old/repo", "aaaa");
        let rec  = make_record("repo-backup", "/new/repo-backup");
        let fp   = format!("{:0<64}", "aaaa");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        assert_eq!(score.name, 15, "prefix name match should give 15 pts");
    }

    #[test]
    fn score_breakdown_no_name_match_adds_0_or_lcs() {
        let snap = make_snapshot("completely-different", "/old/completely-different", "aaaa");
        let rec  = make_record("xyz",                   "/new/xyz");
        let fp   = format!("{:0<64}", "aaaa");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        // Should be 0 or at most 8 (LCS credit); definitely not 15 or 30.
        assert!(score.name <= 8, "unrelated names should score <= 8, got {}", score.name);
    }

    #[test]
    fn score_breakdown_fingerprint_match_adds_40() {
        let snap = make_snapshot("r", "/old/r", "bbbb");
        let rec  = make_record("r", "/new/r");
        let fp   = format!("{:0<64}", "bbbb");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        assert_eq!(score.fingerprint, 40);
    }

    #[test]
    fn score_breakdown_fingerprint_mismatch_adds_0() {
        let snap = make_snapshot("r", "/old/r", "bbbb");
        let rec  = make_record("r", "/new/r");
        let fp   = format!("{:0<64}", "different-hash");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        assert_eq!(score.fingerprint, 0);
    }

    #[test]
    fn score_breakdown_total_does_not_exceed_100() {
        let snap = make_snapshot("r", "/projects/r", "cccc");
        let rec  = make_record("r", "/projects/r-new");
        let fp   = format!("{:0<64}", "cccc");

        let score = ScoreBreakdown::compute(&snap, &rec, &fp);
        assert!(score.total <= 100, "total must not exceed 100, got {}", score.total);
    }

    // ── RelocatorConfig builder ──

    #[test]
    fn relocator_config_builder_defaults() {
        let cfg = RelocatorConfig::default();
        assert!(cfg.search_roots.is_empty());
        assert_eq!(cfg.max_depth, 8);
        assert_eq!(cfg.min_score, 40);
        assert!(cfg.exclude_linked_worktrees);
        assert!(cfg.fast_fingerprint);
        assert!(cfg.candidate_limit.is_none());
    }

    #[test]
    fn relocator_config_builder_sets_fields() {
        let cfg = RelocatorConfig::builder()
            .max_depth(4)
            .min_score(60)
            .exclude_linked_worktrees(false)
            .fast_fingerprint(false)
            .candidate_limit(5)
            .threads(2)
            .build();

        assert_eq!(cfg.max_depth, 4);
        assert_eq!(cfg.min_score, 60);
        assert!(!cfg.exclude_linked_worktrees);
        assert!(!cfg.fast_fingerprint);
        assert_eq!(cfg.candidate_limit, Some(5));
        assert_eq!(cfg.threads, Some(2));
    }

    // ── effective_search_roots ──

    #[test]
    fn effective_search_roots_uses_config_roots_when_set() {
        let tmp  = TempDir::new().unwrap();
        let snap = make_snapshot("r", "/old/r", "aaaa");

        let config = RelocatorConfig::builder()
            .search_root(tmp.path())
            .build();

        let relocator = Relocator::new(config);
        let roots = relocator.effective_search_roots(&snap);

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0], tmp.path());
    }

    #[test]
    fn effective_search_roots_falls_back_to_parent_of_snapshot_workdir() {
        let tmp  = TempDir::new().unwrap();
        let repo = make_git_repo(tmp.path(), "my-repo");

        // Pretend the repo was at `repo` (which exists).
        let snap = RepoSnapshot {
            fingerprint:        make_fp("aaaa"),
            workdir:            repo.clone(),
            git_dir:            repo.join(".git"),
            name:               "my-repo".into(),
            is_bare:            false,
            is_linked_worktree: false,
            snapshotted_at:     0,
            scan_root:          tmp.path().into(),
            current_branch:     None,
        };

        let config    = RelocatorConfig::default(); // no search_roots
        let relocator = Relocator::new(config);
        let roots     = relocator.effective_search_roots(&snap);

        // parent of `repo` is `tmp.path()` – should appear in roots.
        let canonical_tmp = tmp.path().canonicalize().unwrap_or_else(|_| tmp.path().into());
        let found = roots
            .iter()
            .any(|r| r.canonicalize().unwrap_or_else(|_| r.clone()) == canonical_tmp);

        assert!(found, "parent of snapshot workdir should be a search root; roots = {roots:?}");
    }

    // ── is_still_present ──

    #[test]
    fn is_still_present_true_for_existing_repo() {
        let tmp  = TempDir::new().unwrap();
        let path = make_git_repo(tmp.path(), "live-repo");

        // Get the real fingerprint.
        let identity = Fingerprinter::fast().identify(&path).expect("identify");
        let snap = RepoSnapshot {
            fingerprint:        identity.fingerprint,
            workdir:            path.clone(),
            git_dir:            path.join(".git"),
            name:               "live-repo".into(),
            is_bare:            false,
            is_linked_worktree: false,
            snapshotted_at:     0,
            scan_root:          tmp.path().into(),
            current_branch:     None,
        };

        let relocator = Relocator::new(RelocatorConfig::default());
        assert!(relocator.is_still_present(&snap));
    }

    #[test]
    fn is_still_present_false_for_missing_path() {
        let snap = make_snapshot("gone", "/absolutely/does/not/exist/repo", "aaaa");
        let relocator = Relocator::new(RelocatorConfig::default());
        assert!(!relocator.is_still_present(&snap));
    }

    // ── locate (end-to-end) ──

    #[test]
    fn locate_finds_moved_repo() {
        let tmp       = TempDir::new().unwrap();
        let original  = make_git_repo(tmp.path(), "my-repo");

        // Capture fingerprint before the move.
        let identity  = Fingerprinter::fast().identify(&original).expect("identify");

        // Simulate a move: rename the directory.
        let moved = tmp.path().join("my-repo-moved");
        fs::rename(&original, &moved).expect("rename");

        // Build a snapshot that points to the OLD (now missing) location.
        let snap = RepoSnapshot {
            fingerprint:        identity.fingerprint,
            workdir:            original.clone(),
            git_dir:            original.join(".git"),
            name:               "my-repo".into(),
            is_bare:            false,
            is_linked_worktree: false,
            snapshotted_at:     0,
            scan_root:          tmp.path().into(),
            current_branch:     None,
        };

        let config = RelocatorConfig::builder()
            .search_root(tmp.path())
            .max_depth(3)
            .min_score(30)
            .build();

        let relocator = Relocator::new(config);
        let candidate = relocator.locate(&snap).expect("locate failed");

        // The candidate should point to the moved path.
        assert_eq!(
            candidate.new_path.canonicalize().unwrap(),
            moved.canonicalize().unwrap(),
            "locate should find the moved repo"
        );
        assert!(candidate.is_actual_move(), "should be flagged as an actual move");
        assert!(
            candidate.confidence.is_high(),
            "confidence should be Likely or Definitive, got {:?}",
            candidate.confidence
        );
    }

    #[test]
    fn locate_returns_not_found_when_no_match() {
        let tmp  = TempDir::new().unwrap();
        // Empty directory – nothing to find.
        let snap = make_snapshot("ghost", "/ghost/path", "deadbeef");

        let config = RelocatorConfig::builder()
            .search_root(tmp.path())
            .max_depth(3)
            .build();

        let result = Relocator::new(config).locate(&snap);
        assert!(
            matches!(result, Err(TrackerError::RepoNotFound { .. })),
            "expected RepoNotFound, got {:?}",
            result
        );
    }

    #[test]
    fn locate_all_returns_empty_vec_when_nothing_found() {
        let tmp   = TempDir::new().unwrap();
        let snap  = make_snapshot("ghost", "/ghost/path", "deadbeef");

        let config = RelocatorConfig::builder()
            .search_root(tmp.path())
            .max_depth(3)
            .build();

        let candidates = Relocator::new(config)
            .locate_all(&snap)
            .expect("locate_all should not error");

        assert!(candidates.is_empty());
    }

    #[test]
    fn locate_all_sorted_descending_by_score() {
        // Construct two repos with the same fingerprint but in different
        // locations. This exercises the sorting logic without requiring an
        // actual move.  We use the same repo cloned into two paths.

        let tmp    = TempDir::new().unwrap();
        let source = make_git_repo(tmp.path(), "source");
        let clone  = tmp.path().join("clone");

        let ok = git(
            tmp.path(),
            &["clone", source.to_str().unwrap(), clone.to_str().unwrap()],
        );
        if !ok {
            eprintln!("git clone failed – skipping");
            return;
        }

        let identity = Fingerprinter::fast().identify(&source).expect("identify");

        // Snapshot points to a non-existent third path so both are candidates.
        let snap = RepoSnapshot {
            fingerprint:        identity.fingerprint,
            workdir:            PathBuf::from("/somewhere/else/source"),
            git_dir:            PathBuf::from("/somewhere/else/source/.git"),
            name:               "source".into(),
            is_bare:            false,
            is_linked_worktree: false,
            snapshotted_at:     0,
            scan_root:          tmp.path().into(),
            current_branch:     None,
        };

        let config = RelocatorConfig::builder()
            .search_root(tmp.path())
            .max_depth(3)
            .min_score(0) // accept any score
            .build();

        let candidates = Relocator::new(config)
            .locate_all(&snap)
            .expect("locate_all");

        // Results must be in descending score order.
        for window in candidates.windows(2) {
            assert!(
                window[0].score.total >= window[1].score.total,
                "candidates not sorted: {} < {}",
                window[0].score.total,
                window[1].score.total
            );
        }
    }

    // ── MoveCandidate::is_actual_move ──

    #[test]
    fn is_actual_move_true_when_paths_differ() {
        let snap = make_snapshot("r", "/old/r", "aaaa");
        let fp   = make_fp("aaaa");
        let breakdown = ScoreBreakdown {
            fingerprint: 40, name: 30, path_proximity: 10, recency: 5, total: 85,
        };

        let candidate = MoveCandidate {
            new_path:         PathBuf::from("/new/r"),
            new_git_dir:      PathBuf::from("/new/r/.git"),
            fingerprint_hash: fp.hash.clone(),
            score:            breakdown,
            confidence:       CandidateConfidence::Definitive,
            matched_snapshot: snap,
        };

        assert!(candidate.is_actual_move());
    }

    #[test]
    fn is_actual_move_false_when_paths_same() {
        let snap = make_snapshot("r", "/same/r", "aaaa");
        let fp   = make_fp("aaaa");
        let breakdown = ScoreBreakdown {
            fingerprint: 40, name: 30, path_proximity: 20, recency: 10, total: 100,
        };

        let candidate = MoveCandidate {
            new_path:         PathBuf::from("/same/r"),
            new_git_dir:      PathBuf::from("/same/r/.git"),
            fingerprint_hash: fp.hash.clone(),
            score:            breakdown,
            confidence:       CandidateConfidence::Definitive,
            matched_snapshot: snap,
        };

        assert!(!candidate.is_actual_move());
    }
}
