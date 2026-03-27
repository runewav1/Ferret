use std::path::Path;

use super::entry::RegistryEntry;
use super::storage::RegistryStorage;
use crate::error::FerretError;
use crate::git;

/// Manages all registry operations
pub struct RegistryManager {
    storage: RegistryStorage,
}

impl RegistryManager {
    /// Create a new registry manager
    pub fn new() -> crate::error::Result<Self> {
        Ok(Self {
            storage: RegistryStorage::new()?,
        })
    }

    /// Add a local repository to the registry
    pub fn add_local(
        &mut self,
        path: &Path,
        name: Option<&str>,
    ) -> crate::error::Result<RegistryEntry> {
        let canonical_path = crate::pathutil::canonicalize_path(path)?;
        let folder_name =
            crate::pathutil::folder_name(&canonical_path).unwrap_or_else(|| "unknown".to_string());

        let entry_name = name.unwrap_or(&folder_name).to_string();
        let id = entry_name.clone();

        // Check for duplicate
        let mut store = self.storage.load()?;
        if store.entries.iter().any(|e| e.id.eq_ignore_ascii_case(&id)) {
            return Err(FerretError::DuplicateEntry(id));
        }

        // Try to get remote URL
        let remote_url = git::remote::get_origin_url(&canonical_path).unwrap_or(None);
        let remote_name = remote_url
            .as_ref()
            .map(|url| git::remote::remote_url_to_name(url));

        // Detect languages
        let languages = crate::language::detector::LanguageDetector::new()
            .detect_language_names(&canonical_path)
            .unwrap_or_default();

        let mut entry = if let Some(url) = remote_url {
            RegistryEntry::new_linked(
                id.clone(),
                entry_name.clone(),
                canonical_path.clone(),
                url,
                remote_name,
            )
        } else {
            RegistryEntry::new_local(id.clone(), entry_name.clone(), canonical_path.clone())
        };

        entry.languages = languages;

        // ── git-tracker enrichment ────────────────────────────────────────────
        // Populate branch info, worktree kind, and fingerprint.  Any failure
        // here is silently swallowed so that `add` never fails just because
        // git-tracker couldn't open the repo (e.g. empty / corrupt repos).
        populate_tracker_fields(&mut entry, &canonical_path);

        store.entries.push(entry.clone());
        self.storage.save(&store)?;

        Ok(entry)
    }

    /// Add a lone remote (no local clone)
    pub fn add_lone_remote(
        &mut self,
        url: &str,
        name: Option<&str>,
    ) -> crate::error::Result<RegistryEntry> {
        let remote_name = git::remote::remote_url_to_name(url);
        let entry_name = name.unwrap_or(&remote_name).to_string();
        let id = entry_name.clone();

        let mut store = self.storage.load()?;
        if store.entries.iter().any(|e| e.id.eq_ignore_ascii_case(&id)) {
            return Err(FerretError::DuplicateEntry(id));
        }

        let entry = RegistryEntry::new_lone_remote(
            id.clone(),
            entry_name,
            url.to_string(),
            Some(remote_name),
        );
        store.entries.push(entry.clone());
        self.storage.save(&store)?;

        Ok(entry)
    }

    /// Link an existing local entry to a remote
    pub fn link_to_remote(&mut self, local_id: &str, remote_ref: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;

        // Find the local entry
        let local_idx = store
            .entries
            .iter()
            .position(|e| e.id.eq_ignore_ascii_case(local_id))
            .ok_or_else(|| FerretError::NotFound(local_id.to_string()))?;

        // Try to find the remote by name or URL
        let remote_url = if let Some(remote_entry) = store.entries.iter().find(|e| {
            e.remote_name.as_deref() == Some(remote_ref)
                || e.remote_url.as_deref() == Some(remote_ref)
        }) {
            remote_entry.remote_url.clone()
        } else {
            // Assume it's a URL directly
            Some(remote_ref.to_string())
        };

        let url = remote_url.ok_or_else(|| {
            FerretError::RemoteError(format!("Cannot find remote: {}", remote_ref))
        })?;

        let rname = git::remote::remote_url_to_name(&url);
        store.entries[local_idx].link_remote(url, Some(rname));
        self.storage.save(&store)?;

        Ok(())
    }

    /// Remove an entry from the registry
    pub fn remove(&mut self, id: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        let original_len = store.entries.len();
        store.entries.retain(|e| !e.id.eq_ignore_ascii_case(id));

        if store.entries.len() == original_len {
            return Err(FerretError::NotFound(id.to_string()));
        }

        self.storage.save(&store)?;
        Ok(())
    }

