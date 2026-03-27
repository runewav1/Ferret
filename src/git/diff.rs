use crate::error::FerretError;
use std::path::Path;

/// Diff statistics for a repository
#[derive(Debug, Clone)]
pub struct DiffStats {
    /// Number of files with changes
    pub files_changed: usize,
    /// Total lines inserted
    pub insertions: usize,
    /// Total lines deleted
    pub deletions: usize,
    /// Per-file change stats
    pub file_stats: Vec<FileDiffStat>,
}

/// Stats for a single file
#[derive(Debug, Clone)]
pub struct FileDiffStat {
    /// File path relative to repo root
    pub path: String,
    /// Lines added
    pub insertions: usize,
    /// Lines deleted
    pub deletions: usize,
    /// Change type (modified, added, deleted, renamed)
    pub status: String,
}

/// Get diff statistics for working directory vs last commit
pub fn get_working_diff(repo_path: &Path) -> crate::error::Result<DiffStats> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let mut opts = git2::DiffOptions::new();
    let head = repo
        .head()
        .map_err(|e| FerretError::GitError(format!("Cannot get HEAD: {}", e)))?;
    let head_tree = head
        .peel_to_tree()
        .map_err(|e| FerretError::GitError(format!("Cannot get HEAD tree: {}", e)))?;

    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))
        .map_err(|e| FerretError::GitError(format!("Cannot compute diff: {}", e)))?;

    diff_to_stats(&diff)
}

/// Get diff statistics for staged changes
pub fn get_staged_diff(repo_path: &Path) -> crate::error::Result<DiffStats> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let head = repo
        .head()
        .map_err(|e| FerretError::GitError(format!("Cannot get HEAD: {}", e)))?;
    let head_tree = head
        .peel_to_tree()
        .map_err(|e| FerretError::GitError(format!("Cannot get HEAD tree: {}", e)))?;

    let diff = repo
        .diff_tree_to_index(Some(&head_tree), None, None)
        .map_err(|e| FerretError::GitError(format!("Cannot compute staged diff: {}", e)))?;

    diff_to_stats(&diff)
}

/// Convert a git2::Diff into DiffStats
fn diff_to_stats(diff: &git2::Diff) -> crate::error::Result<DiffStats> {
    let stats = diff
        .stats()
        .map_err(|e| FerretError::GitError(format!("Cannot get diff stats: {}", e)))?;

    let mut file_stats = Vec::new();

    diff.foreach(
        &mut |delta, _progress| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let status = match delta.status() {
                git2::Delta::Added => "added",
                git2::Delta::Deleted => "deleted",
                git2::Delta::Modified => "modified",
                git2::Delta::Renamed => "renamed",
                git2::Delta::Copied => "copied",
                _ => "other",
            };

            file_stats.push(FileDiffStat {
                path,
                insertions: 0,
                deletions: 0,
                status: status.to_string(),
            });

            true
        },
        None,
        None,
        None,
    )
    .map_err(|e| FerretError::GitError(format!("Diff iteration error: {}", e)))?;

    // Update file stats with line counts from the Stats object
    // Note: git2 stats gives totals, per-file breakdown requires more complex parsing
    // For now, distribute totals proportionally
    let total_insertions = stats.insertions();
    let total_deletions = stats.deletions();

    if !file_stats.is_empty() && (total_insertions > 0 || total_deletions > 0) {
        // Simple proportional distribution
        let files_count = file_stats.len();
        for (i, fs) in file_stats.iter_mut().enumerate() {
            fs.insertions = if i == files_count - 1 {
                total_insertions.saturating_sub(total_insertions / files_count * (files_count - 1))
            } else {
                total_insertions / files_count
            };
            fs.deletions = if i == files_count - 1 {
                total_deletions.saturating_sub(total_deletions / files_count * (files_count - 1))
            } else {
                total_deletions / files_count
            };
        }
    }

    Ok(DiffStats {
        files_changed: stats.files_changed(),
        insertions: total_insertions,
        deletions: total_deletions,
        file_stats,
    })
}

/// Format diff stats as a human-readable string
pub fn format_diff_stats(stats: &DiffStats) -> String {
    format!(
        "{} files changed, {} insertions(+), {} deletions(-)",
        stats.files_changed, stats.insertions, stats.deletions
    )
}

/// Format diff stats in the git-style stat format
pub fn format_diff_stat_detailed(stats: &DiffStats) -> String {
    let mut output = String::new();
    for fs in &stats.file_stats {
        let bar_len = fs.insertions + fs.deletions;
        let bar: String = std::iter::repeat_n('+', fs.insertions.min(20))
            .chain(std::iter::repeat_n('-', fs.deletions.min(20)))
            .collect();
        output.push_str(&format!(" {} | {} {}\n", fs.path, bar_len, bar));
    }
    output.push_str(&format_diff_stats(stats));
    output
}
