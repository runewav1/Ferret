use std::path::Path;

use super::entry::RegistryEntry;
use super::storage::RegistryStorage;
use crate::error::FerretError;
use crate::git;

/// Every discrete attribute of a registry entry that can be independently
/// refreshed.  Callers compose a `Vec<RefreshField>` to express exactly which
/// parts they want updated without touching anything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RefreshField {
    /// Re-read `remote_url` and `remote_name` from the local git config
    /// (`git remote get-url origin`).  Also promotes the entry type from
    /// `Local` → `Linked` when a remote is discovered, or demotes it from
    /// `Linked` → `Local` when the remote has been removed from git.
    Remote,

    /// Re-read `current_branch`, `head_detached`, `upstream_branch`, `ahead`,
    /// and `behind` from the live repository state.
    Branch,

    /// Re-scan the working tree for file extensions and update `languages` and
    /// `language_cache_time`.
    Languages,

    /// Re-compute the BLAKE3 content fingerprint (`fingerprint_hash`) from the
    /// repository's root/HEAD commit.
    Fingerprint,

    /// Re-resolve the worktree kind (`worktree_kind`) and linked main-repo
    /// path (`main_repo_path`) via git-tracker's `WorktreeResolver`.
    Worktree,

    /// Re-read the last commit metadata (`last_commit_time`,
    /// `last_commit_message`) from the repository's HEAD commit.
    Commit,

    /// Re-canonicalise the stored local path (`local_path`) from disk.  Useful
    /// if the OS has changed how the path is resolved (symlinks, drive-letter
    /// case on Windows, etc.) without the directory actually moving.
    Path,
}

impl RefreshField {
    /// Human-readable label used in CLI output.
    pub fn label(&self) -> &'static str {
        match self {
            RefreshField::Remote => "remote",
            RefreshField::Branch => "branch",
            RefreshField::Languages => "languages",
            RefreshField::Fingerprint => "fingerprint",
            RefreshField::Worktree => "worktree",
            RefreshField::Commit => "commit",
            RefreshField::Path => "path",
        }
    }

    /// Returns every field in a canonical order — used by `--full`.
    pub fn all() -> Vec<RefreshField> {
        vec![
            RefreshField::Remote,
            RefreshField::Branch,
            RefreshField::Languages,
            RefreshField::Fingerprint,
            RefreshField::Worktree,
            RefreshField::Commit,
            RefreshField::Path,
        ]
    }
}

impl std::fmt::Display for RefreshField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ── Outcome of refreshing a single field on a single entry ───────────────────

/// The result of refreshing one [`RefreshField`] on one entry.
#[derive(Debug, Clone)]
pub enum FieldOutcome {
    /// The field was refreshed and its value changed.
    Changed {
        /// Short human-readable description of what changed.
        description: String,
    },
    /// The field was refreshed and the value is the same as before.
    Unchanged,
    /// The field could not be refreshed (e.g. the repo couldn't be opened).
    Skipped {
        /// Short reason why the field was skipped.
        reason: String,
    },
    /// This field is not applicable to this entry type (e.g. `Branch` on a
    /// lone-remote entry).
    NotApplicable,
}

impl FieldOutcome {
    /// Returns `true` when the field value actually changed.
    pub fn changed(&self) -> bool {
        matches!(self, FieldOutcome::Changed { .. })
    }
}

/// The aggregated result of a `refresh_fields` call on one entry.
#[derive(Debug, Clone)]
pub struct RefreshResult {
    /// The registry name / id of the entry that was refreshed.
    pub entry_name: String,

    /// Per-field outcomes, in the same order as the `fields` slice passed to
    /// `refresh_fields`.
    pub outcomes: Vec<(RefreshField, FieldOutcome)>,
}

impl RefreshResult {
    /// Returns `true` when at least one field value changed.
    pub fn any_changed(&self) -> bool {
        self.outcomes.iter().any(|(_, o)| o.changed())
    }

    /// Returns the number of fields that changed.
    pub fn change_count(&self) -> usize {
        self.outcomes.iter().filter(|(_, o)| o.changed()).count()
    }

    /// Returns the number of fields that were skipped.
    pub fn skip_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, o)| matches!(o, FieldOutcome::Skipped { .. }))
            .count()
    }
}

// ── Manager ──────────────────────────────────────────────────────────────────

/// Manages all registry operations.
pub struct RegistryManager {
    storage: RegistryStorage,
}

