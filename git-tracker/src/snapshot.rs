//! On-disk snapshot store for repository location tracking.
//!
//! The snapshot system provides the persistent foundation that makes move
//! detection possible across process restarts. It works by recording a
//! lightweight [`RepoSnapshot`] for every known repository at one point in
//! time, then comparing that record against the live file system at a later
//! point to decide what has moved, appeared, or disappeared.
//!
//! ## Lifecycle
//!
//! ```text
//!  ┌──────────┐   scan()    ┌──────────────┐  save()  ┌───────────┐
//!  │ Scanner  │ ──────────► │ RepoSnapshot │ ───────► │   disk    │
//!  └──────────┘             └──────────────┘          └───────────┘
//!                                                           │
//!                                                      load()│
//!                                                           ▼
//!                                                   ┌──────────────┐
//!                                                   │SnapshotStore │
//!                                                   └──────────────┘
//!                                                           │
//!                                                    diff() │
//!                                                           ▼
//!                                                   ┌──────────────┐
//!                                                   │  SnapDiff    │
//!                                                   └──────────────┘
//! ```
//!
//! ## Storage format
//!
//! Snapshots are stored as newline-delimited JSON (`.ndjson`) so that they
//! can be appended to and streamed without loading the entire file into memory.
//! Each line is one [`RepoSnapshot`] serialized as a JSON object.
//!
//! The store also maintains a small metadata sidecar (`<name>.meta.json`)
//! alongside the data file that records when the snapshot was taken and which
//! scan roots were active.
//!
//! ## Move detection heuristic
//!
//! A repository is considered *moved* when:
//! 1. Its fingerprint exists in the previous snapshot **but** the recorded
//!    path no longer exists on disk.
//! 2. A new path with the **same fingerprint** is found in the current snapshot
//!    (or live scan).
//!
//! When the fingerprint is synthetic (see [`crate::identity::FingerprintKind`])
//! the match confidence is downgraded to [`MatchConfidence::Low`] because
//! synthetic fingerprints are location-derived.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};
use crate::identity::{FingerprintKind, RepoFingerprint};
use crate::scanner::RepoRecord;

// ── RepoSnapshot ──────────────────────────────────────────────────────────────

/// A single persisted record capturing a repository's identity and location.
///
/// These are written one-per-line into the snapshot data file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSnapshot {
    /// The stable fingerprint of this repository.
    pub fingerprint: RepoFingerprint,

    /// Absolute path to the working-tree root at the time of snapshotting.
    pub workdir: PathBuf,

    /// Absolute path to the `.git` entry (directory or file).
    pub git_dir: PathBuf,

    /// Human-readable name (folder name of `workdir`).
    pub name: String,

    /// Whether this was a bare repository.
    pub is_bare: bool,

    /// Whether this was a linked worktree at snapshot time.
    pub is_linked_worktree: bool,

    /// Unix timestamp (seconds) when this snapshot entry was recorded.
    pub snapshotted_at: u64,

    /// The scan root under which this repo was discovered.
    pub scan_root: PathBuf,

    /// The active branch name at snapshot time, or `None` for bare repos /
    /// detached HEAD / when identity collection was not enabled.
    #[serde(default)]
    pub current_branch: Option<String>,
}

impl RepoSnapshot {
    /// Construct a [`RepoSnapshot`] from a [`RepoRecord`] and its fingerprint.
    ///
    /// The `fingerprint` is passed separately because [`RepoRecord`] only
    /// carries an optional fingerprint (it may not have been computed during
    /// the scan).
    pub fn from_record(record: &RepoRecord, fingerprint: RepoFingerprint) -> Self {
        Self {
            fingerprint,
            workdir: record.workdir.clone(),
            git_dir: record.git_dir.clone(),
            name: record.name.clone(),
            is_bare: record.is_bare,
            is_linked_worktree: record.is_linked_worktree,
            snapshotted_at: unix_now(),
            scan_root: record.scan_root.clone(),
            current_branch: record.current_branch.clone(),
        }
    }

    /// Returns `true` if the working-tree path still exists on disk.
    pub fn workdir_exists(&self) -> bool {
        self.workdir.exists()
    }

    /// Returns `true` if the `.git` entry still exists on disk.
    pub fn git_dir_exists(&self) -> bool {
        self.git_dir.exists()
    }

