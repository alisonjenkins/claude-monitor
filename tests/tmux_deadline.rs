#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The hook asks tmux for its pane's location; a wedged tmux must not stall it.
//! One test per binary: it replaces `PATH` for the whole process.

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use claude_monitor::env::{SystemTmux, Tmux};

#[test]
fn a_wedged_tmux_is_abandoned_after_the_deadline() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("tmux");
    std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut path = dir.path().as_os_str().to_owned();
    path.push(":");
    path.push(&original);
    std::env::set_var("PATH", path);

    let started = Instant::now();
    let result = SystemTmux::default().pane_location("%1");
    let elapsed = started.elapsed();

    assert!(result.is_err(), "{result:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "pane_location waited {elapsed:?} on a wedged tmux"
    );
}
