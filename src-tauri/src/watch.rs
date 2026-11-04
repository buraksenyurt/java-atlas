//! Watching the project folder (phase 8).
//!
//! `notify` turns the operating system's file events (inotify on Linux,
//! `ReadDirectoryChangesW` on Windows, FSEvents on macOS) into one stream.
//! That stream is noisy: an editor saving one file can produce four events
//! (create a temp file, write, rename over the original, change metadata),
//! and `mvn compile` produces thousands. So events are *debounced*: they are
//! collected until the folder has been quiet for a moment, and handed over
//! as one batch of distinct paths.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// How long the folder must stay quiet before a batch is delivered.
const QUIET: Duration = Duration::from_millis(300);
/// A batch is delivered after this long even if events keep coming
/// (a long build in a watched folder must not starve the map).
const MAX_WAIT: Duration = Duration::from_secs(2);

/// Distinct changed paths, collected over one quiet period.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
    pub paths: BTreeSet<PathBuf>,
    /// The operating system lost events (its queue overflowed): nobody
    /// knows what changed, only a full scan is safe.
    pub rescan: bool,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.rescan
    }

    pub fn merge(&mut self, other: Batch) {
        self.paths.extend(other.paths);
        self.rescan |= other.rescan;
    }

    fn add(&mut self, event: notify::Result<notify::Event>) {
        match event {
            Ok(event) => {
                if event.need_rescan() {
                    self.rescan = true;
                }
                // Reading a file is not changing it.
                if !matches!(event.kind, EventKind::Access(_)) {
                    self.paths.extend(event.paths);
                }
            }
            // An error from the backend usually means lost events as well.
            Err(_) => self.rescan = true,
        }
    }
}

/// A running watch. Dropping it stops the operating system watch; the
/// debounce thread then sees its channel close and ends by itself.
pub struct Watch {
    _watcher: RecommendedWatcher,
    pub root: PathBuf,
}

/// Starts watching `root` recursively. `on_batch` runs on a dedicated
/// thread, one batch at a time; while it runs, new events wait in the
/// channel and form the next batch.
pub fn start(root: PathBuf, on_batch: impl Fn(Batch) + Send + 'static) -> notify::Result<Watch> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    // The closure runs on notify's own thread: just forward, never block.
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    thread::Builder::new()
        .name("java-atlas-watch".into())
        .spawn(move || {
            // `recv` fails once the watcher (and with it `tx`) is dropped.
            while let Ok(first) = rx.recv() {
                let mut batch = Batch::default();
                batch.add(first);
                let started = Instant::now();
                loop {
                    let left = MAX_WAIT.saturating_sub(started.elapsed());
                    if left.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(QUIET.min(left)) {
                        Ok(event) => batch.add(event),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                if !batch.is_empty() {
                    on_batch(batch);
                }
            }
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;

    Ok(Watch {
        _watcher: watcher,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};

    /// The real thing: write files into a temporary folder and expect them
    /// to arrive as one batch.
    #[test]
    fn a_burst_of_writes_arrives_as_one_batch() {
        let dir = std::env::temp_dir().join(format!("java-atlas-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let batches = Arc::new(Mutex::new(Vec::<Batch>::new()));
        let seen = Arc::clone(&batches);
        let watch = start(dir.clone(), move |b| seen.lock().unwrap().push(b)).unwrap();

        for i in 0..5 {
            fs::write(dir.join(format!("A{i}.java")), "class A {}").unwrap();
            thread::sleep(Duration::from_millis(20));
        }
        // Quiet period + generous slack for slow CI machines.
        let deadline = Instant::now() + Duration::from_secs(5);
        while batches.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        drop(watch);
        let _ = fs::remove_dir_all(&dir);

        let batches = batches.lock().unwrap();
        assert_eq!(batches.len(), 1, "{batches:?}");
        let names: BTreeSet<String> = batches[0]
            .paths
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(
            (0..5).all(|i| names.contains(&format!("A{i}.java"))),
            "{names:?}"
        );
    }
}
