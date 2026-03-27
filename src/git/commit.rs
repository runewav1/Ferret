use crate::error::FerretError;
use chrono::{DateTime, Utc};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub full_message: String,
    pub author: String,
    pub author_email: String,
    pub timestamp: DateTime<Utc>,
    pub files_changed: usize,
}

pub fn get_last_commit(repo_path: &Path) -> crate::error::Result<Option<CommitInfo>> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Ok(None);
        }
        Err(e) => return Err(FerretError::GitError(format!("Cannot get HEAD: {}", e))),
    };

    let commit = head
        .peel_to_commit()
        .map_err(|e| FerretError::GitError(format!("Cannot get commit: {}", e)))?;

    let time = commit.time();
    let timestamp = DateTime::from_timestamp(time.seconds(), 0).unwrap_or_else(Utc::now);

    let message = commit.message().unwrap_or("");
    let first_line = message.lines().next().unwrap_or("").to_string();

    let tree = commit
        .tree()
        .map_err(|e| FerretError::GitError(format!("Cannot get tree: {}", e)))?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| FerretError::GitError(format!("Cannot compute diff: {}", e)))?;
    let stats = diff
        .stats()
        .map_err(|e| FerretError::GitError(format!("Cannot get stats: {}", e)))?;

    let hash = commit.id().to_string();
    let short_hash = hash[..7].to_string();
    let author_sig = commit.author();
    let author = author_sig.name().unwrap_or("Unknown").to_string();
    let author_email = author_sig.email().unwrap_or("").to_string();
    let files_changed = stats.files_changed();

    Ok(Some(CommitInfo {
        hash,
        short_hash,
        message: first_line,
        full_message: message.to_string(),
        author,
        author_email,
        timestamp,
        files_changed,
    }))
}

pub fn get_commit_count(repo_path: &Path) -> crate::error::Result<usize> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let mut count = 0;
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| FerretError::GitError(format!("Cannot create revwalk: {}", e)))?;
    revwalk
        .push_head()
        .map_err(|e| FerretError::GitError(format!("Cannot push head: {}", e)))?;

    for _ in revwalk {
        count += 1;
    }

    Ok(count)
}
