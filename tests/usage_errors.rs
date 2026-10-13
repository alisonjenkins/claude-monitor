#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A command-line usage error exits 1, never clap's default 2: Claude Code
//! reads a hook's exit 2 as "block", and 2 is also the "refused" code.

use std::process::{Command, Stdio};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_claude-monitor"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn a_bad_hook_flag_exits_1() {
    let out = run(&["hook", "--bogus-flag"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--bogus-flag"));
}

#[test]
fn a_bad_flag_in_json_mode_prints_the_error_object_on_stdout() {
    let out = run(&["--json", "hook", "--bogus-flag"]);
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value.get("ok"), Some(&serde_json::Value::Bool(false)));
    assert!(value
        .get("error")
        .and_then(|e| e.as_str())
        .is_some_and(|e| e.contains("--bogus-flag")));
}

#[test]
fn help_still_exits_0_on_stdout() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!out.stdout.is_empty());
}
