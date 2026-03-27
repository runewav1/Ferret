//! File-system event watcher for repository move detection.
//!
//! [`RepoWatcher`] wraps the [`notify`] crate and converts raw file-system
//! events into semantically meaningful [`MoveEvent`]s. It is specifically
//! designed to detect when a directory that contains a git repository is
//! renamed or moved on disk, and to resolve the *new* location of that
//! repository automatically.
//!
//! ## How move detection works
//!
//! Modern file-system APIs (inotify, FSEvents, ReadDirectoryChangesW) emit
//! paired `Remove` / `Create` events (or a single `Rename` event on Linux)
//! when a directory is moved within the same file system. On cross-device
//! moves the OS emits a `Create` at the destination and a `Remove` at the
//! source.
//!
//! The watcher handles both cases:
//!
//! 1. **Same-device rename** – `notify` surfaces a
//!    [`EventKind::Modify(ModifyKind::Name(RenameMode::Both))`] event that
//!    carries both the `from` and `to` paths directly. These are translated
//!    immediately into [`MoveEvent`]s.
//!
//! 2. **Cross-device / two-event rename** – `notify` emits a
//!    [`EventKind::Remove`] followed (shortly) by a
//!    [`EventKind::Create`]. The watcher keeps a small ring-buffer of
//!    recent removes; when a `Create` arrives for a directory that contains a
//!    `.git` entry, we search the buffer for a snapshot whose fingerprint
//!    matches the newly appeared repository, and pair them up.
//!
//! ## Usage
//!
//! ```no_run
//! use git_tracker::watcher::{RepoWatcher, WatcherConfig, MoveEvent};
//! use std::path::PathBuf;
//! use std::sync::mpsc;
//!
//! let (tx, rx) = mpsc::channel::<MoveEvent>();
//!
//! let mut watcher = RepoWatcher::new(
//!     WatcherConfig::default(),
//!     move |event| { let _ = tx.send(event); },
//! ).expect("failed to create watcher");
//!
//! watcher.watch(PathBuf::from("/home/user/projects")).expect("failed to watch");
//!
//! // In your event loop:
//! for event in rx {
//!     println!("Moved: {} → {}", event.from.display(), event.to.display());
//! }
//! ```

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{
    Config as NotifyConfig, Error as NotifyError, Event, EventKind, RecommendedWatcher,
    RecursiveMode, Watcher,
};
use notify::event::{ModifyKind, RenameMode, RemoveKind, CreateKind};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TrackerError};
use crate::identity::{Fingerprinter, RepoFingerprint};

// ── MoveEvent ─────────────────────────────────────────────────────────────────

/// A detected repository move / rename event.
///
/// Produced by [`RepoWatcher`] and delivered to the caller's callback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveEvent {
    /// The previous (source) path of the repository working-tree root.
    pub from: PathBuf,

    /// The new (destination) path of the repository working-tree root.
    pub to: PathBuf,

    /// How the move was detected.
    pub detection_method: DetectionMethod,

    /// The repository fingerprint that was used to link `from` and `to`.
    ///
    /// `None` when [`DetectionMethod::AtomicRename`] is used and fingerprint
    /// computation was not requested (see [`WatcherConfig::compute_fingerprints`]).
    pub fingerprint: Option<RepoFingerprint>,

    /// Wall-clock time when this event was emitted (as a Unix timestamp in
    /// seconds, for easy serialization).
    pub detected_at: u64,
}

impl MoveEvent {
    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// How a [`MoveEvent`] was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionMethod {
    /// The OS reported a single atomic rename event carrying both source and
    /// destination paths (e.g. `inotify IN_MOVED_FROM` / `IN_MOVED_TO` pair,
    /// or a `notify` `RenameMode::Both` event).
    AtomicRename,

    /// A `Create` event for a new git repository was correlated with a prior
    /// `Remove` event via fingerprint matching.  Used for cross-device moves
    /// or on platforms where paired rename events are not available.
    FingerprintCorrelation,

    /// A `Create` event appeared for a new git repository, but no prior
    /// `Remove` event could be matched. This may indicate a copy rather than
    /// a move, or the source was outside a watched directory.
    UnpairedCreate,
}

