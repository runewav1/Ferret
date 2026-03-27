//! Worktree detection and classification.
//!
//! Git supports *linked worktrees* – additional checked-out working trees
//! attached to a single repository. From the outside they are nearly
//! indistinguishable from full repositories: both have a `.git` entry at the
//! root. The difference is:
//!
//! * A **main worktree** has `.git` as a *directory*.
//! * A **linked worktree** has `.git` as a *file* whose content is a `gitdir:`
//!   pointer to a subdirectory inside the main repo's `.git/worktrees/<name>/`
//!   directory.
//! * A **bare repository** has no working tree at all; its "workdir" *is* the
//!   `.git` directory.
//!
//! This module fully resolves those cases and surfaces a [`WorktreeInfo`] that
//! tells callers:
//!
//! * Which [`WorktreeKind`] a path represents.
//! * The **main repository root** (always the canonical, non-linked root).
//! * The **git-dir** for both the worktree itself and for the main repo.
//! * Every other linked worktree that belongs to the same main repo.
//!
//! ## `.git` file format
//!
//! A `.git` file produced by `git worktree add` looks like:
//!
//! ```text
//! gitdir: /absolute/path/to/main/.git/worktrees/feature-x
//! ```
//!
//! The pointed-to directory contains a `gitdir` file that in turn points back
//! to the linked worktree's `.git` file location (a round-trip pointer used by
//! git for garbage collection). We read the outbound pointer only.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};

// ── WorktreeKind ──────────────────────────────────────────────────────────────

/// Classifies a discovered git location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeKind {
    /// This is the primary worktree. Its `.git` entry is a **directory**.
    /// All linked worktrees are subordinate to this one.
    Main,

    /// This is a linked worktree created with `git worktree add`.
    /// Its `.git` entry is a **file** pointing into the main repo.
    Linked {
        /// The name git uses internally for this worktree (the directory name
        /// under `<main>/.git/worktrees/<name>`).
        name: String,
    },

    /// A bare repository – no working tree. The repository directory itself
    /// serves as both the git-dir and the "workdir".
    Bare,
}

impl WorktreeKind {
    /// Returns `true` if this is the main (non-linked) worktree.
    pub fn is_main(&self) -> bool {
        matches!(self, WorktreeKind::Main)
    }

    /// Returns `true` if this is a linked worktree.
    pub fn is_linked(&self) -> bool {
        matches!(self, WorktreeKind::Linked { .. })
    }

    /// Returns `true` if this is a bare repository.
    pub fn is_bare(&self) -> bool {
        matches!(self, WorktreeKind::Bare)
    }

    /// Returns the worktree name if this is a [`WorktreeKind::Linked`] variant.
    pub fn linked_name(&self) -> Option<&str> {
        match self {
            WorktreeKind::Linked { name } => Some(name.as_str()),
            _ => None,
        }
    }
}

impl std::fmt::Display for WorktreeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktreeKind::Main => f.write_str("main"),
            WorktreeKind::Linked { name } => write!(f, "linked({name})"),
            WorktreeKind::Bare => f.write_str("bare"),
        }
    }
}

// ── LinkedWorktreeEntry ───────────────────────────────────────────────────────

/// A single linked worktree entry as seen from the main repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedWorktreeEntry {
    /// The internal name git uses (the directory name under
    /// `<main-git-dir>/worktrees/<name>`).
    pub name: String,

    /// Absolute path to the git-dir for this linked worktree
    /// (`<main-git-dir>/worktrees/<name>`).
    pub linked_git_dir: PathBuf,

    /// The working tree root for this linked worktree, as recorded in the
    /// `gitdir` back-pointer file (may not exist if the worktree was deleted
    /// without being pruned).
    pub worktree_path: Option<PathBuf>,

    /// Whether the working-tree path actually exists on disk right now.
    pub exists_on_disk: bool,
}

// ── WorktreeInfo ──────────────────────────────────────────────────────────────

