//! FR-020: session content never leaves the machine.
//!
//! Three checks, strongest first, and what each cannot prove:
//!
//! 1. `every_command_runs_with_no_network_at_all` runs the real binary for
//!    every `Command::all()` entry except `dashboard` (needs a terminal),
//!    `watch` (runs until signalled) and `serve` (listens; see 3) inside a
//!    fresh network namespace (`unshare -rn`): no interface, not even
//!    loopback, so a connection cannot succeed. `PATH` holds only recording
//!    stubs for the usual exfiltration clients (curl, wget, nc, ssh, ...); a
//!    stub being run fails the test. Limit: a command that tries to connect,
//!    fails and carries on is not detected by the namespace alone, which is
//!    what checks 2 and 3 cover. Needs unprivileged user namespaces; when the
//!    host (some build sandboxes) denies them the test skips with a message,
//!    or fails if `CLAUDE_MONITOR_REQUIRE_NETNS` is set.
//! 2. `only_the_allowed_files_open_sockets` is a source scan: socket types
//!    and HTTP client crates appear only in `src/summary.rs` (loopback-checked
//!    endpoint) and `src/web/` (the loopback listener), the only external
//!    programs the code names are tmux, sysctl and hostname, and the
//!    dependency lock has no HTTP client or TLS client stack (rustls is there
//!    only to serve, and `TlsConnector` and `ClientConnection` are banned from
//!    `src/`, including the web module). Limit: textual, so a
//!    new dependency spelled differently is caught only by the lock check.
//! 3. The serve and export tests run the real binary on loopback and check
//!    that a distinctive piece of session text is written to no file outside
//!    the store, except the `--out` file.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod export_support;
#[cfg(feature = "web")]
mod web_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};

use claude_monitor::cli::Command;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");
const FORBIDDEN_CLIENTS: &[&str] = &[
    "curl", "wget", "nc", "ncat", "socat", "telnet", "ssh", "scp", "ftp",
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn find_in_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A directory of stubs; the forbidden ones append their name to `ran.log`.
fn stub_dir(log: &Path) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in FORBIDDEN_CLIENTS {
        write_executable(
            &dir.path().join(name),
            &format!("#!/bin/sh\necho {name} >> '{}'\nexit 1\n", log.display()),
        );
    }
    write_executable(&dir.path().join("cortexiphan"), "#!/bin/sh\nexit 1\n");
    dir
}

fn netns_available(unshare: &Path) -> bool {
    Process::new(unshare)
        .args(["-rn", "true"])
        .status()
        .is_ok_and(|s| s.success())
}

/// The CLI arguments of each runnable command's example, as `contracts/cli.md`
/// spells them. `None` for a command this test deliberately does not run.
fn argv(name: &str) -> Option<Vec<&'static str>> {
    let args: &[&str] = match name {
        "dashboard" | "watch" | "serve" => return None,
        "list" => &["list"],
        "show" => &["show", "sess-a"],
        "hook" => &["hook"],
        "reconcile" => &["reconcile"],
        "restore" => &["restore", "--all-interrupted"],
        "respawn" => &["respawn", "sess-c"],
        "boot-check" => &["boot-check"],
        "project" => &["project", "add", "p1"],
        "group" => &["group", "ai-control-centre", "sess-a"],
        "note" => &["note", "sess-a", "a note"],
        "park" => &["park", "sess-a"],
        "unpark" => &["unpark", "sess-a"],
        "dismiss" => &["dismiss", "sess-a", "--yes"],
        "undismiss" => &["undismiss", "sess-a"],
        "done" => &["done", "sess-a"],
        "undone" => &["undone", "sess-a"],
        "progress-bind" => &["progress", "bind", "sess-a"],
        "summarize" => &["summarize", "sess-a"],
        "link" => &["link", "sess-a", "1"],
        "unlink" => &["unlink", "sess-a", "1", "--yes"],
        "export-cortexiphan" => &["export", "cortexiphan", "--include-text"],
        "links-check" => &["links", "check"],
        "migrate" => &["migrate"],
        "delete" => &["delete", "sess-a", "--yes"],
        other => panic!("{other} is a new command: give it an argv in no_egress.rs"),
    };
    Some(args.to_vec())
}

