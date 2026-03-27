use crate::error::FerretError;

/// Open a URL in the default browser
pub fn open_url(url: &str) -> crate::error::Result<()> {
    if url.is_empty() {
        return Err(FerretError::RemoteError("URL is empty".to_string()));
    }

    open::that(url).map_err(|e| {
        FerretError::IoError(std::io::Error::other(
            format!("Failed to open URL '{}': {}", url, e),
        ))
    })?;

    Ok(())
}

/// Open a remote repository in the browser
pub fn open_remote(remote_url: &str) -> crate::error::Result<()> {
    let url = if remote_url.starts_with("git@") {
        ssh_to_https(remote_url)
    } else if remote_url.starts_with("ssh://") {
        remote_url.replace("ssh://git@", "https://")
    } else {
        remote_url.to_string()
    };

    open_url(&url)
}

/// Convert an SSH git URL to HTTPS
fn ssh_to_https(ssh_url: &str) -> String {
    if ssh_url.starts_with("git@") {
        let without_prefix = ssh_url.trim_start_matches("git@");
        let without_suffix = without_prefix.trim_end_matches(".git");
        format!("https://{}", without_suffix.replace(':', "/"))
    } else {
        ssh_url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_to_https() {
        assert_eq!(
            ssh_to_https("git@github.com:user/repo.git"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            ssh_to_https("git@gitlab.com:org/project"),
            "https://gitlab.com/org/project"
        );
    }
}
