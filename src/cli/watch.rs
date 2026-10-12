//! `claude-monitor watch`: log session state changes until SIGINT or SIGTERM.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::migrate::open_viewer_store;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::live::build_sources;
use crate::watch::watch;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    STOP.store(true, Ordering::Release);
}

/// After this, SIGINT and SIGTERM set the returned flag instead of killing the
/// process, so the loop can finish its current pass and exit 0.
fn stop_on_signals() -> &'static AtomicBool {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe {
            libc::signal(signal, handler);
        }
    }
    &STOP
}

pub fn run(env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    let store = open_viewer_store(env, state_dir)?;
    let stop = stop_on_signals();
    let built = build_sources(env);
    watch(
        &store,
        env,
        built,
        stop,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    Ok(Output::empty())
}