#[test]
fn every_command_runs_with_no_network_at_all() {
    let Some(unshare) = find_in_path("unshare") else {
        assert!(
            std::env::var_os("CLAUDE_MONITOR_REQUIRE_NETNS").is_none(),
            "unshare is required but not installed"
        );
        eprintln!("skipping: unshare is not installed");
        return;
    };
    if !netns_available(&unshare) {
        assert!(
            std::env::var_os("CLAUDE_MONITOR_REQUIRE_NETNS").is_none(),
            "a network namespace is required but unavailable"
        );
        eprintln!("skipping: unprivileged network namespaces are not available");
        return;
    }
    let scratch = TempDir::new().unwrap();
    let log = scratch.path().join("ran.log");
    let stubs = stub_dir(&log);
    let mut ran = 0;
    for info in Command::all() {
        let Some(args) = argv(info.name) else {
            continue;
        };
        let fixture = export_support::project20();
        let state = fixture.dir.path().to_owned();
        let home = scratch.path().join(format!("home-{}", info.name));
        fs::create_dir_all(&home).unwrap();
        let output = Process::new(&unshare)
            .args(["-rn", BIN, "--json", "--no-refresh", "--state-dir"])
            .arg(&state)
            .args(&args)
            .env_clear()
            .env("PATH", stubs.path())
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &home)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let code = output.status.code();
        assert!(
            code.is_some_and(|c| c <= 3),
            "{}: exit {code:?}, stderr: {}",
            info.name,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!log.exists(), "{} ran a network client", info.name);
        ran += 1;
    }
    assert!(ran >= 20, "only {ran} commands ran");
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn only_the_allowed_files_open_sockets() {
    const SOCKET_TOKENS: &[&str] = &[
        "TcpStream",
        "TcpListener",
        "TcpSocket",
        "UdpSocket",
        "ToSocketAddrs",
        "tokio::net",
        "reqwest",
        "ureq",
        "hyper::client",
        "isahc",
        "attohttpc",
    ];
    const TLS_CLIENT_TOKENS: &[&str] = &["TlsConnector", "ClientConnection", "ClientConfig"];
    const ALLOWED_PROGRAMS: &[&str] = &["tmux", "sysctl", "hostname"];
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    assert!(files.len() > 50);
    for file in files {
        let relative = file.strip_prefix(root()).unwrap().to_string_lossy();
        let text = fs::read_to_string(&file).unwrap();
        let may_listen_or_connect =
            relative == "src/summary.rs" || relative.starts_with("src/web/");
        if !may_listen_or_connect {
            for token in SOCKET_TOKENS {
                assert!(!text.contains(token), "{relative} uses {token}");
            }
        }
        for token in TLS_CLIENT_TOKENS {
            assert!(!text.contains(token), "{relative} uses {token}");
        }
        for line in text.lines() {
            let Some((_, rest)) = line.split_once("Command::new(\"") else {
                continue;
            };
            let program = rest.split('"').next().unwrap();
            assert!(
                ALLOWED_PROGRAMS.contains(&program),
                "{relative} runs the program {program:?}"
            );
        }
    }
    let summary = fs::read_to_string(root().join("src/summary.rs")).unwrap();
    assert!(
        summary.contains("is_loopback"),
        "summary must check loopback"
    );
}

#[test]
fn the_dependency_lock_has_no_http_client_or_tls_client_stack() {
    // rustls is allowed: the web view terminates TLS on loopback. What stays
    // banned is anything that dials out or ships a trust store for doing so.
    const BANNED: &[&str] = &[
        "reqwest",
        "ureq",
        "curl",
        "isahc",
        "surf",
        "attohttpc",
        "openssl",
        "native-tls",
        "hyper-rustls",
        "rustls-native-certs",
        "rustls-platform-verifier",
        "webpki-roots",
    ];
    let lock = fs::read_to_string(root().join("Cargo.lock")).unwrap();
    for banned in BANNED {
        assert!(
            !lock.contains(&format!("name = \"{banned}\"")),
            "{banned} is in Cargo.lock"
        );
    }
}

const MARKER: &str = "Ship the export first.";

fn files_containing(dir: &Path, needle: &str, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_containing(&path, needle, found);
        } else if fs::read(&path)
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
        {
            found.push(path);
        }
    }
}

fn is_store_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("state.db"))
}

#[test]
fn export_writes_session_text_only_to_the_out_file() {
    let fixture = export_support::project20();
    let state = fixture.dir.path().to_owned();
    let home = TempDir::new().unwrap();
    let out = home.path().join("export.jsonl");
    let status = Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(&state)
        .args(["export", "cortexiphan", "--include-text", "--out"])
        .arg(&out)
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .env("TMPDIR", home.path())
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());

    let mut found = Vec::new();
    files_containing(home.path(), MARKER, &mut found);
    files_containing(&state, MARKER, &mut found);
    found.retain(|p| !is_store_file(p));
    assert_eq!(found, vec![out]);
}

#[cfg(feature = "web")]
mod web {
    use super::*;

    use crate::web_support::{KillOnDrop, Request};
    use std::io::{BufRead, BufReader};
    use std::net::SocketAddr;

    #[test]
    fn serve_writes_session_text_to_no_file_outside_the_store() {
        let fixture = export_support::project20();
        let state = fixture.dir.path().to_owned();
        let home = TempDir::new().unwrap();
        let mut child = KillOnDrop::spawn(
            Process::new(BIN)
                .args(["--no-refresh", "--state-dir"])
                .arg(&state)
                .args(["serve", "--port", "0"])
                .env("HOME", home.path())
                .env("XDG_RUNTIME_DIR", home.path())
                .env("TMPDIR", home.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let url = line.trim().strip_prefix("http://").unwrap();
        let (authority, rest) = url.split_once('/').unwrap();
        let token = rest.strip_prefix("?token=").unwrap();
        let addr: SocketAddr = authority.parse().unwrap();
        assert!(addr.ip().is_loopback());

        let cookie = format!("cc_token={token}");
        let anonymous = Request::get("/healthz").send(addr);
        assert_eq!(anonymous.status, 401);
        let authed = Request::get("/healthz").header("Cookie", cookie).send(addr);
        assert_eq!(authed.status, 200);

        let _ = child.kill();
        let _ = child.wait();
        let mut found = Vec::new();
        files_containing(home.path(), MARKER, &mut found);
        files_containing(&state, MARKER, &mut found);
        found.retain(|p| !is_store_file(p));
        assert_eq!(found, Vec::<PathBuf>::new());
    }
}