    /// Unlink remote from a local entry (keep local)
    pub fn unlink_remote(&mut self, id: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        let entry = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))?;
        entry.unlink_remote();
        self.storage.save(&store)?;
        Ok(())
    }

    /// Get an entry by ID
    pub fn get(&self, id: &str) -> crate::error::Result<RegistryEntry> {
        let store = self.storage.load()?;
        store
            .entries
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))
    }

    /// Get all entries
    pub fn get_all(&self) -> crate::error::Result<Vec<RegistryEntry>> {
        let store = self.storage.load()?;
        Ok(store.entries)
    }

    /// Update an existing entry
    pub fn update(&mut self, entry: &RegistryEntry) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        if let Some(existing) = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(&entry.id))
        {
            *existing = entry.clone();
        }
        self.storage.save(&store)?;
        Ok(())
    }

    /// Touch an entry's access time
    pub fn touch_access(&mut self, id: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        let entry = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))?;
        entry.touch_access();
        self.storage.save(&store)?;
        Ok(())
    }

    // ── git-tracker helpers ───────────────────────────────────────────────────

    /// Re-read the current branch (and upstream divergence) for a single entry
    /// and persist the result.
    ///
    /// Returns `Ok(())` if the entry either doesn't have a local path or if
    /// git-tracker cannot open the repo – callers don't need to handle those
    /// cases specially.
    pub fn refresh_branch(&mut self, id: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        let entry = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))?;

        if let Some(path) = entry.local_path.clone() {
            if let Ok(branch_info) = git_tracker::get_branch_info(&path) {
                entry.apply_branch_info(&branch_info);
            }
        }

        self.storage.save(&store)?;
        Ok(())
    }

    /// Re-read branch info **and** all other tracker fields (fingerprint,
    /// worktree kind) for a single entry and persist.
    pub fn refresh_tracker(&mut self, id: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;
        let entry = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))?;

        if let Some(path) = entry.local_path.clone() {
            populate_tracker_fields(entry, &path);
        }

        self.storage.save(&store)?;
        Ok(())
    }

    /// Refresh branch info for **all** local entries in the registry.
    ///
    /// Entries that cannot be opened (missing path, corrupt repo, etc.) are
    /// silently skipped.  Returns the number of entries successfully updated.
    pub fn refresh_all_branches(&mut self) -> crate::error::Result<usize> {
        let mut store = self.storage.load()?;
        let mut updated = 0usize;

        for entry in store.entries.iter_mut() {
            if let Some(path) = &entry.local_path.clone() {
                if let Ok(branch_info) = git_tracker::get_branch_info(path) {
                    entry.apply_branch_info(&branch_info);
                    updated += 1;
                }
            }
        }

        self.storage.save(&store)?;
        Ok(updated)
    }

    /// Refresh all tracker fields for every local entry in the registry.
    ///
    /// More expensive than `refresh_all_branches` because it also re-computes
    /// fingerprints and worktree kinds.  Silently skips any entry that cannot
    /// be opened.  Returns the number of entries updated.
    pub fn refresh_all_tracker(&mut self) -> crate::error::Result<usize> {
        let mut store = self.storage.load()?;
        let mut updated = 0usize;

        for entry in store.entries.iter_mut() {
            if let Some(path) = entry.local_path.clone() {
                populate_tracker_fields(entry, &path);
                updated += 1;
            }
        }

        self.storage.save(&store)?;
        Ok(updated)
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Populate all git-tracker–derived fields on `entry` from the repository at
/// `path`.  Any per-step failure is silently ignored so callers never need to
/// handle partial results.
fn populate_tracker_fields(entry: &mut RegistryEntry, path: &Path) {
    // Branch info
    let branch_result = git_tracker::get_branch_info(path);

    // Worktree info
    let worktree_result = git_tracker::worktree::WorktreeResolver::new().resolve(path);

    // Fingerprint (fast / HEAD-commit based for speed during registration)
    let fp_hash = git_tracker::identity::Fingerprinter::fast()
        .identify(path)
        .ok()
        .map(|id| id.fingerprint.hash);

    match (branch_result, worktree_result) {
        (Ok(branch), Ok(worktree)) => {
            entry.apply_tracker_info(&branch, &worktree, fp_hash);
        }
        (Ok(branch), Err(_)) => {
            // At least apply branch info even if worktree resolution failed.
            entry.apply_branch_info(&branch);
            entry.fingerprint_hash = fp_hash;
        }
        (Err(_), Ok(worktree)) => {
            // Apply worktree / fingerprint even if branch resolution failed.
            entry.fingerprint_hash = fp_hash;
            use git_tracker::WorktreeKind as TK;
            entry.worktree_kind = Some(match &worktree.kind {
                TK::Main            => crate::registry::entry::WorktreeKind::Main,
                TK::Linked { name } => crate::registry::entry::WorktreeKind::Linked(name.clone()),
                TK::Bare            => crate::registry::entry::WorktreeKind::Bare,
            });
            entry.main_repo_path = if worktree.kind.is_linked() {
                Some(worktree.main_repo_workdir.clone())
            } else {
                None
            };
        }
        (Err(_), Err(_)) => {
            // Nothing we can do; at least store the fingerprint.
            entry.fingerprint_hash = fp_hash;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_case_insensitive_lookup() {
        assert!("Lime".eq_ignore_ascii_case("lime"));
        assert!("Lime".eq_ignore_ascii_case("LIME"));
        assert!("Lime".eq_ignore_ascii_case("LiMe"));
        assert!(!"Lime".eq_ignore_ascii_case("Lemon"));
    }
}
