//! Error types for git-tracker.

use std::path::PathBuf;
use thiserror::Error;

/// Every failure mode that git-tracker can surface.
#[derive(Debug, Error)]
pub enum TrackerError {
    // ── I/O ──────────────────────────────────────────────────────────────────
    /// A low-level OS / file-system error.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// The path associated with the I/O failure.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A plain I/O error with no associated path (use [`TrackerError::Io`]
    /// whenever a path is available).
    #[error("I/O error: {0}")]
    IoPlain(#[from] std::io::Error),

    // ── Git / libgit2 ─────────────────────────────────────────────────────────
    /// libgit2 returned an error.
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    /// A path was expected to be inside a git repository but is not.
    #[error("Not a git repository: `{}`", .0.display())]
    NotARepo(PathBuf),

    /// The repository exists but has no commits yet (unborn HEAD).
    #[error("Repository has no commits (unborn HEAD): `{}`", .0.display())]
    UnbornHead(PathBuf),

    // ── Worktree ──────────────────────────────────────────────────────────────
    /// A `.git` file pointed to a location that does not exist or is
    /// not a valid git directory.
    #[error("Broken worktree link in `{}`: target `{}` not found", .link.display(), .target.display())]
    BrokenWorktreeLink {
        /// The `.git` file that contained the broken pointer.
        link: PathBuf,
        /// The target path that the `.git` file pointed to.
        target: PathBuf,
    },

    /// The worktree's main repository could not be resolved.
    #[error("Cannot resolve main repository for worktree `{}`", .0.display())]
    WorktreeMainRepoUnresolvable(PathBuf),

    // ── Scanner ───────────────────────────────────────────────────────────────
    /// One of the root directories passed to the scanner does not exist.
    #[error("Scan root does not exist: `{}`", .0.display())]
    ScanRootMissing(PathBuf),

    /// A directory entry could not be read during scanning.
    #[error("Cannot read directory entry: {0}")]
    DirEntry(String),

    // ── Snapshot ──────────────────────────────────────────────────────────────
    /// The snapshot file could not be parsed.
    #[error("Snapshot parse error in `{}`: {source}", .path.display())]
    SnapshotParse {
        /// The snapshot file that could not be parsed.
        path: PathBuf,
        /// The underlying JSON parse error.
        #[source]
        source: serde_json::Error,
    },

    /// The snapshot file could not be serialized.
    #[error("Snapshot serialize error: {0}")]
    SnapshotSerialize(#[source] serde_json::Error),

    // ── Watcher ───────────────────────────────────────────────────────────────
    /// The underlying `notify` watcher failed to initialise.
    #[error("File-system watcher error: {0}")]
    WatcherInit(String),

    /// A path could not be registered with the file-system watcher.
    #[error("Cannot watch path `{}`: {reason}", .path.display())]
    WatcherRegister {
        /// The path that could not be registered with the watcher.
        path: PathBuf,
        /// Human-readable reason for the failure.
        reason: String,
    },

    // ── Relocator ─────────────────────────────────────────────────────────────
    /// No candidate location was found for a moved repository.
    #[error("Could not locate repository `{name}` (fingerprint {fingerprint}) after move")]
    RepoNotFound {
        /// The human-readable repository name.
        name: String,
        /// Short (16-char) fingerprint prefix used in the search.
        fingerprint: String,
    },

    /// Multiple equally-good candidates were found and the caller must
    /// disambiguate.
    #[error("Ambiguous relocation for `{name}`: {count} equally-scored candidates found")]
    AmbiguousRelocation {
        /// The repository name that could not be unambiguously relocated.
        name: String,
        /// Number of equally-scored candidates that were found.
        count: usize,
    },

    // ── Identity ──────────────────────────────────────────────────────────────
    /// The repository's fingerprint could not be computed.
    #[error("Cannot compute fingerprint for `{}`: {reason}", .path.display())]
    FingerprintFailed {
        /// The repository path for which fingerprinting failed.
        path: PathBuf,
        /// Human-readable description of why fingerprinting failed.
        reason: String,
    },

    // ── Generic ───────────────────────────────────────────────────────────────
    /// A catch-all for contextual messages added at call sites.
    #[error("{context}: {source}")]
    WithContext {
        /// The human-readable context message added at the call site.
        context: String,
        /// The original error being wrapped.
        #[source]
        source: Box<TrackerError>,
    },
}

impl TrackerError {
    /// Wrap `self` with an additional human-readable context message.
    ///
    /// ```
    /// # use git_tracker::TrackerError;
    /// let err = TrackerError::WatcherInit("driver missing".into());
    /// let wrapped = err.context("initialising repository watcher");
    /// assert!(wrapped.to_string().contains("initialising repository watcher"));
    /// ```
    pub fn context(self, ctx: impl Into<String>) -> Self {
        TrackerError::WithContext {
            context: ctx.into(),
            source: Box::new(self),
        }
    }

    /// Convenience constructor: I/O error with a path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        TrackerError::Io {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor: broken worktree link.
    pub fn broken_worktree(link: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        TrackerError::BrokenWorktreeLink {
            link: link.into(),
            target: target.into(),
        }
    }

    /// Convenience constructor: fingerprint failure.
    pub fn fingerprint(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        TrackerError::FingerprintFailed {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// Returns `true` if this error originated from libgit2.
    pub fn is_git_error(&self) -> bool {
        matches!(self, TrackerError::Git(_))
    }

    /// Returns `true` if the error indicates a missing / non-repo path.
    pub fn is_not_a_repo(&self) -> bool {
        matches!(self, TrackerError::NotARepo(_))
    }
}

/// Shorthand `Result` type for git-tracker operations.
pub type Result<T> = std::result::Result<T, TrackerError>;

// ── Trait helpers ─────────────────────────────────────────────────────────────

/// Extension trait that lets any `Result<T, E>` be annotated with a context
/// string, converting the error into [`TrackerError::WithContext`].
pub trait Context<T> {
    /// Annotate the error with a static context string.
    fn context(self, ctx: &'static str) -> Result<T>;

    /// Annotate the error with a lazily-computed context string.
    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T>;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: Into<TrackerError>,
{
    fn context(self, ctx: &'static str) -> Result<T> {
        self.map_err(|e| e.into().context(ctx))
    }

    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|e| e.into().context(f()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_wrapping_preserves_inner() {
        let inner = TrackerError::WatcherInit("test".into());
        let wrapped = inner.context("outer message");
        let msg = wrapped.to_string();
        assert!(msg.contains("outer message"), "got: {msg}");
        assert!(msg.contains("test"), "got: {msg}");
    }

    #[test]
    fn io_constructor_roundtrips() {
        let e = TrackerError::io(
            "/some/path",
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        );
        assert!(e.to_string().contains("/some/path"));
    }

    #[test]
    fn is_not_a_repo_predicate() {
        let e = TrackerError::NotARepo("/tmp/foo".into());
        assert!(e.is_not_a_repo());
        let other = TrackerError::WatcherInit("x".into());
        assert!(!other.is_not_a_repo());
    }

    #[test]
    fn context_trait_on_result() {
        let res: std::result::Result<(), std::io::Error> = Err(std::io::Error::other("boom"));
        let tracked: Result<()> = res.context("doing something");
        assert!(tracked.unwrap_err().to_string().contains("doing something"));
    }
}