impl RegistryManager {
    /// Create a new registry manager.
    pub fn new() -> crate::error::Result<Self> {
        Ok(Self {
            storage: RegistryStorage::new()?,
        })
    }

    // ── Standard CRUD operations (unchanged) ─────────────────────────────────

    /// Add a local repository to the registry.
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

        // Populate branch info, worktree kind, and fingerprint via git-tracker.
        // Any failure here is silently swallowed so that `add` never fails just
        // because git-tracker couldn't open the repo.
        populate_tracker_fields(&mut entry, &canonical_path);

        store.entries.push(entry.clone());
        self.storage.save(&store)?;

        Ok(entry)
    }

    /// Add a lone remote (no local clone).
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

    /// Link an existing local entry to a remote.
    pub fn link_to_remote(&mut self, local_id: &str, remote_ref: &str) -> crate::error::Result<()> {
        let mut store = self.storage.load()?;

        let local_idx = store
            .entries
            .iter()
            .position(|e| e.id.eq_ignore_ascii_case(local_id))
            .ok_or_else(|| FerretError::NotFound(local_id.to_string()))?;

        let remote_url = if let Some(remote_entry) = store.entries.iter().find(|e| {
            e.remote_name.as_deref() == Some(remote_ref)
                || e.remote_url.as_deref() == Some(remote_ref)
        }) {
            remote_entry.remote_url.clone()
        } else {
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

    /// Remove an entry from the registry.
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

    /// Unlink the remote from a local entry (keeps the local entry).
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

    /// Get an entry by id (case-insensitive).
    pub fn get(&self, id: &str) -> crate::error::Result<RegistryEntry> {
        let store = self.storage.load()?;
        store
            .entries
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))
    }

    /// Get all entries.
    pub fn get_all(&self) -> crate::error::Result<Vec<RegistryEntry>> {
        let store = self.storage.load()?;
        Ok(store.entries)
    }

    /// Overwrite an existing entry wholesale (identified by `entry.id`).
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

    /// Update the `last_accessed` timestamp for an entry.
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

    // ── Surgical field-level refresh ─────────────────────────────────────────

    /// Refresh only the specified fields for the entry identified by `id`.
    ///
    /// Each field is handled independently: a failure in one field does not
    /// prevent the others from running.  The returned [`RefreshResult`]
    /// describes exactly what happened for each field.
    ///
    /// The updated entry is persisted to disk at the end of the call (one
    /// write regardless of how many fields were requested).
    ///
    /// # Errors
    ///
    /// Returns an error only when the entry cannot be found in the registry
    /// or when the registry itself cannot be loaded / saved.  Per-field
    /// failures are captured in [`FieldOutcome::Skipped`] rather than
    /// propagated as errors.
    pub fn refresh_fields(
        &mut self,
        id: &str,
        fields: &[RefreshField],
    ) -> crate::error::Result<RefreshResult> {
        let mut store = self.storage.load()?;

        let entry = store
            .entries
            .iter_mut()
            .find(|e| e.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| FerretError::NotFound(id.to_string()))?;

        let outcomes = apply_fields(entry, fields);

        self.storage.save(&store)?;

        Ok(RefreshResult {
            entry_name: id.to_string(),
            outcomes,
        })
    }

    /// Refresh the specified fields for **every** local entry in the registry.
    ///
    /// Lone-remote entries are included in the returned results but will
    /// receive [`FieldOutcome::NotApplicable`] for fields that require a local
    /// path.
    ///
    /// Returns one [`RefreshResult`] per entry, in registry order.
    pub fn refresh_fields_all(
        &mut self,
        fields: &[RefreshField],
    ) -> crate::error::Result<Vec<RefreshResult>> {
        let mut store = self.storage.load()?;
        let mut results = Vec::with_capacity(store.entries.len());

        for entry in store.entries.iter_mut() {
            let outcomes = apply_fields(entry, fields);
            results.push(RefreshResult {
                entry_name: entry.name.clone(),
                outcomes,
            });
        }

        self.storage.save(&store)?;

        Ok(results)
    }

    // ── Legacy coarse-grained refresh helpers (kept for compatibility) ────────

    /// Re-read the current branch (and upstream divergence) for a single entry
    /// and persist the result.
    ///
    /// Equivalent to `refresh_fields(id, &[RefreshField::Branch])`.
    pub fn refresh_branch(&mut self, id: &str) -> crate::error::Result<()> {
        self.refresh_fields(id, &[RefreshField::Branch])?;
        Ok(())
    }

    /// Re-read branch info **and** all tracker fields for a single entry and
    /// persist.
    ///
    /// Equivalent to `refresh_fields(id, &RefreshField::all())`.
    pub fn refresh_tracker(&mut self, id: &str) -> crate::error::Result<()> {
        self.refresh_fields(id, &RefreshField::all())?;
        Ok(())
    }

    /// Refresh branch info for **all** local entries.
    ///
    /// Returns the number of entries where the branch value actually changed.
    pub fn refresh_all_branches(&mut self) -> crate::error::Result<usize> {
        let results = self.refresh_fields_all(&[RefreshField::Branch])?;
        Ok(results.iter().filter(|r| r.any_changed()).count())
    }

    /// Refresh all tracker fields for every local entry.
    ///
    /// Returns the number of entries where at least one value changed.
    pub fn refresh_all_tracker(&mut self) -> crate::error::Result<usize> {
        let results = self.refresh_fields_all(&RefreshField::all())?;
        Ok(results.iter().filter(|r| r.any_changed()).count())
    }
}