    /// Returns `true` if both the workdir and git-dir entries exist.
    pub fn is_present_on_disk(&self) -> bool {
        self.workdir_exists() && self.git_dir_exists()
    }

    /// Returns `true` if the fingerprint is a stable, commit-derived hash.
    pub fn has_stable_fingerprint(&self) -> bool {
        self.fingerprint.is_stable()
    }
}

// ── SnapshotMeta ──────────────────────────────────────────────────────────────

/// Metadata sidecar for a snapshot file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// Human-readable label for this snapshot set (e.g. "daily", "pre-move").
    pub label: String,

    /// Unix timestamp (seconds) when this snapshot was created.
    pub created_at: u64,

    /// Unix timestamp (seconds) of the most recent update to this snapshot.
    pub updated_at: u64,

    /// The scan roots that were active when this snapshot was last written.
    pub scan_roots: Vec<PathBuf>,

    /// Total number of entries in the data file at last write.
    pub entry_count: usize,

    /// Format version – increment when the on-disk format changes.
    pub format_version: u32,
}

impl SnapshotMeta {
    const CURRENT_VERSION: u32 = 1;

    fn new(label: impl Into<String>, scan_roots: Vec<PathBuf>, entry_count: usize) -> Self {
        let now = unix_now();
        Self {
            label: label.into(),
            created_at: now,
            updated_at: now,
            scan_roots,
            entry_count,
            format_version: Self::CURRENT_VERSION,
        }
    }
}

// ── SnapDiff ──────────────────────────────────────────────────────────────────

/// How confident we are in a move/match pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchConfidence {
    /// Fingerprint is synthetic (location-derived); match is a best guess.
    Low,
    /// Fingerprint is HEAD-commit based; reliable for most purposes.
    Medium,
    /// Fingerprint is root-commit based; maximally stable across clones and moves.
    High,
}

impl MatchConfidence {
    fn for_fingerprint(fp: &RepoFingerprint) -> Self {
        match fp.kind {
            FingerprintKind::RootCommit => MatchConfidence::High,
            FingerprintKind::HeadCommit => MatchConfidence::Medium,
            FingerprintKind::Synthetic  => MatchConfidence::Low,
        }
    }
}

impl std::fmt::Display for MatchConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchConfidence::Low    => f.write_str("low"),
            MatchConfidence::Medium => f.write_str("medium"),
            MatchConfidence::High   => f.write_str("high"),
        }
    }
}

/// A repository that appears in the *previous* snapshot but is missing from
/// the *current* snapshot (or the live file system).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingRepo {
    /// The last known snapshot entry for this repository.
    pub last_known: RepoSnapshot,
}

/// A repository that appears in the *current* snapshot but not in the
/// *previous* snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRepo {
    /// The newly discovered snapshot entry.
    pub entry: RepoSnapshot,
}

/// A pair of snapshot entries that share a fingerprint but differ in location.
///
/// This is the primary signal for a repository move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovedRepo {
    /// The previous (now missing) location.
    pub from: RepoSnapshot,
    /// The new (currently present) location.
    pub to: RepoSnapshot,
    /// How reliable this move detection is.
    pub confidence: MatchConfidence,
}

/// A repository that exists in both snapshots at the same path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnchangedRepo {
    /// The current snapshot entry (identical path in both snapshots).
    pub entry: RepoSnapshot,
}

/// The result of comparing two [`SnapshotStore`] instances (or a store against
/// a live scan).
///
/// Obtain via [`SnapshotStore::diff`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapDiff {
    /// Repositories that moved between the two snapshots.
    pub moved: Vec<MovedRepo>,

    /// Repositories present in the previous snapshot but absent from the current one
    /// and with no matching fingerprint in the current snapshot.
    pub removed: Vec<MissingRepo>,

    /// Repositories present in the current snapshot but absent from the previous one.
    pub added: Vec<NewRepo>,

    /// Repositories unchanged between the two snapshots.
    pub unchanged: Vec<UnchangedRepo>,
}

impl SnapDiff {
    /// Returns `true` when there are no changes of any kind.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.removed.is_empty() && self.added.is_empty()
    }

    /// Total number of change events (moved + removed + added).
    pub fn change_count(&self) -> usize {
        self.moved.len() + self.removed.len() + self.added.len()
    }

    /// Returns only the moves with at least the given confidence level.
    pub fn moves_with_confidence(&self, min: MatchConfidence) -> Vec<&MovedRepo> {
        self.moved.iter().filter(|m| m.confidence >= min).collect()
    }
}