/// Complete worktree information for a single path.
///
/// Obtain one via [`WorktreeResolver::resolve`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// The path that was resolved (normalised, canonical).
    pub resolved_path: PathBuf,

    /// Classification of this location.
    pub kind: WorktreeKind,

    /// The `.git` directory (or file) path for *this* worktree.
    pub git_dir: PathBuf,

    /// The working-tree root for *this* worktree.
    /// For bare repositories this equals `git_dir`.
    pub workdir: PathBuf,

    /// The **main** repository's working-tree root.
    ///
    /// * For [`WorktreeKind::Main`] and [`WorktreeKind::Bare`] this equals
    ///   `workdir`.
    /// * For [`WorktreeKind::Linked`] this is the root of the owning repo.
    pub main_repo_workdir: PathBuf,

    /// The **main** repository's git-dir (`<main>/.git`).
    ///
    /// For [`WorktreeKind::Main`] and [`WorktreeKind::Bare`] this equals
    /// `git_dir`.
    pub main_git_dir: PathBuf,

    /// All *other* linked worktrees that belong to the same main repository,
    /// discovered by inspecting `<main-git-dir>/worktrees/`.
    pub linked_worktrees: Vec<LinkedWorktreeEntry>,
}

impl WorktreeInfo {
    /// Returns `true` when this path *is* the main repository root (not a
    /// linked worktree and not bare).
    pub fn is_main_repo(&self) -> bool {
        self.kind.is_main()
    }

    /// Returns `true` when this path is a linked worktree of another repo.
    pub fn is_linked_worktree(&self) -> bool {
        self.kind.is_linked()
    }

    /// Returns `true` when this is a bare repository.
    pub fn is_bare(&self) -> bool {
        self.kind.is_bare()
    }

    /// How many linked worktrees does the main repository own right now?
    pub fn linked_worktree_count(&self) -> usize {
        self.linked_worktrees.len()
    }

    /// Returns the linked worktree entry for this path if it is itself a linked
    /// worktree, otherwise `None`.
    pub fn self_as_linked_entry(&self) -> Option<&LinkedWorktreeEntry> {
        let own_name = self.kind.linked_name()?;
        self.linked_worktrees
            .iter()
            .find(|e| e.name == own_name)
    }
}

// ── WorktreeResolver ──────────────────────────────────────────────────────────

/// Resolves worktree information for arbitrary paths.
///
/// Stateless – create and use immediately, or keep a shared instance.
///
/// ```no_run
/// use git_tracker::worktree::WorktreeResolver;
/// use std::path::Path;
///
/// let info = WorktreeResolver::new()
///     .resolve(Path::new("/home/user/projects/my-repo"))
///     .expect("not a git repo");
///
/// println!("kind: {}", info.kind);
/// println!("main repo: {}", info.main_repo_workdir.display());
/// ```
#[derive(Debug, Default, Clone)]
pub struct WorktreeResolver;

impl WorktreeResolver {
    /// Create a new resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve worktree information for `path`.
    ///
    /// `path` may be the working-tree root, the `.git` directory, or any
    /// subdirectory – libgit2 will walk upward to find the repository root.
    ///
    /// # Errors
    ///
    /// * [`TrackerError::NotARepo`] – `path` is not inside a git repository.
    /// * [`TrackerError::BrokenWorktreeLink`] – a `.git` file points to a
    ///   location that does not exist.
    /// * [`TrackerError::WorktreeMainRepoUnresolvable`] – the main repo cannot
    ///   be determined from a linked worktree's git-dir pointer.
    pub fn resolve(&self, path: &Path) -> Result<WorktreeInfo> {
        // Canonicalise early so that all subsequent comparisons are reliable.
        let canonical = canonicalize_best_effort(path);

        // Open with libgit2 to get the authoritative git-dir and workdir.
        let repo = git2::Repository::discover(&canonical)
            .map_err(|_| TrackerError::NotARepo(canonical.clone()))?;

        let git_dir  = repo.path().to_path_buf();
        let workdir  = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| git_dir.clone());
        let is_bare  = repo.is_bare();

