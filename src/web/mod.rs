//! The loopback web view behind `claude-monitor serve` (FR-030, FR-031).
//!
//! The only async code in the crate: the runtime starts inside [`start`], so
//! the CLI, the hook and the TUI never pay for it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

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
    #[error("web server is not implemented yet")]
    Unimplemented,
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

#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
}

impl Server {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting, ends open streams and joins the server threads.
    pub fn shutdown(self) {}

    /// Blocks until the server stops.
    pub fn wait(self) {}
}

/// Binds, then serves on its own threads until [`Server::shutdown`] or, with
/// `handle_signals`, a termination signal.
pub fn start<S: EventSource + Send + 'static>(
    config: WebConfig,
    feed: ChangeFeed<S, SystemMonotonic>,
) -> Result<Server, WebError> {
    let _ = (config, feed);
    Err(WebError::Unimplemented)
}
