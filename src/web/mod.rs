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
use std::time::{Duration, SystemTime};

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use crate::env::Env;
use crate::feed::{ChangeFeed, EventSource, SystemMonotonic};

pub mod auth;
pub mod clock;
pub mod control;
pub mod events;
pub mod login;
pub mod pages;
pub mod routes;
pub mod secret;
pub mod session;
pub mod throttle;
pub mod tls;
pub mod tree;

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
    #[error("TLS setup failed: {0}")]
    Tls(#[from] tls::TlsError),
    #[error("{0}")]
    Control(#[from] control::ControlError),
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
    pub store_path: PathBuf,
    pub keepalive: Duration,
    /// Stop on SIGINT or SIGTERM; off for tests, which stop the server by hand.
    pub handle_signals: bool,
    /// The providers every action runs against; tests swap in a private tmux.
    pub env: Env,
    /// Time for login-code, session and throttle expiry; tests move it by hand.
    pub clock: Arc<dyn clock::Ticks>,
    /// Failed logins tolerated per window.
    pub max_auth_failures: usize,
    pub failure_window: Duration,
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
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
            store_path,
            keepalive: DEFAULT_KEEPALIVE,
            handle_signals: false,
            env: Env::real(),
            clock: Arc::new(clock::MonotonicTicks::new()),
            max_auth_failures: throttle::DEFAULT_MAX_FAILURES,
            failure_window: throttle::DEFAULT_WINDOW,
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
/// (Host, session, CSRF, `no-store`) wraps whatever this returns.
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
    auth: Arc<auth::Auth>,
    closing: Arc<watch::Sender<bool>>,
    feed_stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    feed: Option<JoinHandle<()>>,
}

impl Server {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A fresh single-use login code, good for five minutes.
    pub fn issue_login_code(&self) -> Result<zeroize::Zeroizing<String>, WebError> {
        self.auth.issue_login_code()
    }

    /// The URL that logs a browser in, once. It holds the only secret that
    /// ever appears in a URL.
    pub fn issue_login_url(&self) -> Result<zeroize::Zeroizing<String>, WebError> {
        let code = self.issue_login_code()?;
        Ok(auth::login_url(self.addr, code.as_str()))
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
    let tls_dir = config
        .store_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let tls_paths = tls::ensure(&tls_dir, SystemTime::now())?;
    let acceptor = TlsAcceptor::from(tls::server_config(&tls_paths)?);
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
    // The control socket only serves `web login`; a path too long for a unix
    // socket must not stop the web view itself.
    let control = {
        let _entered = runtime.enter();
        match control::bind(&tls_dir, &tls_paths.dir) {
            Ok(Some(socket)) => Some(socket),
            Ok(None) => {
                eprintln!(
                    "claude-monitor: another serve owns {}; `web login` will reach that one",
                    control::socket_path(&tls_dir).display()
                );
                None
            }
            Err(error) => {
                eprintln!("claude-monitor: no control socket, `web login` will not work: {error}");
                None
            }
        }
    };
    let (closing, closing_rx) = watch::channel(false);
    let closing = Arc::new(closing);
    let state_dir = config
        .store_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let events = events::Events::new(config.store_path, config.keepalive, closing_rx);
    let auth = Arc::new(auth::Auth::new(
        addr.port(),
        config.clock,
        config.max_auth_failures,
        config.failure_window,
    ));
    let state = AppState {
        auth: Arc::clone(&auth),
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
    let control_auth = Arc::clone(&auth);
    let server_thread = std::thread::Builder::new()
        .name("web-server".to_owned())
        .spawn(move || {
            runtime.block_on(async {
                let control_task = control.map(|socket| {
                    tokio::spawn(control::serve(
                        socket,
                        control_auth,
                        addr,
                        serving.subscribe(),
                    ))
                });
                serve_tls(listener, acceptor, app, serving, signals).await;
                if let Some(task) = control_task {
                    let _ = task.await;
                }
            });
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
        auth,
        closing,
        feed_stop,
        server: Some(server_thread),
        feed: Some(feed_thread),
    })
}

/// A client that opens a socket and never finishes the handshake or the
/// request headers is cut off after this long.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections served at once; the rest wait in the kernel's accept queue.
const MAX_CONNECTIONS: usize = 128;
/// How long shutdown waits for open connections to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Accepts TCP, completes the TLS 1.3 handshake, then speaks HTTP/1.1 over it.
/// A peer that fails the handshake never reaches the HTTP parser.
async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: Router,
    closing: Arc<watch::Sender<bool>>,
    signals: Option<Signals>,
) {
    let stop = shutdown_requested(Arc::clone(&closing), signals);
    tokio::pin!(stop);
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        let permit = tokio::select! {
            () = &mut stop => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            permit = Arc::clone(&slots).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        let accepted = tokio::select! {
            () = &mut stop => break,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((tcp, _peer)) => {
                connections.spawn(serve_connection(
                    tcp,
                    acceptor.clone(),
                    app.clone(),
                    closing.subscribe(),
                    permit,
                ));
            }
            Err(error) => {
                eprintln!("claude-monitor: web accept failed: {error}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    drop(listener);
    let drain = async { while connections.join_next().await.is_some() {} };
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, drain).await;
}

async fn serve_connection(
    tcp: tokio::net::TcpStream,
    acceptor: TlsAcceptor,
    app: Router,
    closing: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
) {
    let Ok(Ok(stream)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await else {
        return;
    };
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT);
    let connection = http.serve_connection(TokioIo::new(stream), TowerToHyperService::new(app));
    tokio::pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        () = events::closed(closing) => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
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