impl std::fmt::Display for DetectionMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectionMethod::AtomicRename          => f.write_str("atomic-rename"),
            DetectionMethod::FingerprintCorrelation => f.write_str("fingerprint-correlation"),
            DetectionMethod::UnpairedCreate         => f.write_str("unpaired-create"),
        }
    }
}

// ── WatcherConfig ─────────────────────────────────────────────────────────────

/// Configuration for a [`RepoWatcher`] instance.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// How long to retain a `Remove` event in the pending buffer while waiting
    /// for a matching `Create`.  Default: 5 seconds.
    ///
    /// After this window elapses, the remove is treated as a deletion (not a
    /// move), and the pending entry is discarded.
    pub correlation_window: Duration,

    /// When `true`, a [`Fingerprinter`] is invoked on every newly-appeared git
    /// repository to enable fingerprint-based correlation.  Adds a small amount
    /// of latency per event but dramatically improves accuracy.  Default: `true`.
    pub compute_fingerprints: bool,

    /// Use the fast (HEAD-commit) fingerprint strategy instead of walking to
    /// the root commit.  Default: `true`.
    pub fast_fingerprint: bool,

    /// Maximum number of pending removes to keep in the correlation buffer.
    /// Once full, the oldest entry is evicted.  Default: 64.
    pub max_pending_removes: usize,

    /// When `true`, also emit [`MoveEvent`]s for [`DetectionMethod::UnpairedCreate`]
    /// (new git repos with no matching prior remove).  Default: `false`.
    pub emit_unpaired_creates: bool,

    /// Polling interval used as the `notify` poll fallback on platforms that
    /// do not support native events.  Default: 2 seconds.
    pub poll_interval: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            correlation_window:   Duration::from_secs(5),
            compute_fingerprints: true,
            fast_fingerprint:     true,
            max_pending_removes:  64,
            emit_unpaired_creates: false,
            poll_interval:        Duration::from_secs(2),
        }
    }
}

// ── Internal pending-remove entry ─────────────────────────────────────────────

/// A `Remove` event for a path that *may* have been the source of a move.
/// Retained in the correlation buffer until a match is found or the window
/// expires.
#[derive(Debug, Clone)]
struct PendingRemove {
    /// The path that was removed.
    path: PathBuf,

    /// Fingerprint of the repo that used to live at `path`, if it could be
    /// read from a cached snapshot before disappearing.  In practice, by the
    /// time the OS delivers the remove event the directory may already be gone,
    /// so this is often `None`.
    fingerprint: Option<RepoFingerprint>,

    /// When this pending entry was recorded.
    recorded_at: Instant,
}

// ── RepoWatcher ───────────────────────────────────────────────────────────────

/// File-system watcher that emits [`MoveEvent`]s for git repository moves.
///
/// Create with [`RepoWatcher::new`], register directories with
/// [`RepoWatcher::watch`] / [`RepoWatcher::unwatch`], and stop with
/// [`RepoWatcher::stop`].
///
/// The internal `notify` watcher runs on a background thread; the callback
/// you supply is invoked from that thread.  Make it cheap and non-blocking
/// (e.g. send to a channel) to avoid stalling the event pipeline.
pub struct RepoWatcher {
    /// The underlying notify watcher.
    inner: RecommendedWatcher,

    /// Configuration for this watcher.
    config: WatcherConfig,

    /// Shared correlation buffer (also accessed from the notify callback
    /// closure via `Arc<Mutex<…>>`).
    pending: Arc<Mutex<VecDeque<PendingRemove>>>,

    /// Paths currently being watched (for bookkeeping / unwatch support).
    watched_paths: Vec<PathBuf>,
}

