use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The type of registry entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntryType {
    /// Local repository only (no remote linked)
    Local,
    /// Remote repository only (no local clone)
    LoneRemote,
    /// Both local and remote linked
    Linked,
}

/// The worktree classification for a local repository, mirroring
/// `git_tracker::worktree::WorktreeKind` but kept as a plain enum so that
/// Ferret's registry does not hard-depend on git-tracker types at the storage
/// layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeKind {
    /// Primary worktree — `.git` is a directory.
    Main,
    /// Linked worktree created with `git worktree add`.
    /// The inner string is the worktree name git uses internally.
    Linked(String),
    /// Bare repository (no working tree).
    Bare,
}

impl WorktreeKind {
    /// Human-readable label used in CLI output.
    pub fn label(&self) -> &str {
        match self {
            WorktreeKind::Main       => "main",
            WorktreeKind::Linked(_)  => "linked-worktree",
            WorktreeKind::Bare       => "bare",
        }
    }

    /// Returns `true` if this is a linked worktree.
    pub fn is_linked(&self) -> bool {
        matches!(self, WorktreeKind::Linked(_))
    }
}

impl std::fmt::Display for WorktreeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktreeKind::Main           => write!(f, "main"),
            WorktreeKind::Linked(name)   => write!(f, "linked({})", name),
            WorktreeKind::Bare           => write!(f, "bare"),
        }
    }
}

/// A registered repository entry in the Ferret registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Unique identifier for this entry
    pub id: String,

    /// Display name (defaults to folder name or remote name)
    pub name: String,

    /// Local filesystem path (None for LoneRemote entries)
    pub local_path: Option<PathBuf>,

    /// Remote git URL (None for Local-only entries)
    pub remote_url: Option<String>,

    /// Friendly name for the remote (if applicable)
    pub remote_name: Option<String>,

    /// Type of entry
    pub entry_type: EntryType,

    /// When this entry was created
    pub created_at: DateTime<Utc>,

    /// When this entry was last accessed via Ferret
    pub last_accessed: DateTime<Utc>,

    /// When the local files were last changed (filesystem mtime)
    pub last_changed: Option<DateTime<Utc>>,

    /// When the last commit was made (local)
    pub last_commit_time: Option<DateTime<Utc>>,

    /// Detected programming languages
    pub languages: Vec<String>,

    /// When language detection was last run
    pub language_cache_time: Option<DateTime<Utc>>,

    /// Last known commit message (cached)
    pub last_commit_message: Option<String>,

    // ── git-tracker fields ────────────────────────────────────────────────────
    // All fields below are `#[serde(default)]` so existing registry.json files
    // that pre-date these fields continue to deserialise without error.

    /// The branch currently checked out in the local working tree.
    ///
    /// `None` for lone-remote entries, bare repos, or when the HEAD is
    /// detached (in which case `head_detached` is `true`).
    #[serde(default)]
    pub current_branch: Option<String>,

    /// `true` when the repository's HEAD is in detached state.
    #[serde(default)]
    pub head_detached: bool,

    /// The configured upstream / tracking branch (e.g. `"origin/main"`).
    ///
    /// `None` when there is no tracking branch configured, or for non-local
    /// entries.
    #[serde(default)]
    pub upstream_branch: Option<String>,

    /// How many local commits are ahead of the upstream branch.
    /// `0` when there is no upstream, or when in sync.
    #[serde(default)]
    pub ahead: u32,

    /// How many commits the upstream is ahead of the local branch (i.e. how
    /// many commits the local branch needs to pull).
    /// `0` when there is no upstream, or when in sync.
    #[serde(default)]
    pub behind: u32,

    /// Content-stable fingerprint hash (64-char BLAKE3 hex) derived by
    /// git-tracker from the repository's root commit (or HEAD commit / a
    /// synthetic hash for empty repos).
    ///
    /// Used to detect repository moves: if the path changes but the hash
    /// stays the same, it is the same repository.
    #[serde(default)]
    pub fingerprint_hash: Option<String>,

    /// The kind of git worktree this entry represents.
    ///
    /// `None` for lone-remote entries or when worktree resolution has not
    /// been performed yet.
    #[serde(default)]
    pub worktree_kind: Option<WorktreeKind>,

    /// For linked worktrees, the absolute path to the main repository's
    /// working-tree root.
    ///
    /// `None` for main / bare repos and lone-remote entries.
    #[serde(default)]
    pub main_repo_path: Option<PathBuf>,
}