        if is_bare {
            return Ok(WorktreeInfo {
                resolved_path:    canonical.clone(),
                kind:             WorktreeKind::Bare,
                git_dir:          git_dir.clone(),
                workdir:          workdir.clone(),
                main_repo_workdir: workdir.clone(),
                main_git_dir:     git_dir.clone(),
                linked_worktrees: Vec::new(),
            });
        }

        // Determine whether the workdir's .git entry is a file or a directory.
        let dot_git = workdir.join(".git");

        if dot_git.is_file() {
            // ── Linked worktree ───────────────────────────────────────────────
            self.resolve_linked(canonical, git_dir, workdir, &dot_git)
        } else {
            // ── Main worktree ─────────────────────────────────────────────────
            // git_dir IS the .git directory itself.
            let linked = discover_linked_worktrees(&git_dir);
            Ok(WorktreeInfo {
                resolved_path:    canonical,
                kind:             WorktreeKind::Main,
                git_dir:          git_dir.clone(),
                workdir:          workdir.clone(),
                main_repo_workdir: workdir,
                main_git_dir:     git_dir,
                linked_worktrees: linked,
            })
        }
    }

    // ── Linked-worktree resolution ────────────────────────────────────────────

    fn resolve_linked(
        &self,
        canonical:  PathBuf,
        git_dir:    PathBuf,   // e.g. /main/.git/worktrees/feature/
        workdir:    PathBuf,   // e.g. /linked/feature/
        dot_git:    &Path,     // e.g. /linked/feature/.git  (the file)
    ) -> Result<WorktreeInfo> {
        // The git-dir for a linked worktree is something like:
        //   /main-repo/.git/worktrees/<name>
        // We need to climb two levels to get to the main .git dir.
        let worktrees_dir = git_dir
            .parent()
            .ok_or_else(|| TrackerError::WorktreeMainRepoUnresolvable(workdir.clone()))?;

        let main_git_dir = worktrees_dir
            .parent()
            .ok_or_else(|| TrackerError::WorktreeMainRepoUnresolvable(workdir.clone()))?
            .to_path_buf();

        // The name is the leaf of the git_dir path
        // (e.g. "feature" from `.git/worktrees/feature`).
        let name = git_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // The main working tree is the parent of the main .git dir.
        let main_repo_workdir = main_git_dir
            .parent()
            .ok_or_else(|| TrackerError::WorktreeMainRepoUnresolvable(workdir.clone()))?
            .to_path_buf();

        // Verify the link target actually exists.
        if !main_git_dir.exists() {
            return Err(TrackerError::broken_worktree(dot_git, &main_git_dir));
        }

        // Enumerate all linked worktrees from the main git dir.
        let linked = discover_linked_worktrees(&main_git_dir);

        Ok(WorktreeInfo {
            resolved_path:    canonical,
            kind:             WorktreeKind::Linked { name },
            git_dir,
            workdir,
            main_repo_workdir,
            main_git_dir,
            linked_worktrees: linked,
        })
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Read the `gitdir:` pointer out of a `.git` *file*.
///
/// Returns the target path (not necessarily existent) or an error if the
/// file cannot be read or does not start with `gitdir:`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn read_gitdir_file(dot_git_file: &Path) -> Result<PathBuf> {
    let content = fs::read_to_string(dot_git_file).map_err(|e| {
        TrackerError::io(dot_git_file, e)
    })?;

    let trimmed = content.trim();
    let suffix = trimmed.strip_prefix("gitdir:").ok_or_else(|| {
        TrackerError::BrokenWorktreeLink {
            link:   dot_git_file.to_path_buf(),
            target: PathBuf::from("<invalid gitdir file>"),
        }
    })?;

    let target = PathBuf::from(suffix.trim());

    // If the path is relative, resolve it relative to the .git file's parent.
    let resolved = if target.is_absolute() {
        target
    } else {
        dot_git_file
            .parent()
            .unwrap_or(Path::new("."))
            .join(&target)
    };

    Ok(resolved)
}

