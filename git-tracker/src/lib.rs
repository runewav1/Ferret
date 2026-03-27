//! # git-tracker
//!
//! A standalone Rust library for high-performance git repository discovery,
//! identity tracking, move detection, and worktree awareness. Designed to
//! integrate with Ferret but usable entirely independently.
//!
//! ## Core capabilities
//!
//! | Capability | Module |
//! |---|---|
//! | Identify & fingerprint repositories | [`identity`] |
//! | Differentiate worktrees from full repos | [`worktree`] |
//! | Watch for repository move / rename events | [`watcher`] |
//! | Locate a repo after it has been moved | [`relocator`] |
//! | Scan directories for git repos at high speed | [`scanner`] |
//! | Persist and compare snapshots across runs | [`snapshot`] |
//!
//! ## Design philosophy
//!
//! * **Zero false positives on worktrees.** Git worktrees look like repositories
//!   from the outside (they contain a `.git` *file*, not a directory). git-tracker
//!   fully resolves the chain back to the main repository so callers always know
//!   which physical checkout they are dealing with.
//!
//! * **Speed first.** [`scanner`] uses a work-stealing thread pool (rayon) and
//!   avoids `git2` for the hot path; it only opens the repository with libgit2
//!   when richer metadata is genuinely required.
//!
//! * **Identity is content, not location.** A repository's identity is derived
//!   from its root-commit hash (or a synthetic fingerprint when history is
//!   absent). This means a moved repository is still the same repository.
//!
//! * **Explicit snapshot model.** Rather than keeping a background daemon alive,
//!   git-tracker uses a lightweight on-disk [`snapshot`] that is compared against
//!   the live file-system to detect moves. An optional file-system watcher
//!   ([`watcher`]) can trigger re-scans in real time.

#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod branch;
pub mod error;
pub mod identity;
pub mod relocator;
pub mod scanner;
pub mod snapshot;
pub mod watcher;
pub mod worktree;

// ── Re-exports that form the public surface area ──────────────────────────────

pub use branch::{BranchInfo, BranchSummary, get_branch_info};
pub use error::{Result, TrackerError};
pub use identity::{RepoFingerprint, RepoIdentity};
pub use relocator::{MoveCandidate, Relocator, RelocatorConfig};
pub use scanner::{RepoRecord, ScanConfig, Scanner};
pub use snapshot::{RepoSnapshot, SnapshotStore};
pub use watcher::{MoveEvent, RepoWatcher, WatcherConfig};
pub use worktree::{WorktreeInfo, WorktreeKind};