impl RegistryEntry {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new local-only entry
    pub fn new_local(id: String, name: String, path: PathBuf) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            local_path: Some(path),
            remote_url: None,
            remote_name: None,
            entry_type: EntryType::Local,
            created_at: now,
            last_accessed: now,
            last_changed: None,
            last_commit_time: None,
            languages: Vec::new(),
            language_cache_time: None,
            last_commit_message: None,
            // tracker fields
            current_branch: None,
            head_detached: false,
            upstream_branch: None,
            ahead: 0,
            behind: 0,
            fingerprint_hash: None,
            worktree_kind: None,
            main_repo_path: None,
        }
    }

    /// Create a new linked entry (local + remote)
    pub fn new_linked(
        id: String,
        name: String,
        path: PathBuf,
        remote_url: String,
        remote_name: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            local_path: Some(path),
            remote_url: Some(remote_url),
            remote_name,
            entry_type: EntryType::Linked,
            created_at: now,
            last_accessed: now,
            last_changed: None,
            last_commit_time: None,
            languages: Vec::new(),
            language_cache_time: None,
            last_commit_message: None,
            // tracker fields
            current_branch: None,
            head_detached: false,
            upstream_branch: None,
            ahead: 0,
            behind: 0,
            fingerprint_hash: None,
            worktree_kind: None,
            main_repo_path: None,
        }
    }

    /// Create a new lone remote entry
    pub fn new_lone_remote(
        id: String,
        name: String,
        remote_url: String,
        remote_name: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            local_path: None,
            remote_url: Some(remote_url),
            remote_name,
            entry_type: EntryType::LoneRemote,
            created_at: now,
            last_accessed: now,
            last_changed: None,
            last_commit_time: None,
            languages: Vec::new(),
            language_cache_time: None,
            last_commit_message: None,
            // tracker fields — not applicable for lone remotes
            current_branch: None,
            head_detached: false,
            upstream_branch: None,
            ahead: 0,
            behind: 0,
            fingerprint_hash: None,
            worktree_kind: None,
            main_repo_path: None,
        }
    }

    // ── Tracker-field helpers ─────────────────────────────────────────────────

    /// Apply all git-tracker derived fields from a `git_tracker::BranchInfo`
    /// and `git_tracker::worktree::WorktreeInfo`.
    ///
    /// This is called by `RegistryManager` after successfully opening the
    /// repository with git-tracker, so the caller handles the `Option`
    /// unwrapping / error silencing.
    pub fn apply_tracker_info(
        &mut self,
        branch:   &git_tracker::BranchInfo,
        worktree: &git_tracker::WorktreeInfo,
        fp_hash:  Option<String>,
    ) {
        // Branch / HEAD
        self.head_detached   = branch.is_detached;
        self.current_branch  = if branch.is_detached {
            None
        } else {
            Some(branch.name.clone())
        };
        self.upstream_branch = branch.upstream.clone();
        self.ahead           = branch.ahead;
        self.behind          = branch.behind;

        // Fingerprint
        self.fingerprint_hash = fp_hash;

        // Worktree kind
        use git_tracker::WorktreeKind as TK;
        self.worktree_kind = Some(match &worktree.kind {
            TK::Main           => WorktreeKind::Main,
            TK::Linked { name } => WorktreeKind::Linked(name.clone()),
            TK::Bare           => WorktreeKind::Bare,
        });

        // Main-repo path (only meaningful for linked worktrees)
        self.main_repo_path = if worktree.kind.is_linked() {
            Some(worktree.main_repo_workdir.clone())
        } else {
            None
        };
    }

    /// Refresh only the branch-related fields from a `git_tracker::BranchInfo`.
    /// Used by `RegistryManager::refresh_branch()`.
    pub fn apply_branch_info(&mut self, branch: &git_tracker::BranchInfo) {
        self.head_detached   = branch.is_detached;
        self.current_branch  = if branch.is_detached {
            None
        } else {
            Some(branch.name.clone())
        };
        self.upstream_branch = branch.upstream.clone();
        self.ahead           = branch.ahead;
        self.behind          = branch.behind;
    }

    /// Return a short human-readable branch label for display purposes.
    ///
    /// - Named branch   → `"main"`, `"feat/foo"`, …
    /// - Detached HEAD  → `"(detached)"`
    /// - No info yet    → `"—"`
    pub fn branch_label(&self) -> &str {
        if self.head_detached {
            return "(detached)";
        }
        self.current_branch.as_deref().unwrap_or("—")
    }

    /// Return a one-line divergence hint, e.g. `"↑2 ↓1"`, `"↑3"`, `"↓5"`,
    /// `""` (empty string when in sync or no upstream).
    pub fn divergence_hint(&self) -> String {
        match (self.ahead, self.behind) {
            (0, 0) => String::new(),
            (a, 0) => format!("↑{}", a),
            (0, b) => format!("↓{}", b),
            (a, b) => format!("↑{} ↓{}", a, b),
        }
    }

    // ── Existing helpers (unchanged) ──────────────────────────────────────────

    /// Update the last accessed time to now
    pub fn touch_access(&mut self) {
        self.last_accessed = Utc::now();
    }

    /// Link a remote to this local entry
    pub fn link_remote(&mut self, remote_url: String, remote_name: Option<String>) {
        self.remote_url  = Some(remote_url);
        self.remote_name = remote_name;
        self.entry_type  = EntryType::Linked;
    }

    /// Unlink the remote (convert back to Local)
    pub fn unlink_remote(&mut self) {
        self.remote_url  = None;
        self.remote_name = None;
        self.entry_type  = EntryType::Local;
    }

    /// Get the display name for the remote (friendly name or URL)
    pub fn remote_display_name(&self) -> Option<&str> {
        self.remote_name.as_deref().or(self.remote_url.as_deref())
    }

    /// Check if this entry matches a given language
    pub fn has_language(&self, lang: &str) -> bool {
        self.languages.iter().any(|l| l.eq_ignore_ascii_case(lang))
    }

    /// Check if this entry matches any of the given languages
    pub fn has_any_language(&self, langs: &[String]) -> bool {
        langs.iter().any(|lang| self.has_language(lang))
    }

    /// Check if this entry matches all of the given languages
    pub fn has_all_languages(&self, langs: &[String]) -> bool {
        langs.iter().all(|lang| self.has_language(lang))
    }
}