impl RepoWatcher {
    /// Create a new [`RepoWatcher`].
    ///
    /// `callback` is invoked once for every [`MoveEvent`] that is detected.
    /// It is called from the `notify` background thread, so it must be
    /// `Send + 'static`.
    ///
    /// # Errors
    ///
    /// Returns [`TrackerError::WatcherInit`] when the underlying `notify`
    /// watcher cannot be created (e.g. the OS does not support the required
    /// API).
    pub fn new<F>(config: WatcherConfig, callback: F) -> Result<Self>
    where
        F: Fn(MoveEvent) + Send + Sync + 'static,
    {
        let pending: Arc<Mutex<VecDeque<PendingRemove>>> =
            Arc::new(Mutex::new(VecDeque::new()));

        let pending_clone  = Arc::clone(&pending);
        let config_clone   = config.clone();
        let callback: Arc<dyn Fn(MoveEvent) + Send + Sync> = Arc::new(callback);

        let notify_config = NotifyConfig::default()
            .with_poll_interval(config.poll_interval);

        let inner: RecommendedWatcher =
            notify::RecommendedWatcher::new(
                move |res: std::result::Result<Event, NotifyError>| {
                    let event = match res {
                        Ok(e)  => e,
                        Err(_) => return,
                    };

                    handle_event(
                        event,
                        &pending_clone,
                        &config_clone,
                        &callback,
                    );
                },
                notify_config,
            )
            .map_err(|e| TrackerError::WatcherInit(e.to_string()))?;

        Ok(Self {
            inner,
            config,
            pending,
            watched_paths: Vec::new(),
        })
    }

    /// Begin watching `path` recursively.
    ///
    /// Can be called multiple times to watch several root directories.
    ///
    /// # Errors
    ///
    /// Returns [`TrackerError::WatcherRegister`] if `notify` cannot register
    /// the path (e.g. path does not exist, or permission denied).
    pub fn watch(&mut self, path: impl Into<PathBuf>) -> Result<()> {
        let path = path.into();

        self.inner
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|e| TrackerError::WatcherRegister {
                path:   path.clone(),
                reason: e.to_string(),
            })?;

        if !self.watched_paths.contains(&path) {
            self.watched_paths.push(path);
        }

        Ok(())
    }

    /// Stop watching `path`.
    ///
    /// No-op (no error) if the path was not being watched.
    pub fn unwatch(&mut self, path: &Path) -> Result<()> {
        // notify returns an error if the path wasn't watched; we treat that
        // as a no-op since the caller's intent is satisfied either way.
        let _ = self.inner.unwatch(path);
        self.watched_paths.retain(|p| p != path);
        Ok(())
    }

    /// Stop all watches and shut down the watcher.
    ///
    /// After this call the [`RepoWatcher`] is unusable.
    pub fn stop(mut self) -> Result<()> {
        let paths: Vec<PathBuf> = self.watched_paths.drain(..).collect();
        for path in paths {
            let _ = self.inner.unwatch(&path);
        }
        Ok(())
    }

    /// Returns a snapshot of the paths currently being watched.
    pub fn watched_paths(&self) -> &[PathBuf] {
        &self.watched_paths
    }

    /// Flush the correlation buffer, discarding any pending removes that have
    /// exceeded the [`WatcherConfig::correlation_window`].
    ///
    /// This is called automatically as part of event processing, but can also
    /// be called manually (e.g. from a periodic housekeeping task).
    pub fn flush_expired_pending(&self) {
        flush_expired(&self.pending, self.config.correlation_window);
    }

    /// Snapshot the current correlation buffer for debugging / inspection.
    pub fn pending_removes_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

// ── Event handler (called from notify background thread) ─────────────────────

