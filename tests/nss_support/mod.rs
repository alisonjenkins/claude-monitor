//! Shared by the tests that run the real `certutil` and `pk12util`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

pub const SKIP_VAR: &str = "CM_SKIP_NSS_TESTS";

/// True when the NSS tools are on PATH. Without them the test fails with the
/// install hint, unless `CM_SKIP_NSS_TESTS=1` says skipping is intended.
pub fn nss_tools_or_skip() -> bool {
    let missing = claude_monitor::browser_setup::runner::missing_tools(
        std::env::var_os("PATH").unwrap_or_default().as_os_str(),
    );
    if missing.is_empty() {
        return true;
    }
    if std::env::var(SKIP_VAR).as_deref() == Ok("1") {
        eprintln!("skipping: {missing:?} not on PATH ({SKIP_VAR}=1)");
        return false;
    }
    panic!(
        "{missing:?} not on PATH; run the tests under `{}` (or set {SKIP_VAR}=1 to skip them)",
        claude_monitor::browser_setup::runner::NSS_INSTALL_HINT
    );
}

fn tool(program: &str, args: &[&OsStr]) -> std::process::Output {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn sql(dir: &Path) -> std::ffi::OsString {
    let mut arg = std::ffi::OsString::from("sql:");
    arg.push(dir);
    arg
}

/// An empty NSS database, with no primary password.
pub fn new_db(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    tool(
        "certutil",
        &[
            "-N".as_ref(),
            "-d".as_ref(),
            &sql(dir),
            "--empty-password".as_ref(),
        ],
    );
}

/// An NSS database protected by a primary password.
pub fn new_db_with_password(dir: &Path, password_file: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    tool(
        "certutil",
        &[
            "-N".as_ref(),
            "-d".as_ref(),
            &sql(dir),
            "-f".as_ref(),
            password_file.as_os_str(),
        ],
    );
}

/// `certutil -L` output: one line per certificate, nickname first.
pub fn list(dir: &Path) -> String {
    let out = tool("certutil", &["-L".as_ref(), "-d".as_ref(), &sql(dir)]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `certutil -K` output: one line per private key.
pub fn keys(dir: &Path, password_file: &Path) -> String {
    let out = Command::new("certutil")
        .args(["-K", "-d"])
        .arg(sql(dir))
        .arg("-f")
        .arg(password_file)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}
