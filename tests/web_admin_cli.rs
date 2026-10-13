//! `claude-monitor web ...`: the CA, login and client-certificate helpers, run
//! as the real binary against a temporary state directory.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command as Process, Output, Stdio};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");

fn web(state: &Path, args: &[&str]) -> Output {
    Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(state)
        .arg("web")
        .args(args)
        .env("HOME", state)
        .env("XDG_RUNTIME_DIR", state)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn web_ca_prints_the_certificate_path_and_how_to_trust_it() {
    let state = TempDir::new().unwrap();

    let text = stdout(&web(state.path(), &["ca"]));

    let ca = state.path().join("tls").join("ca.pem");
    assert!(text.contains(&ca.display().to_string()), "{text}");
    assert!(ca.is_file());
    assert!(text.contains("Firefox"), "{text}");
    assert!(
        text.contains("Chromium") || text.contains("Chrome"),
        "{text}"
    );
    assert!(!text.contains("PRIVATE KEY"));
    assert_eq!(
        std::fs::metadata(&ca).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn web_ca_print_emits_only_the_certificate_pem() {
    let state = TempDir::new().unwrap();

    let first = stdout(&web(state.path(), &["ca", "--print"]));
    let second = stdout(&web(state.path(), &["ca", "--print"]));

    assert!(
        first.starts_with("-----BEGIN CERTIFICATE-----\n"),
        "{first}"
    );
    assert!(first.trim_end().ends_with("-----END CERTIFICATE-----"));
    assert!(!first.contains("PRIVATE KEY"));
    assert_eq!(first, second, "the CA is created once and reused");
    assert_eq!(
        first,
        std::fs::read_to_string(state.path().join("tls").join("ca.pem")).unwrap()
    );
}

#[test]
fn web_ca_in_json_mode_names_the_file() {
    let state = TempDir::new().unwrap();

    let output = Process::new(BIN)
        .args(["--json", "--no-refresh", "--state-dir"])
        .arg(state.path())
        .args(["web", "ca"])
        .env("HOME", state.path())
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        value["path"],
        state
            .path()
            .join("tls")
            .join("ca.pem")
            .display()
            .to_string()
    );
    assert!(value["hint"].as_str().unwrap().contains("Firefox"));
}