// ── Field dispatch ────────────────────────────────────────────────────────────

/// Apply every field in `fields` to `entry`, returning one [`FieldOutcome`]
/// per field.  Runs independently per field so a failure in one does not block
/// the others.
fn apply_fields(
    entry: &mut RegistryEntry,
    fields: &[RefreshField],
) -> Vec<(RefreshField, FieldOutcome)> {
    let mut outcomes = Vec::with_capacity(fields.len());

    for field in fields {
        let outcome = apply_single_field(entry, field);
        outcomes.push((field.clone(), outcome));
    }

    outcomes
}

/// Dispatch one field update on `entry`.
fn apply_single_field(entry: &mut RegistryEntry, field: &RefreshField) -> FieldOutcome {
    match field {
        RefreshField::Remote => refresh_remote(entry),
        RefreshField::Branch => refresh_branch(entry),
        RefreshField::Languages => refresh_languages(entry),
        RefreshField::Fingerprint => refresh_fingerprint(entry),
        RefreshField::Worktree => refresh_worktree(entry),
        RefreshField::Commit => refresh_commit(entry),
        RefreshField::Path => refresh_path(entry),
    }
}

// ── Per-field refresh functions ───────────────────────────────────────────────

/// Re-read the git remote (`origin`) URL from the local git config and update
/// `remote_url`, `remote_name`, and `entry_type` accordingly.
fn refresh_remote(entry: &mut RegistryEntry) -> FieldOutcome {
    use crate::registry::entry::EntryType;

    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    // Ask git for all remotes.
    let remotes = match git::remote::get_remotes(&path) {
        Ok(r) => r,
        Err(e) => {
            return FieldOutcome::Skipped {
                reason: e.to_string(),
            }
        }
    };

    // Build a map: remote_name → fetch URL.
    let mut fetch_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for r in &remotes {
        if r.remote_type == git::remote::RemoteType::Fetch {
            fetch_map.insert(r.name.clone(), r.url.clone());
        }
    }

    // Prefer "origin"; fall back to the first available fetch remote.
    let new_url: Option<String> = fetch_map
        .get("origin")
        .cloned()
        .or_else(|| fetch_map.into_values().next());

    let old_url = entry.remote_url.clone();

    match new_url {
        Some(url) => {
            let remote_name = git::remote::remote_url_to_name(&url);
            let changed = old_url.as_deref() != Some(url.as_str());

            let description = if changed {
                if old_url.is_none() {
                    format!("remote discovered: {}", url)
                } else {
                    format!(
                        "remote updated: {} → {}",
                        old_url.as_deref().unwrap_or("(none)"),
                        url
                    )
                }
            } else {
                String::new()
            };

            entry.remote_url = Some(url);
            entry.remote_name = Some(remote_name);

            // Promote entry type when a remote is now present.
            if entry.entry_type == EntryType::Local {
                entry.entry_type = EntryType::Linked;
            }

            if changed {
                FieldOutcome::Changed { description }
            } else {
                FieldOutcome::Unchanged
            }
        }

        None => {
            // No remote in git — clear any stale remote data.
            let had_remote = entry.remote_url.is_some();

            if had_remote {
                let old = entry.remote_url.take().unwrap_or_default();
                entry.remote_name = None;

                // Demote to Local if we were Linked.
                if entry.entry_type == EntryType::Linked {
                    entry.entry_type = EntryType::Local;
                }

                FieldOutcome::Changed {
                    description: format!("remote removed: {}", old),
                }
            } else {
                FieldOutcome::Unchanged
            }
        }
    }
}