// ── SnapshotStore ─────────────────────────────────────────────────────────────

/// Persistent, append-friendly store of [`RepoSnapshot`] entries.
///
/// ## File layout
///
/// Given a store `name` and a `base_dir`:
///
/// ```text
/// <base_dir>/
///   <name>.ndjson       ← one JSON object per line, each a RepoSnapshot
///   <name>.meta.json    ← SnapshotMeta sidecar
/// ```
///
/// ## Usage
///
/// ```no_run
/// use git_tracker::snapshot::SnapshotStore;
/// use std::path::PathBuf;
///
/// // Create / open a store.
/// let mut store = SnapshotStore::open(
///     std::env::temp_dir().join("git-tracker"),
///     "default",
/// ).expect("failed to open store");
///
/// // Load entries from the previous run.
/// let previous = store.load_all().expect("failed to load");
///
/// // … run a new scan, build new snapshots … //
///
/// // Persist the new snapshots.
/// store.replace_all(&[], vec![]).expect("failed to save");
/// ```
pub struct SnapshotStore {
    /// Base directory for all snapshot files.
    base_dir: PathBuf,

    /// Logical name of this store (used as the filename stem).
    name: String,
}

impl SnapshotStore {
    // ── Construction ─────────────────────────────────────────────────────────

    /// Open (or create) a snapshot store.
    ///
    /// Creates `base_dir` and all parent directories if they do not exist.
    pub fn open(base_dir: impl Into<PathBuf>, name: impl Into<String>) -> Result<Self> {
        let base_dir = base_dir.into();
        let name     = name.into();

        fs::create_dir_all(&base_dir).map_err(|e| TrackerError::io(&base_dir, e))?;

        Ok(Self { base_dir, name })
    }

    // ── Paths ─────────────────────────────────────────────────────────────────

    /// Path to the NDJSON data file.
    pub fn data_path(&self) -> PathBuf {
        self.base_dir.join(format!("{}.ndjson", self.name))
    }

    /// Path to the metadata sidecar file.
    pub fn meta_path(&self) -> PathBuf {
        self.base_dir.join(format!("{}.meta.json", self.name))
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Load all snapshot entries from disk.
    ///
    /// Returns an empty `Vec` (not an error) when the data file does not yet
    /// exist.
    pub fn load_all(&self) -> Result<Vec<RepoSnapshot>> {
        let path = self.data_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&path).map_err(|e| TrackerError::io(&path, e))?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (line_no, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| TrackerError::io(&path, e))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let entry: RepoSnapshot =
                serde_json::from_str(trimmed).map_err(|e| TrackerError::SnapshotParse {
                    path: path.clone(),
                    source: e,
                })?;

            let _ = line_no; // suppress unused warning in release
            entries.push(entry);
        }

