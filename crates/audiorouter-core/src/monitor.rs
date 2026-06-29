//! Shared monitoring primitives for device connectivity and config file changes.
//!
//! Both the TUI main loop (sync, crossterm event loop) and the dashboard API
//! (async, tokio) need to detect when audio devices appear/disappear and when
//! the config file is edited on disk. This module provides the two polling /
//! watching primitives they share, so neither consumer re-implements the logic.
//!
//! Both types are synchronous and runtime-agnostic:
//!
//! - [`DevicePoller`] wraps a rate-limited CPAL enumeration loop and exposes a
//!   simple `poll() -> Option<Vec<String>>` interface.
//! - [`ConfigFileWatcher`] spawns an OS-native file watcher thread (`notify`)
//!   and exposes `poll() -> bool`, matching the existing TUI contract.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::device_inventory::{DevicesResponse, device_diff, list_audio_devices};

// ── DevicePoller ──────────────────────────────────────────────────────────

/// Polls the system audio device inventory at a fixed interval and reports
/// connectivity changes as human-readable event strings.
///
/// Created once and polled repeatedly. The first [`DevicePoller::poll`] after
/// `interval` has elapsed performs the expensive CPAL enumeration; calls
/// before that return `None` immediately.
///
/// **Consumers:**
/// - TUI: call `poll()` every tick (50 ms); the poller internally rate-limits.
/// - Dashboard API: call `poll()` in a `tokio::time::interval` task.
pub struct DevicePoller {
    interval: Duration,
    last_poll: Instant,
    prev: DevicesResponse,
}

impl DevicePoller {
    /// Create a poller with the given minimum interval between CPAL queries.
    ///
    /// On creation, immediately takes a baseline snapshot so the first `poll()`
    /// only reports changes that happen *after* construction.
    pub fn new(interval: Duration) -> Self {
        let prev = list_audio_devices().unwrap_or_default();
        Self {
            interval,
            last_poll: Instant::now(),
            prev,
        }
    }

    /// Check for device changes. Returns `Some(events)` if devices were
    /// added, removed, or changed since the last check (subject to `interval`
    /// rate-limiting), or `None` if nothing changed or the interval hasn't
    /// elapsed yet.
    pub fn poll(&mut self) -> Option<Vec<String>> {
        if self.last_poll.elapsed() < self.interval {
            return None;
        }
        self.last_poll = Instant::now();
        let curr = list_audio_devices().ok()?;
        let events = device_diff(&self.prev, &curr);
        if events.is_empty() {
            return None;
        }
        self.prev = curr;
        Some(events)
    }

    /// The most recent device inventory snapshot (cached, does not query CPAL).
    pub fn snapshot(&self) -> &DevicesResponse {
        &self.prev
    }
}

// ── ConfigFileWatcher ─────────────────────────────────────────────────────

/// Watches the config file for changes using OS-native file notifications.
///
/// Spawns a background thread on construction. When a change is detected,
/// sets an internal flag that the main loop can consume via [`poll`](Self::poll).
///
/// **Consumers:**
/// - TUI: `poll()` in the main event loop to trigger hot-reload.
/// - Dashboard API (future): `poll()` in a loop to emit `ConfigChanged` SSE.
pub struct ConfigFileWatcher {
    config_changed: Arc<AtomicBool>,
}

impl ConfigFileWatcher {
    /// Start watching `config_path`. Returns a watcher handle.
    ///
    /// If the OS file watcher fails to initialise (e.g. sandbox restrictions),
    /// the watcher is silently disabled — `poll()` will always return `false`.
    pub fn new(config_path: &Path) -> Self {
        let config_changed = Arc::new(AtomicBool::new(false));
        let flag = config_changed.clone();
        let watch_path = config_path.to_path_buf();

        std::thread::spawn(move || {
            use notify::{EventKind, RecursiveMode, Watcher};

            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("config watch disabled: {e}");
                    return;
                }
            };

            let canonical_watch_path = std::fs::canonicalize(&watch_path).ok();

            for watch_dir in config_watch_dirs(&watch_path, canonical_watch_path.as_deref()) {
                if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                    tracing::warn!("config watch disabled: {e}");
                    return;
                }
            }

            for event in rx.into_iter().flatten() {
                let is_config_event = config_event_matches(
                    &event.paths,
                    &watch_path,
                    canonical_watch_path.as_deref(),
                );
                if !is_config_event {
                    continue;
                }
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    flag.store(true, Ordering::SeqCst);
                }
            }
        });

        Self { config_changed }
    }

    /// Check (and consume) the config-changed flag.
    pub fn poll(&self) -> bool {
        self.config_changed.swap(false, Ordering::SeqCst)
    }
}

// ── Path helpers (moved from audio.rs) ────────────────────────────────────

/// Collect directories to watch for the config file.
///
/// We watch the parent directory of the config path (and its canonical
/// equivalent when the path is a symlink) because FSEvents/inotify report
/// changes on directory entries, not on file descriptors.
fn config_watch_dirs(watch_path: &Path, canonical_watch_path: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique_path(
        &mut dirs,
        watch_path.parent().unwrap_or(Path::new(".")).to_path_buf(),
    );
    if let Some(canonical_watch_path) = canonical_watch_path {
        push_unique_path(
            &mut dirs,
            canonical_watch_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
        );
    }
    // Remove duplicates (symlink dir may equal real dir).
    let mut seen: HashSet<PathBuf> = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

fn config_event_matches(
    event_paths: &[PathBuf],
    watch_path: &Path,
    canonical_watch_path: Option<&Path>,
) -> bool {
    event_paths
        .iter()
        .any(|p| p == watch_path || canonical_watch_path.is_some_and(|canonical| p == canonical))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_symlink_watches_and_matches_real_target_file() {
        let link_path = PathBuf::from("/tmp/audiorouter-test/link/config.toml");
        let target_path = PathBuf::from("/tmp/audiorouter-test/real/config.toml");
        let dirs = config_watch_dirs(&link_path, Some(&target_path));
        assert!(dirs.iter().any(|d| d.ends_with("link")));
        assert!(dirs.iter().any(|d| d.ends_with("real")));
    }

    #[test]
    fn config_event_matches_direct_and_canonical() {
        let watch = Path::new("/tmp/audiorouter-test/config.toml");
        let canonical = Path::new("/tmp/audiorouter-test/real/config.toml");

        assert!(config_event_matches(&[watch.to_path_buf()], watch, None));
        assert!(config_event_matches(
            &[canonical.to_path_buf()],
            watch,
            Some(canonical)
        ));
        assert!(!config_event_matches(
            &[PathBuf::from("/tmp/other.toml")],
            watch,
            Some(canonical)
        ));
    }
}