/// Re-read the current branch, detached state, upstream, and divergence.
fn refresh_branch(entry: &mut RegistryEntry) -> FieldOutcome {
    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    match git_tracker::get_branch_info(&path) {
        Ok(info) => {
            let old_branch = entry.current_branch.clone();
            let old_detached = entry.head_detached;
            let old_upstream = entry.upstream_branch.clone();
            let old_ahead = entry.ahead;
            let old_behind = entry.behind;

            entry.apply_branch_info(&info);

            let branch_changed = entry.current_branch != old_branch;
            let detached_changed = entry.head_detached != old_detached;
            let upstream_changed = entry.upstream_branch != old_upstream;
            let diverge_changed = entry.ahead != old_ahead || entry.behind != old_behind;

            if branch_changed || detached_changed || upstream_changed || diverge_changed {
                let new_label = entry.branch_label().to_string();
                let old_label = if old_detached {
                    "(detached)".to_string()
                } else {
                    old_branch.as_deref().unwrap_or("—").to_string()
                };

                let mut parts = Vec::new();

                if branch_changed || detached_changed {
                    parts.push(format!("branch: {} → {}", old_label, new_label));
                }
                if upstream_changed {
                    let old_up = old_upstream.as_deref().unwrap_or("(none)");
                    let new_up = entry.upstream_branch.as_deref().unwrap_or("(none)");
                    parts.push(format!("tracking: {} → {}", old_up, new_up));
                }
                if diverge_changed {
                    let hint = entry.divergence_hint();
                    if hint.is_empty() {
                        parts.push("divergence: cleared".to_string());
                    } else {
                        parts.push(format!("divergence: {}", hint));
                    }
                }

                FieldOutcome::Changed {
                    description: parts.join("; "),
                }
            } else {
                FieldOutcome::Unchanged
            }
        }

        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

/// Re-scan the working tree for file extensions and update `languages`.
fn refresh_languages(entry: &mut RegistryEntry) -> FieldOutcome {
    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    match crate::language::detector::LanguageDetector::new().detect_language_names(&path) {
        Ok(new_langs) => {
            let old_langs = entry.languages.clone();

            // Compare as sorted sets to avoid false positives from ordering.
            let mut old_sorted = old_langs.clone();
            let mut new_sorted = new_langs.clone();
            old_sorted.sort_unstable();
            new_sorted.sort_unstable();

            if old_sorted != new_sorted {
                let added: Vec<&str> = new_sorted
                    .iter()
                    .filter(|l| !old_sorted.contains(l))
                    .map(|l| l.as_str())
                    .collect();
                let removed: Vec<&str> = old_sorted
                    .iter()
                    .filter(|l| !new_sorted.contains(l))
                    .map(|l| l.as_str())
                    .collect();

                let mut parts = Vec::new();
                if !added.is_empty() {
                    parts.push(format!("added: {}", added.join(", ")));
                }
                if !removed.is_empty() {
                    parts.push(format!("removed: {}", removed.join(", ")));
                }

                entry.languages = new_langs;
                entry.language_cache_time = Some(chrono::Utc::now());

                FieldOutcome::Changed {
                    description: parts.join("; "),
                }
            } else {
                // Even when unchanged, update the cache timestamp.
                entry.language_cache_time = Some(chrono::Utc::now());
                FieldOutcome::Unchanged
            }
        }

        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

/// Re-compute the content-stable BLAKE3 fingerprint hash.
fn refresh_fingerprint(entry: &mut RegistryEntry) -> FieldOutcome {
    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    match git_tracker::identity::Fingerprinter::fast().identify(&path) {
        Ok(identity) => {
            let new_hash = identity.fingerprint.hash.clone();
            let old_hash = entry.fingerprint_hash.clone();

            if old_hash.as_deref() != Some(new_hash.as_str()) {
                let description = match &old_hash {
                    None => format!("fingerprint set: {}", &new_hash[..16]),
                    Some(old) => format!(
                        "fingerprint changed: {}… → {}…",
                        &old[..16.min(old.len())],
                        &new_hash[..16]
                    ),
                };
                entry.fingerprint_hash = Some(new_hash);
                FieldOutcome::Changed { description }
            } else {
                FieldOutcome::Unchanged
            }
        }

        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

/// Re-resolve the worktree kind and linked main-repo path.
fn refresh_worktree(entry: &mut RegistryEntry) -> FieldOutcome {
    use crate::registry::entry::WorktreeKind;
    use git_tracker::WorktreeKind as TK;

    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    match git_tracker::worktree::WorktreeResolver::new().resolve(&path) {
        Ok(info) => {
            let new_kind: WorktreeKind = match &info.kind {
                TK::Main => WorktreeKind::Main,
                TK::Linked { name } => WorktreeKind::Linked(name.clone()),
                TK::Bare => WorktreeKind::Bare,
            };

            let new_main_path: Option<std::path::PathBuf> = if info.kind.is_linked() {
                Some(info.main_repo_workdir.clone())
            } else {
                None
            };

            let old_kind = entry.worktree_kind.clone();
            let old_main_path = entry.main_repo_path.clone();

            let kind_changed = old_kind.as_ref() != Some(&new_kind);
            let main_path_changed = old_main_path != new_main_path;

            if kind_changed || main_path_changed {
                let old_label = old_kind
                    .as_ref()
                    .map(|k| k.label().to_string())
                    .unwrap_or_else(|| "(unknown)".to_string());

                let description = format!("worktree kind: {} → {}", old_label, new_kind.label());

                entry.worktree_kind = Some(new_kind);
                entry.main_repo_path = new_main_path;

                FieldOutcome::Changed { description }
            } else {
                FieldOutcome::Unchanged
            }
        }

        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

/// Re-read the last commit metadata from HEAD.
fn refresh_commit(entry: &mut RegistryEntry) -> FieldOutcome {
    let path = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    match git::commit::get_last_commit(&path) {
        Ok(Some(commit)) => {
            let old_time = entry.last_commit_time;
            let old_msg = entry.last_commit_message.clone();

            let new_time = Some(commit.timestamp);
            let new_msg = Some(commit.message.clone());

            let time_changed = old_time != new_time;
            let msg_changed = old_msg.as_deref() != new_msg.as_deref();

            if time_changed || msg_changed {
                let mut parts = Vec::new();

                if time_changed {
                    parts.push(format!(
                        "commit time updated: {}",
                        commit.timestamp.format("%Y-%m-%d %H:%M UTC")
                    ));
                }
                if msg_changed {
                    let short = if commit.message.len() > 50 {
                        format!("{}…", &commit.message[..50])
                    } else {
                        commit.message.clone()
                    };
                    parts.push(format!("last message: {}", short));
                }

                entry.last_commit_time = new_time;
                entry.last_commit_message = new_msg;

                FieldOutcome::Changed {
                    description: parts.join("; "),
                }
            } else {
                FieldOutcome::Unchanged
            }
        }

        Ok(None) => {
            // Repository has no commits yet.
            if entry.last_commit_time.is_some() || entry.last_commit_message.is_some() {
                entry.last_commit_time = None;
                entry.last_commit_message = None;
                FieldOutcome::Changed {
                    description: "commit data cleared (repository is now empty)".to_string(),
                }
            } else {
                FieldOutcome::Unchanged
            }
        }

        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

/// Re-canonicalise the stored local path from disk.
fn refresh_path(entry: &mut RegistryEntry) -> FieldOutcome {
    let current = match &entry.local_path {
        Some(p) => p.clone(),
        None => return FieldOutcome::NotApplicable,
    };

    if !current.exists() {
        return FieldOutcome::Skipped {
            reason: format!("path does not exist: {}", current.display()),
        };
    }

    match crate::pathutil::canonicalize_path(&current) {
        Ok(canonical) => {
            if canonical != current {
                let description = format!(
                    "path canonicalised: {} → {}",
                    current.display(),
                    canonical.display()
                );
                entry.local_path = Some(canonical);
                FieldOutcome::Changed { description }
            } else {
                FieldOutcome::Unchanged
            }
        }
        Err(e) => FieldOutcome::Skipped {
            reason: e.to_string(),
        },
    }
}

// ── Bulk tracker helper (used by add_local) ───────────────────────────────────

/// Populate all git-tracker–derived fields on `entry` from the repository at
/// `path`.  Any per-step failure is silently ignored.
fn populate_tracker_fields(entry: &mut RegistryEntry, path: &Path) {
    let branch_result = git_tracker::get_branch_info(path);
    let worktree_result = git_tracker::worktree::WorktreeResolver::new().resolve(path);
    let fp_hash = git_tracker::identity::Fingerprinter::fast()
        .identify(path)
        .ok()
        .map(|id| id.fingerprint.hash);

    // Populate last commit time and message so --by-commit and --inverse sort correctly.
    if let Ok(Some(commit)) = crate::git::commit::get_last_commit(path) {
        entry.last_commit_time = Some(commit.timestamp);
        entry.last_commit_message = Some(commit.message);
    }

    match (branch_result, worktree_result) {
        (Ok(branch), Ok(worktree)) => {
            entry.apply_tracker_info(&branch, &worktree, fp_hash);
        }
        (Ok(branch), Err(_)) => {
            entry.apply_branch_info(&branch);
            entry.fingerprint_hash = fp_hash;
        }
        (Err(_), Ok(worktree)) => {
            use crate::registry::entry::WorktreeKind;
            use git_tracker::WorktreeKind as TK;

            entry.fingerprint_hash = fp_hash;
            entry.worktree_kind = Some(match &worktree.kind {
                TK::Main => WorktreeKind::Main,
                TK::Linked { name } => WorktreeKind::Linked(name.clone()),
                TK::Bare => WorktreeKind::Bare,
            });
            entry.main_repo_path = if worktree.kind.is_linked() {
                Some(worktree.main_repo_workdir.clone())
            } else {
                None
            };
        }
        (Err(_), Err(_)) => {
            entry.fingerprint_hash = fp_hash;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_case_insensitive_lookup() {
        assert!("Lime".eq_ignore_ascii_case("lime"));
        assert!("Lime".eq_ignore_ascii_case("LIME"));
        assert!("Lime".eq_ignore_ascii_case("LiMe"));
        assert!(!"Lime".eq_ignore_ascii_case("Lemon"));
    }

    #[test]
    fn refresh_field_all_contains_every_variant() {
        let all = RefreshField::all();
        assert!(all.contains(&RefreshField::Remote));
        assert!(all.contains(&RefreshField::Branch));
        assert!(all.contains(&RefreshField::Languages));
        assert!(all.contains(&RefreshField::Fingerprint));
        assert!(all.contains(&RefreshField::Worktree));
        assert!(all.contains(&RefreshField::Commit));
        assert!(all.contains(&RefreshField::Path));
        assert_eq!(all.len(), 7);
    }

    #[test]
    fn refresh_field_labels_are_unique() {
        let all = RefreshField::all();
        let labels: Vec<&str> = all.iter().map(|f| f.label()).collect();
        let unique: std::collections::HashSet<&&str> = labels.iter().collect();
        assert_eq!(labels.len(), unique.len(), "labels must be unique");
    }

    #[test]
    fn refresh_field_display() {
        assert_eq!(RefreshField::Remote.to_string(), "remote");
        assert_eq!(RefreshField::Branch.to_string(), "branch");
        assert_eq!(RefreshField::Languages.to_string(), "languages");
        assert_eq!(RefreshField::Fingerprint.to_string(), "fingerprint");
        assert_eq!(RefreshField::Worktree.to_string(), "worktree");
        assert_eq!(RefreshField::Commit.to_string(), "commit");
        assert_eq!(RefreshField::Path.to_string(), "path");
    }

    #[test]
    fn field_outcome_changed_predicate() {
        assert!(FieldOutcome::Changed {
            description: "x".into()
        }
        .changed());
        assert!(!FieldOutcome::Unchanged.changed());
        assert!(!FieldOutcome::Skipped { reason: "y".into() }.changed());
        assert!(!FieldOutcome::NotApplicable.changed());
    }

    #[test]
    fn refresh_result_change_count() {
        let result = RefreshResult {
            entry_name: "test".into(),
            outcomes: vec![
                (
                    RefreshField::Branch,
                    FieldOutcome::Changed {
                        description: "x".into(),
                    },
                ),
                (RefreshField::Remote, FieldOutcome::Unchanged),
                (
                    RefreshField::Languages,
                    FieldOutcome::Skipped {
                        reason: "err".into(),
                    },
                ),
                (RefreshField::Fingerprint, FieldOutcome::NotApplicable),
            ],
        };
        assert_eq!(result.change_count(), 1);
        assert_eq!(result.skip_count(), 1);
        assert!(result.any_changed());
    }

    #[test]
    fn refresh_result_no_changes() {
        let result = RefreshResult {
            entry_name: "test".into(),
            outcomes: vec![
                (RefreshField::Branch, FieldOutcome::Unchanged),
                (RefreshField::Remote, FieldOutcome::Unchanged),
            ],
        };
        assert_eq!(result.change_count(), 0);
        assert!(!result.any_changed());
    }
}
