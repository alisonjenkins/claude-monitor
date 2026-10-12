//! The loopback web view behind `claude-monitor serve` (FR-030, FR-031).
//!
//! The only async code in the crate: the runtime starts inside [`start`], so
//! the CLI, the hook and the TUI never pay for it.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio::sync::watch;

use crate::env::Env;
use crate::feed::{ChangeFeed, EventSource, SystemMonotonic};

pub mod auth;
pub mod events;
pub mod pages;
pub mod routes;

pub use auth::Token;

/// Where `serve` listens unless told otherwise.
pub const DEFAULT_PORT: u16 = 8765;

/// A comment line this often keeps proxies and the connection from idling out.
pub const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(25);

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("the web view only binds to a loopback address, not {addr}")]
    NotLoopback { addr: IpAddr },
    #[error("cannot read the OS random number generator: {detail}")]
    Random { detail: String },
    #[error("cannot write the token file {path}: {source}")]
    TokenFile { path: PathBuf, source: io::Error },
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot listen on {addr}: {source}")]
    Bind { addr: SocketAddr, source: io::Error },
    #[error("cannot start the server thread: {0}")]
    Thread(io::Error),
}

#[derive(Clone)]
pub struct WebConfig {
    pub bind: IpAddr,
    pub port: u16,
    pub token: Token,
    pub store_path: PathBuf,
    pub keepalive: Duration,
    /// Stop on SIGINT or SIGTERM; off for tests, which stop the server by hand.
    pub handle_signals: bool,
    /// The providers every action runs against; tests swap in a private tmux.
    pub env: Env,
}

impl fmt::Debug for WebConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebConfig")
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field("store_path", &self.store_path)
            .field("keepalive", &self.keepalive)
            .field("handle_signals", &self.handle_signals)
            .finish_non_exhaustive()
    }
}

impl WebConfig {
    pub fn new(token: Token, store_path: PathBuf) -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
            token,
            store_path,
            keepalive: DEFAULT_KEEPALIVE,
            handle_signals: false,
            env: Env::real(),
        }
    }
}

/// What every handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<auth::Auth>,
    pub events: events::Events,
    pub env: Env,
    /// The directory holding `state.db`, as `--state-dir` would name it.
    pub state_dir: PathBuf,
}

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("state_dir", &self.state_dir)
            .finish_non_exhaustive()
    }
}

/// The routes behind the guard. Pages and actions are added here; the guard
/// (Host, token, CSRF, `no-store`) wraps whatever this returns.
pub fn routes() -> Router<AppState> {
    routes::router()
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/events", get(events::events))
        .merge(routes())
        .with_state(state.clone())
        .layer(from_fn_with_state(state, auth::guard))
}

async fn healthz() -> &'static str {
    "ok\n"
}

#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
    closing: Arc<watch::Sender<bool>>,
    feed_stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    feed: Option<JoinHandle<()>>,
}

impl Server {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting and joins the server thread.
    pub fn shutdown(self) {}

    /// Blocks until the server stops.
    pub fn wait(mut self) {
        join(self.server.take());
    }
}

fn join(thread: Option<JoinHandle<()>>) {
    if let Some(thread) = thread {
        // A panicked server thread has nothing left to report here.
        let _ = thread.join();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.feed_stop.store(true, Ordering::Release);
        self.closing.send_replace(true);
        join(self.server.take());
        join(self.feed.take());
    }
}

/// Binds, then serves on its own thread until [`Server::shutdown`] or, with
/// `handle_signals`, a termination signal.
pub fn start<S: EventSource + Send + 'static>(
    config: WebConfig,
    feed: ChangeFeed<S, SystemMonotonic>,
) -> Result<Server, WebError> {
    if !config.bind.is_loopback() {
        return Err(WebError::NotLoopback { addr: config.bind });
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(WebError::Runtime)?;
    let wanted = SocketAddr::new(config.bind, config.port);
    let listener = runtime
        .block_on(TcpListener::bind(wanted))
        .map_err(|source| WebError::Bind {
            addr: wanted,
            source,
        })?;
    let addr = listener.local_addr().map_err(|source| WebError::Bind {
        addr: wanted,
        source,
    })?;
    let signals = if config.handle_signals {
        Some(install_signals(&runtime)?)
    } else {
        None
    };
    let (closing, closing_rx) = watch::channel(false);
    let closing = Arc::new(closing);
    let state_dir = config
        .store_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let events = events::Events::new(config.store_path, config.keepalive, closing_rx);
    let state = AppState {
        auth: Arc::new(auth::Auth::new(config.token, addr.port())),
        events: events.clone(),
        env: config.env,
        state_dir,
    };
    let app = router(state);
    let feed_stop = Arc::new(AtomicBool::new(false));
    let feed_thread = events
        .spawn_feed(feed, Arc::clone(&feed_stop))
        .map_err(WebError::Thread)?;
    let serving = Arc::clone(&closing);
    let server_thread = std::thread::Builder::new()
        .name("web-server".to_owned())
        .spawn(move || {
            let served = runtime.block_on(async {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown_requested(serving, signals))
                    .await
            });
            if let Err(error) = served {
                eprintln!("claude-monitor: web server stopped: {error}");
            }
        })
        .map_err(WebError::Thread);
    let server_thread = match server_thread {
        Ok(thread) => thread,
        Err(error) => {
            feed_stop.store(true, Ordering::Release);
            let _ = feed_thread.join();
            return Err(error);
        }
    };
    Ok(Server {
        addr,
        closing,
        feed_stop,
        server: Some(server_thread),
        feed: Some(feed_thread),
    })
}

struct Signals {
    interrupt: Signal,
    terminate: Signal,
}

/// Installed before [`start`] returns, so a signal sent right after the caller
/// learns the address already stops the server instead of killing the process.
fn install_signals(runtime: &tokio::runtime::Runtime) -> Result<Signals, WebError> {
    let _entered = runtime.enter();
    Ok(Signals {
        interrupt: signal(SignalKind::interrupt()).map_err(WebError::Runtime)?,
        terminate: signal(SignalKind::terminate()).map_err(WebError::Runtime)?,
    })
}

/// Resolves when the server is told to stop, or when a signal arrives, which
/// also ends the open event streams.
async fn shutdown_requested(closing: Arc<watch::Sender<bool>>, signals: Option<Signals>) {
    let stopped = events::closed(closing.subscribe());
    let Some(mut signals) = signals else {
        stopped.await;
        return;
    };
    tokio::select! {
        () = stopped => {}
        _ = signals.interrupt.recv() => {
            closing.send_replace(true);
        }
        _ = signals.terminate.recv() => {
            closing.send_replace(true);
        }
    }
}