/// Enumerate every entry in `<git-dir>/worktrees/` and build a
/// [`LinkedWorktreeEntry`] for each one found.
///
/// Silently skips unreadable entries (a partial list is better than an error).
fn discover_linked_worktrees(main_git_dir: &Path) -> Vec<LinkedWorktreeEntry> {
    let worktrees_dir = main_git_dir.join("worktrees");

    let read_dir = match fs::read_dir(&worktrees_dir) {
        Ok(rd) => rd,
        // No `worktrees/` directory at all – perfectly normal for repos without
        // any linked worktrees.
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();

    for entry in read_dir.flatten() {
        let linked_git_dir = entry.path();
        if !linked_git_dir.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();

        // The `gitdir` file inside each worktrees/<name>/ directory points
        // *back* to the linked worktree's .git file.  We can derive the
        // worktree path from it.
        let worktree_path = read_worktree_path_from_gitdir_back_pointer(&linked_git_dir);
        let exists_on_disk = worktree_path
            .as_deref()
            .map(|p| p.exists())
            .unwrap_or(false);

        entries.push(LinkedWorktreeEntry {
            name,
            linked_git_dir,
            worktree_path,
            exists_on_disk,
        });
    }

    // Sort for deterministic output.
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Attempt to read the path of the linked worktree from the back-pointer file
/// `<worktrees/<name>/gitdir>`.
///
/// The file contains the absolute path of the `.git` **file** inside the
/// linked worktree directory. Stripping the `.git` suffix gives the
/// worktree root.
fn read_worktree_path_from_gitdir_back_pointer(linked_git_dir: &Path) -> Option<PathBuf> {
    let gitdir_file = linked_git_dir.join("gitdir");
    let content = fs::read_to_string(&gitdir_file).ok()?;
    let dot_git_path = PathBuf::from(content.trim());

    // dot_git_path is the .git FILE inside the linked worktree – strip it to
    // get the worktree root.
    dot_git_path.parent().map(|p| p.to_path_buf())
}

/// Attempt to canonicalize `path`; fall back to the original if it fails
/// (e.g. if the path does not exist yet – rare but possible during move detection).
fn canonicalize_best_effort(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    // ── test helpers ──

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

    fn git_config(dir: &Path) {
        git(dir, &["config", "user.email", "test@test.com"]);
        git(dir, &["config", "user.name", "Test"]);
    }

    /// Init a repo and make one commit so that worktrees can be added.
    fn make_committed_repo(tmp: &TempDir, name: &str) -> PathBuf {
        let path = tmp.path().join(name);
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init"]);
        git_config(&path);
        fs::write(path.join("a.txt"), b"hello").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "init"]);
        path
    }

    // ── WorktreeKind predicates ──

    #[test]
    fn kind_main_predicates() {
        let k = WorktreeKind::Main;
        assert!(k.is_main());
        assert!(!k.is_linked());
        assert!(!k.is_bare());
        assert!(k.linked_name().is_none());
    }

    #[test]
    fn kind_linked_predicates() {
        let k = WorktreeKind::Linked { name: "feat".into() };
        assert!(!k.is_main());
        assert!(k.is_linked());
        assert!(!k.is_bare());
        assert_eq!(k.linked_name(), Some("feat"));
    }

    #[test]
    fn kind_bare_predicates() {
        let k = WorktreeKind::Bare;
        assert!(!k.is_main());
        assert!(!k.is_linked());
        assert!(k.is_bare());
    }

    #[test]
    fn kind_display() {
        assert_eq!(WorktreeKind::Main.to_string(), "main");
        assert_eq!(
            WorktreeKind::Linked { name: "feat".into() }.to_string(),
            "linked(feat)"
        );
        assert_eq!(WorktreeKind::Bare.to_string(), "bare");
    }

    // ── Main worktree resolution ──

    #[test]
    fn resolves_main_worktree() {
        let tmp  = TempDir::new().unwrap();
        let path = make_committed_repo(&tmp, "main-repo");

        let resolver = WorktreeResolver::new();
        let info     = resolver.resolve(&path).expect("resolve failed");

        assert!(info.kind.is_main(),  "expected Main, got {:?}", info.kind);
        assert!(!info.kind.is_bare());
        assert_eq!(info.workdir,            info.main_repo_workdir);
        assert_eq!(info.git_dir,            info.main_git_dir);
        assert_eq!(info.linked_worktrees.len(), 0, "freshly-made repo should have no linked worktrees");
    }

    #[test]
    fn resolves_from_subdirectory() {
        let tmp  = TempDir::new().unwrap();
        let path = make_committed_repo(&tmp, "main-repo");

        // Create a subdirectory inside the repo.
        let sub = path.join("src");
        fs::create_dir_all(&sub).unwrap();

        let resolver = WorktreeResolver::new();
        let info = resolver.resolve(&sub).expect("resolve from subdir failed");

        assert!(info.kind.is_main());
        // workdir should still be the repo root, not the subdir
        assert_eq!(
            info.workdir.canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
    }

    // ── Linked worktree resolution ──

    #[test]
    fn resolves_linked_worktree() {
        let tmp   = TempDir::new().unwrap();
        let main  = make_committed_repo(&tmp, "main-repo");
        let linked = tmp.path().join("linked-feature");

        // Add a linked worktree.
        let ok = git(
            &main,
            &[
                "worktree", "add",
                linked.to_str().unwrap(),
                "-b", "feature-branch",
            ],
        );
        if !ok {
            // Some CI environments may not support worktrees; skip gracefully.
            eprintln!("git worktree add failed – skipping linked worktree test");
            return;
        }

        let resolver = WorktreeResolver::new();
        let info     = resolver.resolve(&linked).expect("resolve linked worktree failed");

        assert!(info.kind.is_linked(), "expected Linked, got {:?}", info.kind);

        // The main repo workdir should point to the original repo.
        assert_eq!(
            info.main_repo_workdir.canonicalize().unwrap(),
            main.canonicalize().unwrap(),
            "main_repo_workdir must point to the main repo root"
        );

        // The linked worktrees list should contain exactly one entry.
        assert_eq!(
            info.linked_worktrees.len(), 1,
            "main repo should advertise one linked worktree"
        );
        assert!(
            info.linked_worktrees[0].exists_on_disk,
            "linked worktree should exist on disk"
        );
    }

    #[test]
    fn linked_and_main_agree_on_main_git_dir() {
        let tmp   = TempDir::new().unwrap();
        let main  = make_committed_repo(&tmp, "main-repo");
        let linked = tmp.path().join("linked-wt");

        let ok = git(
            &main,
            &["worktree", "add", linked.to_str().unwrap(), "-b", "wt-branch"],
        );
        if !ok {
            eprintln!("git worktree add not supported – skipping");
            return;
        }

        let resolver     = WorktreeResolver::new();
        let main_info    = resolver.resolve(&main).unwrap();
        let linked_info  = resolver.resolve(&linked).unwrap();

        assert_eq!(
            main_info.main_git_dir.canonicalize().unwrap(),
            linked_info.main_git_dir.canonicalize().unwrap(),
            "both resolutions must agree on the main .git directory"
        );
    }

    #[test]
    fn main_info_sees_linked_worktree() {
        let tmp   = TempDir::new().unwrap();
        let main  = make_committed_repo(&tmp, "main-repo");
        let linked = tmp.path().join("linked-view");

        let ok = git(
            &main,
            &["worktree", "add", linked.to_str().unwrap(), "-b", "view-branch"],
        );
        if !ok {
            eprintln!("git worktree add not supported – skipping");
            return;
        }

        let resolver  = WorktreeResolver::new();
        let main_info = resolver.resolve(&main).unwrap();

        assert_eq!(main_info.linked_worktrees.len(), 1);
        assert!(main_info.linked_worktrees[0].exists_on_disk);
    }

    // ── Bare repository ──

    #[test]
    fn resolves_bare_repository() {
        let tmp = TempDir::new().unwrap();
        let bare_path = tmp.path().join("bare.git");
        fs::create_dir_all(&bare_path).unwrap();
        git(&bare_path, &["init", "--bare"]);

        let resolver = WorktreeResolver::new();
        let info = resolver.resolve(&bare_path).expect("resolve bare failed");

        assert!(info.kind.is_bare(), "expected Bare, got {:?}", info.kind);
        assert_eq!(info.linked_worktrees.len(), 0);
    }

    // ── Error cases ──

    #[test]
    fn error_on_non_repo_path() {
        let tmp      = TempDir::new().unwrap();
        let resolver = WorktreeResolver::new();
        let result   = resolver.resolve(tmp.path());

        assert!(
            matches!(result, Err(TrackerError::NotARepo(_))),
            "expected NotARepo, got {:?}",
            result
        );
    }

    // ── gitdir file reader ──

    #[test]
    fn read_gitdir_file_absolute() {
        let tmp  = TempDir::new().unwrap();
        let file = tmp.path().join(".git");

        // Use a path that is absolute on the current platform.
        #[cfg(windows)]
        let abs_target = "C:/abs/path/to/.git/worktrees/feat";
        #[cfg(not(windows))]
        let abs_target = "/abs/path/to/.git/worktrees/feat";

        fs::write(&file, format!("gitdir: {abs_target}\n")).unwrap();
        let result = read_gitdir_file(&file).unwrap();
        assert_eq!(result, PathBuf::from(abs_target));
    }

    #[test]
    fn read_gitdir_file_relative() {
        let tmp  = TempDir::new().unwrap();
        let file = tmp.path().join(".git");
        // Relative path – should be resolved against the file's parent.
        fs::write(&file, "gitdir: ../.git/worktrees/feat\n").unwrap();
        let result = read_gitdir_file(&file).unwrap();
        // Should not be an absolute path rooted elsewhere
        assert!(result.ends_with(".git/worktrees/feat"));
    }

    #[test]
    fn read_gitdir_file_invalid_prefix_errors() {
        let tmp  = TempDir::new().unwrap();
        let file = tmp.path().join(".git");
        fs::write(&file, "not a gitdir file\n").unwrap();
        let result = read_gitdir_file(&file);
        assert!(
            matches!(result, Err(TrackerError::BrokenWorktreeLink { .. })),
            "expected BrokenWorktreeLink, got {:?}",
            result
        );
    }

    // ── Serde roundtrip ──

    #[test]
    fn worktree_info_serde_roundtrip() {
        let tmp  = TempDir::new().unwrap();
        let path = make_committed_repo(&tmp, "serde-repo");
        let info = WorktreeResolver::new().resolve(&path).unwrap();
        let json = serde_json::to_string(&info).expect("serialize");
        let back: WorktreeInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(info.kind, back.kind);
        assert_eq!(info.workdir, back.workdir);
    }

    // ── WorktreeInfo helpers ──

    #[test]
    fn linked_worktree_count_zero_for_fresh_main() {
        let tmp  = TempDir::new().unwrap();
        let path = make_committed_repo(&tmp, "count-repo");
        let info = WorktreeResolver::new().resolve(&path).unwrap();
        assert_eq!(info.linked_worktree_count(), 0);
    }

    #[test]
    fn linked_worktree_count_nonzero_after_add() {
        let tmp   = TempDir::new().unwrap();
        let main  = make_committed_repo(&tmp, "count-main");
        let wt    = tmp.path().join("count-wt");

        let ok = git(&main, &["worktree", "add", wt.to_str().unwrap(), "-b", "count-b"]);
        if !ok {
            eprintln!("git worktree add not supported – skipping");
            return;
        }

        let info = WorktreeResolver::new().resolve(&main).unwrap();
        assert_eq!(info.linked_worktree_count(), 1);
    }
}
