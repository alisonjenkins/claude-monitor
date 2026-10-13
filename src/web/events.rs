//! `GET /events`: server-sent events pushed from the change feed.
//!
//! One thread owns the [`ChangeFeed`] and sleeps in its source; a fired window
//! hands the new rows to every open stream through a broadcast channel, so
//! there is no polling loop. A reconnect with `Last-Event-ID: <seq>` is
//! replayed straight from the events table, and a stream that falls behind
//! the channel catches up from the table the same way, so no change is missed
//! and none is sent twice.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream};
use serde_json::json;
use tokio::sync::{broadcast, watch};

use zeroize::Zeroizing;

use super::auth::{current_session_id, Auth};
use super::AppState;
use crate::feed::{ChangeFeed, EventSource, SystemMonotonic};
use crate::store::events::EventRow;
use crate::store::{Store, StoreOptions};

/// Longest the feed thread sleeps without rechecking whether to stop.
const FEED_SLICE: Duration = Duration::from_millis(200);

/// Pause after a failed read, so a broken store does not spin the thread.
const FEED_RETRY: Duration = Duration::from_secs(1);

/// Rows buffered per stream before it falls back to reading the table.
const BROADCAST_CAPACITY: usize = 1024;

/// Rows read from the table per catch-up query.
const CATCH_UP_PAGE: u32 = 500;

/// How often an idle stream rechecks that its session is still live.
const SESSION_RECHECK: Duration = Duration::from_secs(1);

const KEEPALIVE_TEXT: &str = "keepalive";

#[derive(Debug, Clone)]
pub struct Events {
    sender: broadcast::Sender<Arc<EventRow>>,
    store_path: std::path::PathBuf,
    keepalive: Duration,
    closing: watch::Receiver<bool>,
}

impl Events {
    pub fn new(
        store_path: std::path::PathBuf,
        keepalive: Duration,
        closing: watch::Receiver<bool>,
    ) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            sender,
            store_path,
            keepalive,
            closing,
        }
    }

    /// Moves `feed` onto its own thread; every batch it yields reaches the
    /// open streams. Stops within [`FEED_SLICE`] of `stop` being set.
    pub fn spawn_feed<S: EventSource + Send + 'static>(
        &self,
        mut feed: ChangeFeed<S, SystemMonotonic>,
        stop: Arc<AtomicBool>,
    ) -> io::Result<JoinHandle<()>> {
        let sender = self.sender.clone();
        std::thread::Builder::new()
            .name("web-feed".to_owned())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match feed.wait_poll(FEED_SLICE) {
                        Ok(rows) => {
                            for row in rows {
                                // No receiver just means no stream is open.
                                let _ = sender.send(Arc::new(row));
                            }
                        }
                        Err(error) => {
                            eprintln!("claude-monitor: web: change feed: {error}");
                            std::thread::sleep(FEED_RETRY);
                        }
                    }
                }
            })
    }
}

/// Resolves once `closing` is set (or its sender is gone).
pub async fn closed(mut closing: watch::Receiver<bool>) {
    while !*closing.borrow_and_update() {
        if closing.changed().await.is_err() {
            return;
        }
    }
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let events = state.events;
    let auth = state.auth;
    let session_id = current_session_id().ok_or(StatusCode::UNAUTHORIZED)?;
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|seq| *seq >= 0);
    let receiver = events.sender.subscribe();
    let store_path = events.store_path.clone();
    let opened = tokio::task::spawn_blocking(move || -> Result<(Store, i64), String> {
        let store = Store::open(&store_path, &StoreOptions::viewer()).map_err(|e| e.to_string())?;
        let end = store.last_seq().map_err(|e| e.to_string())?;
        Ok((store, end))
    })
    .await;
    let (store, end) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => {
            eprintln!("claude-monitor: web: events stream: {error}");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Err(error) => {
            eprintln!("claude-monitor: web: events stream: {error}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let subscriber = Subscriber {
        receiver,
        store: Arc::new(Mutex::new(store)),
        last_sent: resume.unwrap_or(end),
        queue: VecDeque::new(),
        catch_up: resume.is_some(),
        closing: events.closing,
        auth,
        session_id,
    };
    let keep_alive = KeepAlive::new()
        .interval(events.keepalive)
        .text(KEEPALIVE_TEXT);
    Ok(Sse::new(stream::unfold(subscriber, Subscriber::next)).keep_alive(keep_alive))
}

/// One open `/events` connection.
struct Subscriber {
    receiver: broadcast::Receiver<Arc<EventRow>>,
    store: Arc<Mutex<Store>>,
    last_sent: i64,
    queue: VecDeque<Arc<EventRow>>,
    /// The table holds rows after `last_sent` that the channel did not carry.
    catch_up: bool,
    closing: watch::Receiver<bool>,
    /// The stream was authenticated once, when it opened; it ends as soon as
    /// that session expires or is evicted.
    auth: Arc<Auth>,
    session_id: Zeroizing<String>,
}

impl Subscriber {
    async fn next(mut self) -> Option<(Result<Event, Infallible>, Self)> {
        loop {
            if !self.auth.session_is_live(&self.session_id) {
                return None;
            }
            if let Some(row) = self.queue.pop_front() {
                self.last_sent = row.seq;
                return Some((Ok(message(&row)), self));
            }
            if self.catch_up {
                self.read_table().await;
                continue;
            }
            tokio::select! {
                received = self.receiver.recv() => match received {
                    Ok(row) if row.seq > self.last_sent => self.queue.push_back(row),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => self.catch_up = true,
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                () = closed(self.closing.clone()) => return None,
                () = tokio::time::sleep(SESSION_RECHECK) => {}
            }
        }
    }

    /// Queues the next page of rows after `last_sent`; more is pending while
    /// pages come back full.
    async fn read_table(&mut self) {
        let store = Arc::clone(&self.store);
        let after = self.last_sent;
        let page = tokio::task::spawn_blocking(move || {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            store.events_since(after, CATCH_UP_PAGE)
        })
        .await;
        match page {
            Ok(Ok(rows)) => {
                self.catch_up = u32::try_from(rows.len()).is_ok_and(|n| n >= CATCH_UP_PAGE);
                self.queue.extend(rows.into_iter().map(Arc::new));
            }
            Ok(Err(error)) => {
                eprintln!("claude-monitor: web: events stream: {error}");
                self.catch_up = false;
            }
            Err(error) => {
                eprintln!("claude-monitor: web: events stream: {error}");
                self.catch_up = false;
            }
        }
    }
}

/// `id` is the row's seq; `data` says exactly what changed.
fn message(row: &EventRow) -> Event {
    let data = json!({
        "changed": [{
            "entity": row.entity_kind,
            "id": row.entity_id,
            "kind": row.kind,
        }]
    });
    Event::default()
        .id(row.seq.to_string())
        .data(data.to_string())
}