        Ok(entries)
    }

    /// Load the metadata sidecar, if it exists.
    pub fn load_meta(&self) -> Result<Option<SnapshotMeta>> {
        let path = self.meta_path();
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path).map_err(|e| TrackerError::io(&path, e))?;
        let meta: SnapshotMeta =
            serde_json::from_str(&content).map_err(|e| TrackerError::SnapshotParse {
                path: path.clone(),
                source: e,
            })?;

        Ok(Some(meta))
    }

    /// Build a lookup map from fingerprint hash → snapshot entry.
    ///
    /// When multiple entries share a fingerprint (shouldn't happen in a clean
    /// store, but possible after corruption), the most recently snapshotted
    /// one wins.
    pub fn load_as_map(&self) -> Result<HashMap<String, RepoSnapshot>> {
        let entries = self.load_all()?;
        let mut map = HashMap::with_capacity(entries.len());
        for entry in entries {
            map.insert(entry.fingerprint.hash.clone(), entry);
        }
        Ok(map)
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Append a single snapshot entry to the data file.
    ///
    /// Does **not** rewrite the entire file – suitable for incremental updates.
    /// Call [`SnapshotStore::compact`] periodically to deduplicate.
    pub fn append(&self, entry: &RepoSnapshot) -> Result<()> {
        let path = self.data_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| TrackerError::io(&path, e))?;

        let mut line =
            serde_json::to_string(entry).map_err(TrackerError::SnapshotSerialize)?;
        line.push('\n');
        file.write_all(line.as_bytes())
            .map_err(|e| TrackerError::io(&path, e))?;

        Ok(())
    }

    /// Append multiple entries efficiently in a single buffered write.
    pub fn append_many(&self, entries: &[RepoSnapshot]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let path = self.data_path();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| TrackerError::io(&path, e))?;

        let mut writer = BufWriter::new(file);

        for entry in entries {
            let line =
                serde_json::to_string(entry).map_err(TrackerError::SnapshotSerialize)?;
            writer.write_all(line.as_bytes())
                .map_err(|e| TrackerError::io(&path, e))?;
            writer.write_all(b"\n")
                .map_err(|e| TrackerError::io(&path, e))?;
        }

        writer.flush().map_err(|e| TrackerError::io(&path, e))?;
        Ok(())
    }

    /// Atomically replace the entire data file with `entries` and write fresh
    /// metadata.
    ///
    /// Writes to a sibling `.tmp` file first, then renames over the original,
    /// so a crash mid-write cannot corrupt existing data.
    pub fn replace_all(
        &self,
        entries: &[RepoSnapshot],
        scan_roots: Vec<PathBuf>,
    ) -> Result<()> {
        let data_path = self.data_path();
        let tmp_path  = data_path.with_extension("ndjson.tmp");

        // Write to the temp file.
        {
            let file = File::create(&tmp_path)
                .map_err(|e| TrackerError::io(&tmp_path, e))?;
            let mut writer = BufWriter::new(file);

            for entry in entries {
                let line = serde_json::to_string(entry)
                    .map_err(TrackerError::SnapshotSerialize)?;
                writer.write_all(line.as_bytes())
                    .map_err(|e| TrackerError::io(&tmp_path, e))?;
                writer.write_all(b"\n")
                    .map_err(|e| TrackerError::io(&tmp_path, e))?;
            }
            writer.flush().map_err(|e| TrackerError::io(&tmp_path, e))?;
        }

        // Atomic rename.
        fs::rename(&tmp_path, &data_path)
            .map_err(|e| TrackerError::io(&data_path, e))?;

        // Write updated metadata.
        self.write_meta(&self.name, scan_roots, entries.len())?;

        Ok(())
    }

    /// Update a single entry identified by its fingerprint hash.
    ///
    /// If no entry with that fingerprint exists, the entry is appended.
    /// This loads the entire file, modifies it in memory, and replaces it,
    /// so it is not suitable for very large stores (prefer [`SnapshotStore::replace_all`]
    /// in batch pipelines).
    pub fn upsert(&self, updated: &RepoSnapshot) -> Result<()> {
        let mut entries = self.load_all()?;
        let fingerprint_hash = &updated.fingerprint.hash;

        if let Some(existing) = entries
            .iter_mut()
            .find(|e| &e.fingerprint.hash == fingerprint_hash)
        {
            *existing = updated.clone();
        } else {
            entries.push(updated.clone());
        }

        let scan_roots = entries
            .iter()
            .map(|e| e.scan_root.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        self.replace_all(&entries, scan_roots)
    }

    /// Remove the entry with the given fingerprint hash, if present.
    ///
    /// Returns `true` if an entry was removed.
    pub fn remove_by_fingerprint(&self, fingerprint_hash: &str) -> Result<bool> {
        let mut entries = self.load_all()?;
        let before = entries.len();
        entries.retain(|e| e.fingerprint.hash != fingerprint_hash);
        let removed = entries.len() < before;

        if removed {
            let scan_roots = entries
                .iter()
                .map(|e| e.scan_root.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            self.replace_all(&entries, scan_roots)?;
        }

        Ok(removed)
    }

    /// Compact the data file by deduplicating on fingerprint hash, keeping
    /// only the most recently snapshotted entry for each fingerprint.
    ///
    /// Returns the number of duplicate lines removed.
    pub fn compact(&self) -> Result<usize> {
        let entries = self.load_all()?;
        let before  = entries.len();

        // Keep only the most recent entry per fingerprint.
        let mut seen: HashMap<String, RepoSnapshot> = HashMap::new();
        for entry in entries {
            seen.entry(entry.fingerprint.hash.clone())
                .and_modify(|existing| {
                    if entry.snapshotted_at > existing.snapshotted_at {
                        *existing = entry.clone();
                    }
                })
                .or_insert(entry);
        }

        let deduped: Vec<RepoSnapshot> = {
            let mut v: Vec<_> = seen.into_values().collect();
            // Stable sort by workdir for reproducibility.
            v.sort_by(|a, b| a.workdir.cmp(&b.workdir));
            v
        };

        let after   = deduped.len();
        let removed = before.saturating_sub(after);

        let scan_roots = deduped
            .iter()
            .map(|e| e.scan_root.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        self.replace_all(&deduped, scan_roots)?;
        Ok(removed)
    }

    /// Delete the data file and metadata sidecar entirely.
    ///
    /// The store directory itself is preserved.  Returns without error if the
    /// files do not exist.
    pub fn clear(&self) -> Result<()> {
        for path in [self.data_path(), self.meta_path()] {
            if path.exists() {
                fs::remove_file(&path).map_err(|e| TrackerError::io(&path, e))?;
            }
        }
        Ok(())
    }

    // ── Diff / comparison ─────────────────────────────────────────────────────

    /// Compare this store (treated as *previous*) against `current_entries`
    /// (treated as the *current* state) and return a [`SnapDiff`].
    ///
    /// The diff algorithm:
    ///
    /// 1. Build a hash-keyed map of `previous` entries.
    /// 2. Build a hash-keyed map of `current` entries.
    /// 3. For each `previous` entry:
    ///    - If its fingerprint appears in `current` at the **same path** → `unchanged`.
    ///    - If its fingerprint appears in `current` at a **different path** → `moved`.
    ///    - If its fingerprint does **not** appear in `current` → `removed`.
    /// 4. Any `current` entry whose fingerprint was not in `previous` → `added`.
    pub fn diff(&self, current_entries: &[RepoSnapshot]) -> Result<SnapDiff> {
        let previous = self.load_all()?;
        Ok(diff_snapshots(&previous, current_entries))
    }

    /// Compare two arbitrary snapshot slices without touching the store.
    ///
    /// Useful for in-memory comparisons (e.g. before persisting a new snapshot).
    pub fn diff_slices(previous: &[RepoSnapshot], current: &[RepoSnapshot]) -> SnapDiff {
        diff_snapshots(previous, current)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn write_meta(
        &self,
        label:       &str,
        scan_roots:  Vec<PathBuf>,
        entry_count: usize,
    ) -> Result<()> {
        let meta_path = self.meta_path();

        // Try to preserve created_at from the existing meta.
        let created_at = self
            .load_meta()
            .ok()
            .flatten()
            .map(|m| m.created_at)
            .unwrap_or_else(unix_now);

        let mut meta = SnapshotMeta::new(label, scan_roots, entry_count);
        meta.created_at = created_at;
        meta.updated_at = unix_now();

        let json = serde_json::to_string_pretty(&meta)
            .map_err(TrackerError::SnapshotSerialize)?;

        fs::write(&meta_path, json.as_bytes())
            .map_err(|e| TrackerError::io(&meta_path, e))?;

        Ok(())
    }
}

// ── Diff algorithm ────────────────────────────────────────────────────────────

fn diff_snapshots(previous: &[RepoSnapshot], current: &[RepoSnapshot]) -> SnapDiff {
    // Keyed by fingerprint hash.
    let prev_map: HashMap<&str, &RepoSnapshot> = previous
        .iter()
        .map(|e| (e.fingerprint.hash.as_str(), e))
        .collect();

    let curr_map: HashMap<&str, &RepoSnapshot> = current
        .iter()
        .map(|e| (e.fingerprint.hash.as_str(), e))
        .collect();

    let mut moved     = Vec::new();
    let mut removed   = Vec::new();
    let mut unchanged = Vec::new();

    for (hash, prev_entry) in &prev_map {
        if let Some(curr_entry) = curr_map.get(hash) {
            if curr_entry.workdir == prev_entry.workdir {
                unchanged.push(UnchangedRepo {
                    entry: (*curr_entry).clone(),
                });
            } else {
                let confidence = MatchConfidence::for_fingerprint(&prev_entry.fingerprint);
                moved.push(MovedRepo {
                    from: (*prev_entry).clone(),
                    to:   (*curr_entry).clone(),
                    confidence,
                });
            }
        } else {
            removed.push(MissingRepo {
                last_known: (*prev_entry).clone(),
            });
        }
    }

    // Added = present in current but not in previous.
    let added: Vec<NewRepo> = current
        .iter()
        .filter(|e| !prev_map.contains_key(e.fingerprint.hash.as_str()))
        .map(|e| NewRepo { entry: e.clone() })
        .collect();

    SnapDiff { moved, removed, added, unchanged }
}

// ── Utility ───────────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{FingerprintKind, RepoFingerprint};
    use tempfile::TempDir;

    // ── Helpers ──

    fn make_fp(hash: &str, kind: FingerprintKind) -> RepoFingerprint {
        // Pad hash to 64 chars for realism.
        let padded = format!("{:0<64}", hash);
        RepoFingerprint::from_raw(padded, kind, None)
    }

    fn make_snap(name: &str, workdir: &str, hash: &str, kind: FingerprintKind) -> RepoSnapshot {
        RepoSnapshot {
            fingerprint: make_fp(hash, kind),
            workdir: PathBuf::from(workdir),
            git_dir: PathBuf::from(format!("{workdir}/.git")),
            name: name.into(),
            is_bare: false,
            is_linked_worktree: false,
            snapshotted_at: unix_now(),
            scan_root: PathBuf::from("/projects"),
            current_branch: None,
        }
    }

    fn open_store(tmp: &TempDir) -> SnapshotStore {
        SnapshotStore::open(tmp.path(), "test").expect("open store")
    }

    // ── Round-trip ──

    #[test]
    fn append_and_load_single_entry() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);
        let snap  = make_snap("repo-a", "/projects/repo-a", "aaaa", FingerprintKind::RootCommit);

        store.append(&snap).expect("append");
        let loaded = store.load_all().expect("load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "repo-a");
        assert_eq!(loaded[0].workdir, PathBuf::from("/projects/repo-a"));
    }

    #[test]
    fn append_many_and_load() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let snaps: Vec<RepoSnapshot> = (0..5)
            .map(|i| make_snap(
                &format!("repo-{i}"),
                &format!("/projects/repo-{i}"),
                &format!("{i:0<4}"),
                FingerprintKind::RootCommit,
            ))
            .collect();

        store.append_many(&snaps).expect("append_many");
        let loaded = store.load_all().expect("load");
        assert_eq!(loaded.len(), 5);
    }

    #[test]
    fn empty_store_returns_empty_vec() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);
        let loaded = store.load_all().expect("load empty");
        assert!(loaded.is_empty());
    }

    // ── replace_all ──

    #[test]
    fn replace_all_overwrites_previous_entries() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let old = make_snap("old", "/old", "0000", FingerprintKind::RootCommit);
        store.append(&old).unwrap();

        let new_entries = vec![
            make_snap("new-a", "/new-a", "aaaa", FingerprintKind::RootCommit),
            make_snap("new-b", "/new-b", "bbbb", FingerprintKind::RootCommit),
        ];
        store.replace_all(&new_entries, vec![]).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 2);
        let names: Vec<&str> = loaded.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"old"), "old entry should be gone");
    }

    #[test]
    fn replace_all_writes_meta_sidecar() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);
        let snaps = vec![make_snap("r", "/r", "1111", FingerprintKind::RootCommit)];

        store.replace_all(&snaps, vec![PathBuf::from("/scan")]).unwrap();

        let meta = store.load_meta().unwrap().expect("meta should exist");
        assert_eq!(meta.entry_count, 1);
        assert_eq!(meta.scan_roots, vec![PathBuf::from("/scan")]);
        assert_eq!(meta.format_version, SnapshotMeta::CURRENT_VERSION);
    }

    // ── upsert ──

    #[test]
    fn upsert_updates_existing_entry() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let original = make_snap("repo", "/old/path", "cccc", FingerprintKind::RootCommit);
        store.append(&original).unwrap();

        let mut updated = original.clone();
        updated.workdir = PathBuf::from("/new/path");
        store.upsert(&updated).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1, "should not create a duplicate");
        assert_eq!(loaded[0].workdir, PathBuf::from("/new/path"));
    }

    #[test]
    fn upsert_appends_when_fingerprint_not_found() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let a = make_snap("a", "/a", "aaaa", FingerprintKind::RootCommit);
        store.append(&a).unwrap();

        let b = make_snap("b", "/b", "bbbb", FingerprintKind::RootCommit);
        store.upsert(&b).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    // ── remove_by_fingerprint ──

    #[test]
    fn remove_by_fingerprint_removes_entry() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let a = make_snap("a", "/a", "aaaa", FingerprintKind::RootCommit);
        let b = make_snap("b", "/b", "bbbb", FingerprintKind::RootCommit);
        store.append_many(&[a.clone(), b]).unwrap();

        let removed = store
            .remove_by_fingerprint(&a.fingerprint.hash)
            .unwrap();

        assert!(removed, "should report that an entry was removed");
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "b");
    }

    #[test]
    fn remove_by_fingerprint_returns_false_when_not_found() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let a = make_snap("a", "/a", "aaaa", FingerprintKind::RootCommit);
        store.append(&a).unwrap();

        let removed = store
            .remove_by_fingerprint(&"z".repeat(64))
            .unwrap();

        assert!(!removed);
    }

    // ── compact ──

    #[test]
    fn compact_removes_duplicates_keeping_most_recent() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let mut old = make_snap("old-path", "/old", "dddd", FingerprintKind::RootCommit);
        old.snapshotted_at = 1000;

        let mut new = make_snap("new-path", "/new", "dddd", FingerprintKind::RootCommit);
        new.snapshotted_at = 2000;

        // Append both – creates a duplicate fingerprint.
        let path = store.data_path();
        let mut file = OpenOptions::new()
            .create(true).append(true).open(&path).unwrap();
        writeln!(file, "{}", serde_json::to_string(&old).unwrap()).unwrap();
        writeln!(file, "{}", serde_json::to_string(&new).unwrap()).unwrap();
        drop(file);

        let removed = store.compact().unwrap();
        assert_eq!(removed, 1, "one duplicate should be removed");

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        // Most recent (snapshotted_at = 2000) should win.
        assert_eq!(loaded[0].name, "new-path");
    }

    // ── clear ──

    #[test]
    fn clear_removes_files() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        store.append(&make_snap("r", "/r", "eeee", FingerprintKind::RootCommit)).unwrap();
        store.replace_all(&[], vec![]).unwrap(); // writes meta

        store.clear().unwrap();

        assert!(!store.data_path().exists(), "data file should be gone");
        assert!(!store.meta_path().exists(), "meta file should be gone");
    }

    // ── diff ──

    #[test]
    fn diff_detects_unchanged_repo() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let snap = make_snap("repo", "/projects/repo", "ffff", FingerprintKind::RootCommit);
        store.append(&snap).unwrap();

        let current = vec![snap.clone()];
        let diff = store.diff(&current).unwrap();

        assert!(diff.moved.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.added.is_empty());
        assert_eq!(diff.unchanged.len(), 1);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_detects_moved_repo() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let old = make_snap("repo", "/old/path", "1234", FingerprintKind::RootCommit);
        store.append(&old).unwrap();

        let mut new = old.clone();
        new.workdir = PathBuf::from("/new/path");

        let diff = store.diff(&[new]).unwrap();

        assert_eq!(diff.moved.len(), 1, "should detect one move");
        assert_eq!(diff.moved[0].from.workdir, PathBuf::from("/old/path"));
        assert_eq!(diff.moved[0].to.workdir, PathBuf::from("/new/path"));
        assert_eq!(diff.moved[0].confidence, MatchConfidence::High);
        assert!(diff.removed.is_empty());
        assert!(diff.added.is_empty());
    }

    #[test]
    fn diff_detects_removed_repo() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let snap = make_snap("gone", "/gone", "5678", FingerprintKind::RootCommit);
        store.append(&snap).unwrap();

        // Current state: empty.
        let diff = store.diff(&[]).unwrap();

        assert_eq!(diff.removed.len(), 1);
        assert!(diff.moved.is_empty());
        assert!(diff.added.is_empty());
    }

    #[test]
    fn diff_detects_added_repo() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);
        // Previous state: empty.

        let new_snap = make_snap("new", "/new", "9abc", FingerprintKind::RootCommit);
        let diff = store.diff(&[new_snap]).unwrap();

        assert_eq!(diff.added.len(), 1);
        assert!(diff.moved.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_confidence_low_for_synthetic_fingerprint() {
        let previous = vec![make_snap(
            "repo", "/old", "synth1",
            FingerprintKind::Synthetic,
        )];
        let mut current = previous.clone();
        current[0].workdir = PathBuf::from("/new");

        let diff = SnapshotStore::diff_slices(&previous, &current);

        assert_eq!(diff.moved.len(), 1);
        assert_eq!(diff.moved[0].confidence, MatchConfidence::Low);
    }

    #[test]
    fn diff_confidence_medium_for_head_commit() {
        let previous = vec![make_snap(
            "repo", "/old", "head1",
            FingerprintKind::HeadCommit,
        )];
        let mut current = previous.clone();
        current[0].workdir = PathBuf::from("/new");

        let diff = SnapshotStore::diff_slices(&previous, &current);
        assert_eq!(diff.moved[0].confidence, MatchConfidence::Medium);
    }

    #[test]
    fn change_count_sums_correctly() {
        let previous = vec![
            make_snap("old",  "/old",  "aaaa", FingerprintKind::RootCommit),
            make_snap("gone", "/gone", "bbbb", FingerprintKind::RootCommit),
        ];
        let mut moved = previous[0].clone();
        moved.workdir = PathBuf::from("/moved");

        let new_snap = make_snap("new", "/new", "cccc", FingerprintKind::RootCommit);
        let current  = vec![moved, new_snap];

        let diff = SnapshotStore::diff_slices(&previous, &current);
        // 1 moved + 1 removed + 1 added = 3
        assert_eq!(diff.change_count(), 3);
    }

    #[test]
    fn moves_with_confidence_filters_correctly() {
        let previous = vec![
            make_snap("hi",  "/hi",  "h1", FingerprintKind::RootCommit),
            make_snap("lo",  "/lo",  "l1", FingerprintKind::Synthetic),
        ];
        let current = vec![
            {
                let mut e = previous[0].clone();
                e.workdir = PathBuf::from("/hi-new");
                e
            },
            {
                let mut e = previous[1].clone();
                e.workdir = PathBuf::from("/lo-new");
                e
            },
        ];

        let diff = SnapshotStore::diff_slices(&previous, &current);
        assert_eq!(diff.moved.len(), 2);

        let high_only = diff.moves_with_confidence(MatchConfidence::High);
        assert_eq!(high_only.len(), 1);
        assert_eq!(high_only[0].from.name, "hi");

        let med_and_up = diff.moves_with_confidence(MatchConfidence::Medium);
        assert_eq!(med_and_up.len(), 1);
    }

    // ── load_as_map ──

    #[test]
    fn load_as_map_keys_by_fingerprint_hash() {
        let tmp   = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let a = make_snap("a", "/a", "aaaa", FingerprintKind::RootCommit);
        let b = make_snap("b", "/b", "bbbb", FingerprintKind::RootCommit);
        store.append_many(&[a.clone(), b.clone()]).unwrap();

        let map = store.load_as_map().unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&a.fingerprint.hash));
        assert!(map.contains_key(&b.fingerprint.hash));
    }

    // ── Serde correctness ──

    #[test]
    fn snapshot_serde_roundtrip() {
        let snap = make_snap("r", "/r", "dead", FingerprintKind::HeadCommit);
        let json = serde_json::to_string(&snap).unwrap();
        let back: RepoSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap.name, back.name);
        assert_eq!(snap.fingerprint.hash, back.fingerprint.hash);
        assert_eq!(snap.workdir, back.workdir);
    }

    // ── is_present_on_disk ──

    #[test]
    fn is_present_on_disk_true_for_existing_path() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let mut snap = make_snap("r", tmp.path().to_str().unwrap(), "1111", FingerprintKind::RootCommit);
        snap.git_dir = git_dir;

        assert!(snap.is_present_on_disk());
    }

    #[test]
    fn is_present_on_disk_false_for_missing_path() {
        let snap = make_snap("gone", "/absolutely/does/not/exist", "2222", FingerprintKind::RootCommit);
        assert!(!snap.is_present_on_disk());
    }

    // ── MatchConfidence ordering ──

    #[test]
    fn match_confidence_ordering() {
        assert!(MatchConfidence::High   > MatchConfidence::Medium);
        assert!(MatchConfidence::Medium > MatchConfidence::Low);
        assert!(MatchConfidence::High   > MatchConfidence::Low);
    }
}
