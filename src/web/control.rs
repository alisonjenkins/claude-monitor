//! The control socket: how `claude-monitor web login` asks a running `serve`
//! for a fresh login URL without restarting it.
//!
//! A unix-domain socket at `<state-dir>/web.sock`, mode 0600, reachable only
//! from this machine and, by permission and by a peer-uid check, only by this
//! user. One request exists: the line `login`, answered by `ok <url>`.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdListener, UnixStream as StdStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::auth::{login_url, Auth};
use super::events;

pub const SOCKET_NAME: &str = "web.sock";

const SOCKET_MODE: u32 = 0o600;
const REQUEST: &str = "login";
/// Longest request line read; the only valid one is five bytes.
const MAX_REQUEST_BYTES: u64 = 64;
/// A peer that connects and says nothing is dropped after this long.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SOCKET_NAME)
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("cannot create the control socket {path}: {source}")]
    Bind { path: PathBuf, source: io::Error },
    #[error(
        "no `claude-monitor serve` is answering on {path} ({source}); start one with `claude-monitor serve`"
    )]
    NotRunning { path: PathBuf, source: io::Error },
    #[error("the control socket {path} failed: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("serve refused the request: {0}")]
    Refused(String),
    #[error("serve sent a reply this version does not understand")]
    BadReply,
}

/// A bound control socket; the file goes away when this does.
#[derive(Debug)]
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
    owner_uid: u32,
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Binds `<state-dir>/web.sock` with mode 0600, or returns `None` when another
/// live server already owns it (that server keeps answering; this one runs
/// without a control socket). Must run inside a tokio runtime context.
///
/// The socket is bound inside `private_dir` (mode 0700) and renamed into
/// place after the chmod, so it is never connectable by anyone else, not even
/// for the instant between bind and chmod.
pub fn bind(state_dir: &Path, private_dir: &Path) -> Result<Option<ControlSocket>, ControlError> {
    let path = socket_path(state_dir);
    let fail = |source| ControlError::Bind {
        path: path.clone(),
        source,
    };
    if StdStream::connect(&path).is_ok() {
        return Ok(None);
    }
    let staging = private_dir.join(format!("{SOCKET_NAME}.{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    let listener = StdListener::bind(&staging).map_err(fail)?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(SOCKET_MODE)).map_err(fail)?;
    let owner_uid = fs::metadata(&staging).map_err(fail)?.uid();
    fs::rename(&staging, &path).map_err(fail)?;
    listener.set_nonblocking(true).map_err(fail)?;
    let listener = UnixListener::from_std(listener).map_err(fail)?;
    Ok(Some(ControlSocket {
        listener,
        path,
        owner_uid,
    }))
}

/// Answers requests until the server is told to stop.
pub async fn serve(
    socket: ControlSocket,
    auth: Arc<Auth>,
    addr: SocketAddr,
    closing: watch::Receiver<bool>,
) {
    let stop = events::closed(closing);
    tokio::pin!(stop);
    loop {
        let accepted = tokio::select! {
            () = &mut stop => break,
            accepted = socket.listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let _ = tokio::time::timeout(IO_TIMEOUT, answer(stream, &socket, &auth, addr)).await;
    }
}

async fn answer(
    mut stream: tokio::net::UnixStream,
    socket: &ControlSocket,
    auth: &Auth,
    addr: SocketAddr,
) {
    let reply = match stream.peer_cred() {
        Ok(peer) if peer.uid() == socket.owner_uid => respond(&mut stream, auth, addr).await,
        _ => Zeroizing::new("err not permitted\n".to_owned()),
    };
    let _ = stream.write_all(reply.as_bytes()).await;
}

async fn respond(
    stream: &mut tokio::net::UnixStream,
    auth: &Auth,
    addr: SocketAddr,
) -> Zeroizing<String> {
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(stream.take(MAX_REQUEST_BYTES));
    if reader.read_line(&mut line).await.is_err() || line.trim_end() != REQUEST {
        return Zeroizing::new("err unknown request\n".to_owned());
    }
    match auth.issue_login_code() {
        Ok(code) => Zeroizing::new(format!("ok {}\n", login_url(addr, code.as_str()).as_str())),
        Err(_) => Zeroizing::new("err cannot make a login code\n".to_owned()),
    }
}

/// Asks the server running for `state_dir` for a fresh single-use login URL.
pub fn request_login_url(state_dir: &Path) -> Result<Zeroizing<String>, ControlError> {
    let path = socket_path(state_dir);
    let io_error = |source| ControlError::Io {
        path: path.clone(),
        source,
    };
    let mut stream = StdStream::connect(&path).map_err(|source| ControlError::NotRunning {
        path: path.clone(),
        source,
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(io_error)?;
    writeln!(stream, "{REQUEST}").map_err(io_error)?;
    let mut reply = Zeroizing::new(String::new());
    BufReader::new(stream.take(MAX_REQUEST_BYTES.saturating_mul(16)))
        .read_line(&mut reply)
        .map_err(io_error)?;
    let reply = reply.trim_end();
    if let Some(url) = reply.strip_prefix("ok ") {
        return Ok(Zeroizing::new(url.to_owned()));
    }
    match reply.strip_prefix("err ") {
        Some(reason) => Err(ControlError::Refused(reason.to_owned())),
        None => Err(ControlError::BadReply),
    }
}
