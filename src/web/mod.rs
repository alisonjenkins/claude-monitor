//! The loopback web view behind `claude-monitor serve` (FR-030, FR-031).
//!
//! The only async code in the crate: the runtime starts inside [`start`], so
//! the CLI, the hook and the TUI never pay for it.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::feed::{ChangeFeed, EventSource, SystemMonotonic};

pub mod auth;

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

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub bind: IpAddr,
    pub port: u16,
    pub token: Token,
    pub store_path: PathBuf,
    pub keepalive: Duration,
    /// Stop on SIGINT or SIGTERM; off for tests, which stop the server by hand.
    pub handle_signals: bool,
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
        }
    }
}

/// What every handler can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pub auth: Arc<auth::Auth>,
}

/// The routes behind the guard. Pages and actions are added here; the guard
/// (Host, token, CSRF, `no-store`) wraps whatever this returns.
pub fn routes() -> Router<AppState> {
    Router::new()
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
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
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting and joins the server thread.
    pub fn shutdown(self) {}

    /// Blocks until the server stops.
    pub fn wait(mut self) {
        self.join();
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            // A panicked server thread has nothing left to report here.
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // The receiver is gone once the server has already stopped.
            let _ = stop.send(());
        }
        self.join();
    }
}

/// Binds, then serves on its own thread until [`Server::shutdown`] or, with
/// `handle_signals`, a termination signal.
pub fn start<S: EventSource + Send + 'static>(
    config: WebConfig,
    feed: ChangeFeed<S, SystemMonotonic>,
) -> Result<Server, WebError> {
    drop(feed);
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
    let state = AppState {
        auth: Arc::new(auth::Auth::new(config.token, addr.port())),
    };
    let app = router(state);
    let (stop, stopped) = oneshot::channel();
    let handle_signals = config.handle_signals;
    let thread = std::thread::Builder::new()
        .name("web-server".to_owned())
        .spawn(move || {
            let served = runtime.block_on(async {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown_requested(stopped, handle_signals))
                    .await
            });
            if let Err(error) = served {
                eprintln!("claude-monitor: web server stopped: {error}");
            }
        })
        .map_err(WebError::Thread)?;
    Ok(Server {
        addr,
        stop: Some(stop),
        thread: Some(thread),
    })
}

async fn shutdown_requested(stopped: oneshot::Receiver<()>, handle_signals: bool) {
    if handle_signals {
        tokio::select! {
            _ = stopped => {}
            () = termination_signal() => {}
        }
    } else {
        let _ = stopped.await;
    }
}

async fn termination_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