fn handle_event(
    event:         Event,
    pending:       &Arc<Mutex<VecDeque<PendingRemove>>>,
    config:        &WatcherConfig,
    callback:      &Arc<dyn Fn(MoveEvent) + Send + Sync>,
) {
    // Expire old pending removes first.
    flush_expired(pending, config.correlation_window);

    match event.kind {
        // ── Atomic rename (same-device, Linux inotify / Windows RDCW) ────────
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            // notify provides [from, to] in event.paths for this variant.
            if event.paths.len() == 2 {
                let from = event.paths[0].clone();
                let to   = event.paths[1].clone();

                // Only care if the destination looks like a git repo.
                if !is_git_root(&to) {
                    return;
                }

                let fingerprint = if config.compute_fingerprints {
                    compute_fingerprint(&to, config.fast_fingerprint)
                } else {
                    None
                };

                callback(MoveEvent {
                    from,
                    to,
                    detection_method: DetectionMethod::AtomicRename,
                    fingerprint,
                    detected_at: MoveEvent::now_secs(),
                });
            }
        }

        // ── Source half of a two-event rename (or a true deletion) ────────────
        EventKind::Remove(RemoveKind::Folder) | EventKind::Remove(RemoveKind::Any) => {
            for path in &event.paths {
                // We can no longer read the directory, so we cannot compute a
                // fingerprint here.  We record the path with no fingerprint and
                // hope that a cached snapshot (if any) can supply it later.
                // In practice, the OS often delivers the remove event *before*
                // the files are actually gone, so we try a quick fingerprint
                // read opportunistically.
                let fingerprint = if config.compute_fingerprints && path.exists() {
                    compute_fingerprint(path, config.fast_fingerprint)
                } else {
                    None
                };

                let entry = PendingRemove {
                    path:        path.clone(),
                    fingerprint,
                    recorded_at: Instant::now(),
                };

                let mut buf = pending.lock().unwrap();

                // Evict the oldest entry if the buffer is full.
                if buf.len() >= config.max_pending_removes {
                    buf.pop_front();
                }

                buf.push_back(entry);
            }
        }

        // ── Destination half of a two-event rename (or a new clone/copy) ──────
        EventKind::Create(CreateKind::Folder) | EventKind::Create(CreateKind::Any) => {
            for path in &event.paths {
                if !is_git_root(path) {
                    continue;
                }

                let new_fp = if config.compute_fingerprints {
                    compute_fingerprint(path, config.fast_fingerprint)
                } else {
                    None
                };

                // Try to find a matching pending remove.
                let matched = {
                    let mut buf = pending.lock().unwrap();
                    find_and_remove_match(&mut buf, new_fp.as_ref())
                };

                match matched {
                    Some(pending_remove) => {
                        callback(MoveEvent {
                            from:             pending_remove.path.clone(),
                            to:               path.clone(),
                            detection_method: DetectionMethod::FingerprintCorrelation,
                            fingerprint:      new_fp,
                            detected_at:      MoveEvent::now_secs(),
                        });
                    }
                    None => {
                        if config.emit_unpaired_creates {
                            // Emit with a synthetic "from" path that signals
                            // the source is unknown.
                            callback(MoveEvent {
                                from:             PathBuf::from("<unknown>"),
                                to:               path.clone(),
                                detection_method: DetectionMethod::UnpairedCreate,
                                fingerprint:      new_fp,
                                detected_at:      MoveEvent::now_secs(),
                            });
                        }
                    }
                }
            }
        }

        // All other event kinds are irrelevant for move detection.
        _ => {}
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` if `path` contains a `.git` directory or file, making it
/// a git working-tree root.  Also returns `true` for bare repositories
/// (identified by the presence of `HEAD` + `objects/` + `refs/`).
fn is_git_root(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }

    let dot_git = path.join(".git");
    if dot_git.exists() {
        return true;
    }

    // Bare repo heuristic.
    path.join("HEAD").is_file()
        && path.join("objects").is_dir()
        && path.join("refs").is_dir()
}

/// Attempt to compute a [`RepoFingerprint`] for a repository at `path`.
///
/// Returns `None` on any error (the directory may not be fully written yet,
/// or it may not actually be a git repository).
fn compute_fingerprint(path: &Path, fast: bool) -> Option<RepoFingerprint> {
    let fp = if fast {
        Fingerprinter::fast()
    } else {
        Fingerprinter::new()
    };

    fp.identify(path).ok().map(|id| id.fingerprint)
}

/// Remove expired pending entries from the buffer.
fn flush_expired(pending: &Arc<Mutex<VecDeque<PendingRemove>>>, window: Duration) {
    let mut buf = pending.lock().unwrap();
    let cutoff  = Instant::now().checked_sub(window).unwrap_or_else(Instant::now);
    buf.retain(|e| e.recorded_at > cutoff);
}

/// Search `buf` for a pending remove whose fingerprint matches `target_fp`.
///
/// If `target_fp` is `None`, match any pending remove (last-in, first-out
/// ordering to prefer the most recent remove, which is most likely to be the
/// source of this create).
///
/// Removes and returns the matched entry so it cannot be matched again.
fn find_and_remove_match(
    buf:       &mut VecDeque<PendingRemove>,
    target_fp: Option<&RepoFingerprint>,
) -> Option<PendingRemove> {
    match target_fp {
        Some(fp) => {
            // Find by fingerprint.
            if let Some(pos) = buf.iter().position(|e| {
                e.fingerprint
                    .as_ref()
                    .map(|efp| efp.hash == fp.hash)
                    .unwrap_or(false)
            }) {
                buf.remove(pos)
            } else {
                // No fingerprint match.  Fall back to the most-recent entry
                // (back of the deque) as a best-effort guess.
                buf.pop_back()
            }
        }
        // No fingerprint available – use the most recently queued remove.
        None => buf.pop_back(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ── DetectionMethod display ──

    #[test]
    fn detection_method_display() {
        assert_eq!(DetectionMethod::AtomicRename.to_string(),          "atomic-rename");
        assert_eq!(DetectionMethod::FingerprintCorrelation.to_string(), "fingerprint-correlation");
        assert_eq!(DetectionMethod::UnpairedCreate.to_string(),         "unpaired-create");
    }

    // ── is_git_root ──

    #[test]
    fn is_git_root_false_for_plain_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_true_for_repo_with_dot_git_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        assert!(is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_true_for_bare_repo_heuristic() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Simulate bare repo layout.
        std::fs::write(tmp.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::create_dir(tmp.path().join("objects")).unwrap();
        std::fs::create_dir(tmp.path().join("refs")).unwrap();
        assert!(is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_false_for_non_existent_path() {
        assert!(!is_git_root(Path::new("/absolutely/does/not/exist")));
    }

    // ── find_and_remove_match ──

    fn make_pending(path: &str, fp_hash: Option<&str>) -> PendingRemove {
        PendingRemove {
            path: PathBuf::from(path),
            fingerprint: fp_hash.map(|h| crate::identity::RepoFingerprint::from_raw(
                format!("{:0<64}", h),
                crate::identity::FingerprintKind::RootCommit,
                None,
            )),
            recorded_at: Instant::now(),
        }
    }

    #[test]
    fn find_and_remove_match_by_fingerprint() {
        let mut buf = VecDeque::new();
        buf.push_back(make_pending("/old-a", Some("aaaa")));
        buf.push_back(make_pending("/old-b", Some("bbbb")));

        let target = crate::identity::RepoFingerprint::from_raw(
            format!("{:0<64}", "aaaa"),
            crate::identity::FingerprintKind::RootCommit,
            None,
        );

        let matched = find_and_remove_match(&mut buf, Some(&target));
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().path, PathBuf::from("/old-a"));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn find_and_remove_match_falls_back_to_most_recent_when_no_fp_match() {
        let mut buf = VecDeque::new();
        buf.push_back(make_pending("/old-a", Some("aaaa")));
        buf.push_back(make_pending("/old-b", Some("bbbb")));

        // Target fingerprint "cccc" does not exist in the buffer.
        let target = crate::identity::RepoFingerprint::from_raw(
            format!("{:0<64}", "cccc"),
            crate::identity::FingerprintKind::RootCommit,
            None,
        );

        let matched = find_and_remove_match(&mut buf, Some(&target));
        assert!(matched.is_some());
        // Falls back to the most recently pushed entry ("old-b").
        assert_eq!(matched.unwrap().path, PathBuf::from("/old-b"));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn find_and_remove_match_no_fp_pops_last() {
        let mut buf = VecDeque::new();
        buf.push_back(make_pending("/first",  None));
        buf.push_back(make_pending("/second", None));

        let matched = find_and_remove_match(&mut buf, None);
        assert_eq!(matched.unwrap().path, PathBuf::from("/second"));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn find_and_remove_match_empty_buf_returns_none() {
        let mut buf: VecDeque<PendingRemove> = VecDeque::new();
        let result = find_and_remove_match(&mut buf, None);
        assert!(result.is_none());
    }

    // ── flush_expired ──

    #[test]
    fn flush_expired_removes_old_entries() {
        let pending: Arc<Mutex<VecDeque<PendingRemove>>> =
            Arc::new(Mutex::new(VecDeque::new()));

        // Manually insert an entry with a very old recorded_at.
        {
            let mut buf = pending.lock().unwrap();
            buf.push_back(PendingRemove {
                path:        PathBuf::from("/old"),
                fingerprint: None,
                recorded_at: Instant::now()
                    .checked_sub(Duration::from_secs(100))
                    .unwrap(),
            });
            // Fresh entry.
            buf.push_back(PendingRemove {
                path:        PathBuf::from("/fresh"),
                fingerprint: None,
                recorded_at: Instant::now(),
            });
        }

        flush_expired(&pending, Duration::from_secs(5));

        let buf = pending.lock().unwrap();
        assert_eq!(buf.len(), 1, "only the fresh entry should remain");
        assert_eq!(buf[0].path, PathBuf::from("/fresh"));
    }

    // ── WatcherConfig defaults ──

    #[test]
    fn watcher_config_defaults_are_sensible() {
        let cfg = WatcherConfig::default();
        assert_eq!(cfg.correlation_window, Duration::from_secs(5));
        assert!(cfg.compute_fingerprints);
        assert!(cfg.fast_fingerprint);
        assert_eq!(cfg.max_pending_removes, 64);
        assert!(!cfg.emit_unpaired_creates);
        assert_eq!(cfg.poll_interval, Duration::from_secs(2));
    }

    // ── MoveEvent serde ──

    #[test]
    fn move_event_serde_roundtrip() {
        let event = MoveEvent {
            from:             PathBuf::from("/old/path"),
            to:               PathBuf::from("/new/path"),
            detection_method: DetectionMethod::AtomicRename,
            fingerprint:      None,
            detected_at:      12345,
        };

        let json = serde_json::to_string(&event).expect("serialize");
        let back: MoveEvent = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.from, event.from);
        assert_eq!(back.to,   event.to);
        assert_eq!(back.detection_method, event.detection_method);
        assert_eq!(back.detected_at,      event.detected_at);
    }

    // ── RepoWatcher construction ──
    //
    // We can't easily test the live file-system watch pipeline in a unit test
    // without spawning background threads and introducing race conditions, so
    // we just verify that the watcher can be constructed and that it starts
    // with sane state.

    #[test]
    fn watcher_constructs_without_error() {
        let result = RepoWatcher::new(WatcherConfig::default(), |_event| {});
        // On most platforms this should succeed.  We skip gracefully on those
        // where it doesn't (e.g. certain restricted CI environments).
        match result {
            Ok(w) => {
                assert_eq!(w.watched_paths().len(), 0);
                assert_eq!(w.pending_removes_count(), 0);
            }
            Err(TrackerError::WatcherInit(_)) => {
                eprintln!("notify watcher not available in this environment – skipping");
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn watcher_watch_nonexistent_path_returns_error() {
        let Ok(mut w) = RepoWatcher::new(WatcherConfig::default(), |_| {}) else {
            eprintln!("watcher unavailable – skipping");
            return;
        };

        let result = w.watch(PathBuf::from("/absolutely/does/not/exist/xyz"));
        assert!(
            matches!(result, Err(TrackerError::WatcherRegister { .. })),
            "expected WatcherRegister error, got {:?}",
            result
        );
    }

    #[test]
    fn watcher_watch_records_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let Ok(mut w) = RepoWatcher::new(WatcherConfig::default(), |_| {}) else {
            eprintln!("watcher unavailable – skipping");
            return;
        };

        w.watch(tmp.path().to_path_buf()).expect("watch should succeed");
        assert_eq!(w.watched_paths().len(), 1);
        assert_eq!(w.watched_paths()[0], tmp.path());
    }

    #[test]
    fn watcher_unwatch_removes_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let Ok(mut w) = RepoWatcher::new(WatcherConfig::default(), |_| {}) else {
            eprintln!("watcher unavailable – skipping");
            return;
        };

        w.watch(tmp.path().to_path_buf()).expect("watch");
        w.unwatch(tmp.path()).expect("unwatch");
        assert_eq!(w.watched_paths().len(), 0);
    }
}
