use std::path::Path;

use crate::error::FerretError;

/// Status of a git repository
#[derive(Debug, Clone)]
pub struct RepoStatus {
    /// Number of modified files (unstaged)
    pub modified: usize,
    /// Number of added/staged files
    pub staged: usize,
    /// Number of untracked files
    pub untracked: usize,
    /// Number of deleted files
    pub deleted: usize,
    /// Number of renamed files
    pub renamed: usize,
    /// Whether the working directory is clean
    pub is_clean: bool,
    /// List of file statuses
    pub files: Vec<FileStatus>,
}

/// Status of a single file
#[derive(Debug, Clone)]
pub struct FileStatus {
    /// File path relative to repo root
    pub path: String,
    /// Status type
    pub status_type: FileStatusType,
    /// Whether this change is staged
    pub staged: bool,
}

/// Type of file status
#[derive(Debug, Clone, PartialEq)]
pub enum FileStatusType {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Ignored,
}

/// Get the status of a git repository
pub fn get_repo_status(repo_path: &Path) -> crate::error::Result<RepoStatus> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(false)
        .recurse_untracked_dirs(true);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| FerretError::GitError(format!("Cannot get status: {}", e)))?;

    let mut modified = 0;
    let mut staged = 0;
    let mut untracked = 0;
    let mut deleted = 0;
    let mut renamed = 0;
    let mut files = Vec::new();

    for entry in statuses.iter() {
        let status = entry.status();
        let path = entry.path().unwrap_or("unknown").to_string();

        // Check staged changes
        if status.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ) {
            staged += 1;
            let status_type = if status.contains(git2::Status::INDEX_NEW) {
                FileStatusType::Added
            } else if status.contains(git2::Status::INDEX_DELETED) {
                FileStatusType::Deleted
            } else if status.contains(git2::Status::INDEX_RENAMED) {
                FileStatusType::Renamed
            } else {
                FileStatusType::Modified
            };
            files.push(FileStatus {
                path: path.clone(),
                status_type,
                staged: true,
            });
        }

        // Check working tree changes
        if status.contains(git2::Status::WT_MODIFIED) {
            modified += 1;
            files.push(FileStatus {
                path: path.clone(),
                status_type: FileStatusType::Modified,
                staged: false,
            });
        }
        if status.contains(git2::Status::WT_DELETED) {
            deleted += 1;
            files.push(FileStatus {
                path: path.clone(),
                status_type: FileStatusType::Deleted,
                staged: false,
            });
        }
        if status.contains(git2::Status::WT_RENAMED) {
            renamed += 1;
            files.push(FileStatus {
                path: path.clone(),
                status_type: FileStatusType::Renamed,
                staged: false,
            });
        }
        if status.contains(git2::Status::WT_TYPECHANGE) {
            files.push(FileStatus {
                path: path.clone(),
                status_type: FileStatusType::Modified,
                staged: false,
            });
        }

        // Check untracked
        if status.contains(git2::Status::WT_NEW) {
            untracked += 1;
            files.push(FileStatus {
                path: path.clone(),
                status_type: FileStatusType::Untracked,
                staged: false,
            });
        }
    }

    let is_clean = modified == 0 && staged == 0 && untracked == 0 && deleted == 0;

    Ok(RepoStatus {
        modified,
        staged,
        untracked,
        deleted,
        renamed,
        is_clean,
        files,
    })
}

/// Format repo status as a human-readable string
pub fn format_status(status: &RepoStatus) -> String {
    if status.is_clean {
        return "Working tree clean".to_string();
    }

    let mut parts = Vec::new();
    if status.staged > 0 {
        parts.push(format!("{} staged", status.staged));
    }
    if status.modified > 0 {
        parts.push(format!("{} modified", status.modified));
    }
    if status.deleted > 0 {
        parts.push(format!("{} deleted", status.deleted));
    }
    if status.untracked > 0 {
        parts.push(format!("{} untracked", status.untracked));
    }

    parts.join(", ")
}
