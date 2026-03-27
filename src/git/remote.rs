use crate::error::FerretError;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    pub remote_type: RemoteType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RemoteType {
    Fetch,
    Push,
}

pub fn get_remotes(repo_path: &Path) -> crate::error::Result<Vec<RemoteInfo>> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| FerretError::GitError(format!("Cannot open repo: {}", e)))?;

    let remotes = repo
        .remotes()
        .map_err(|e| FerretError::GitError(format!("Cannot list remotes: {}", e)))?;

    let mut result = Vec::new();

    for remote_name in remotes.iter().flatten() {
        if let Ok(remote) = repo.find_remote(remote_name) {
            if let Some(url) = remote.url() {
                result.push(RemoteInfo {
                    name: remote_name.to_string(),
                    url: url.to_string(),
                    remote_type: RemoteType::Fetch,
                });
            }
            if let Some(push_url) = remote.pushurl() {
                if push_url != remote.url().unwrap_or("") {
                    result.push(RemoteInfo {
                        name: remote_name.to_string(),
                        url: push_url.to_string(),
                        remote_type: RemoteType::Push,
                    });
                }
            }
        }
    }

    Ok(result)
}

pub fn get_origin_url(repo_path: &Path) -> crate::error::Result<Option<String>> {
    let remotes = get_remotes(repo_path)?;
    Ok(remotes
        .into_iter()
        .find(|r| r.name == "origin" && r.remote_type == RemoteType::Fetch)
        .map(|r| r.url))
}

pub fn remote_url_to_name(url: &str) -> String {
    url.trim_end_matches(".git")
        .split('/')
        .next_back()
        .unwrap_or(url)
        .to_string()
}

pub fn is_git_repo(path: &Path) -> bool {
    git2::Repository::open(path).is_ok()
}
