//! Fan-in of several event sources into one, so a viewer waits on the file
//! watcher, the registry, process exits and the safety net with a single
//! `wait`.
//!
//! Each source gets a forwarding thread that blocks on its own `wait`. The
//! threads wait in slices of [`SOURCE_SLICE`] and check a shutdown flag between
//! slices, so dropping the merged source joins them within one slice.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::EventSource;

/// Longest a forwarding thread blocks in its source before it rechecks the
/// shutdown flag; bounds how long a drop takes.
pub const SOURCE_SLICE: Duration = Duration::from_millis(250);

/// One queued wake is enough: it means "re-read", and more would only repeat it.
const WAKE_QUEUE_DEPTH: usize = 1;

pub struct MergedSource {
    wakes: Receiver<()>,
    /// Keeps the channel open when there are no sources, so `wait` blocks for
    /// its timeout instead of returning at once.
    _idle: SyncSender<()>,
    shutdown: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// A source whose thread could not start is never heard from, so every
    /// timeout counts as a wake and the caller re-reads on its own cadence.
    lost_source: bool,
}

fn forward(mut source: Box<dyn EventSource + Send>, tx: SyncSender<()>, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Acquire) {
        if source.wait(SOURCE_SLICE) {
            // Full means a wake is already queued; that covers this one.
            if let Err(TrySendError::Disconnected(())) = tx.try_send(()) {
                break;
            }
        }
    }
}

impl MergedSource {
    pub fn new(sources: Vec<Box<dyn EventSource + Send>>) -> Self {
        let (tx, wakes) = mpsc::sync_channel(WAKE_QUEUE_DEPTH);
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::with_capacity(sources.len());
        let mut lost_source = false;
        for source in sources {
            let tx = tx.clone();
            let flag = Arc::clone(&shutdown);
            let spawned = thread::Builder::new()
                .name("feed-merge".to_owned())
                .spawn(move || forward(source, tx, &flag));
            match spawned {
                Ok(handle) => threads.push(handle),
                Err(_) => lost_source = true,
            }
        }
        Self {
            wakes,
            _idle: tx,
            shutdown,
            threads,
            lost_source,
        }
    }
}

impl EventSource for MergedSource {
    fn wait(&mut self, timeout: Duration) -> bool {
        let woken = self.wakes.recv_timeout(timeout).is_ok();
        while self.wakes.try_recv().is_ok() {}
        woken || self.lost_source
    }
}

impl Drop for MergedSource {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        for handle in self.threads.drain(..) {
            // A panicked source thread has nothing left to report here.
            let _ = handle.join();
        }
    }
}
