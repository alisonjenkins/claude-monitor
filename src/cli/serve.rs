//! `claude-monitor serve`: the loopback web view until SIGINT or SIGTERM.

use std::net::IpAddr;
use std::path::Path;

use super::Output;
use crate::env::Env;
use crate::error::Error;

/// The address `--bind` accepts: loopback or nothing.
pub fn loopback_addr(text: &str) -> Result<IpAddr, String> {
    let addr: IpAddr = text.parse().map_err(|_| {
        format!("{text:?} is not an IP address; only a loopback address is allowed")
    })?;
    if addr.is_loopback() {
        Ok(addr)
    } else {
        Err(format!(
            "{addr} is not a loopback address; the web view only listens on 127.0.0.1 or ::1"
        ))
    }
}

#[cfg(feature = "web")]
pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    port: Option<u16>,
    bind: Option<IpAddr>,
) -> Result<Output, Error> {
    enabled::run(env, state_dir, port, bind)
}

#[cfg(not(feature = "web"))]
pub fn run(
    _env: &Env,
    _state_dir: Option<&Path>,
    _port: Option<u16>,
    _bind: Option<IpAddr>,
) -> Result<Output, Error> {
    Err(Error::Usage(
        "serve needs the `web` feature; rebuild with `--features web`".to_owned(),
    ))
}

#[cfg(feature = "web")]
mod enabled {
    use std::io::Write;
    use std::net::IpAddr;
    use std::path::Path;

    use crate::cli::migrate::open_viewer_store;
    use crate::cli::Output;
    use crate::env::Env;
    use crate::error::Error;
    use crate::feed::{ChangeFeed, FsWatcherSource, SystemMonotonic};
    use crate::live::{build_sources, spawn_live};
    use crate::store::{Store, StoreOptions};
    use crate::web::{self, auth, WebConfig, WebError};

    fn failed(operation: &str, source: impl std::fmt::Display) -> Error {
        Error::Io {
            operation: operation.to_owned(),
            path: None,
            source: std::io::Error::other(source.to_string()),
        }
    }

    fn web_error(operation: &str, error: WebError) -> Error {
        match error {
            WebError::NotLoopback { .. } => Error::Usage(error.to_string()),
            other => failed(operation, other),
        }
    }

    pub fn run(
        env: &Env,
        state_dir: Option<&Path>,
        port: Option<u16>,
        bind: Option<IpAddr>,
    ) -> Result<Output, Error> {
        let store = open_viewer_store(env, state_dir)?;
        let db = store.path().to_owned();
        let dir = db
            .parent()
            .ok_or_else(|| failed("find the state directory", db.display()))?
            .to_owned();
        let viewer = |what: &str| {
            Store::open(&db, &StoreOptions::viewer())
                .map_err(|e| failed(&format!("open the {what} store"), e))
        };

        let source =
            FsWatcherSource::watch(&dir).map_err(|e| failed("watch the state directory", e))?;
        let feed = ChangeFeed::new(viewer("change feed")?, source, SystemMonotonic)
            .map_err(|e| failed("start the change feed", e))?;

        let built = build_sources(env);
        for warning in &built.warnings {
            eprintln!("claude-monitor: serve: {warning}");
        }
        let _live = spawn_live(
            viewer("live refresh")?,
            env.clone(),
            built.sources,
            built.proc_exit,
            |_| {},
        )
        .map_err(|e| failed("start the live refresh", e))?;

        let token =
            auth::issue_token_file(&dir).map_err(|e| web_error("write the web token", e))?;
        let mut config = WebConfig::new(token.clone(), db);
        config.port = port.unwrap_or(web::DEFAULT_PORT);
        if let Some(bind) = bind {
            config.bind = bind;
        }
        config.handle_signals = true;
        let server = web::start(config, feed).map_err(|e| web_error("start the web server", e))?;

        let mut out = std::io::stdout().lock();
        let printed = writeln!(out, "http://{}/?token={}", server.addr(), token.as_str())
            .and_then(|()| out.flush());
        drop(out);
        if let Err(error) = printed {
            return Err(failed("print the web address", error));
        }
        server.wait();
        Ok(Output::empty())
    }
}
