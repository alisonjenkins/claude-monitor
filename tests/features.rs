// Panicking on bad test setup is the correct failure mode here.
#![allow(clippy::expect_used)]

use std::path::Path;
use std::process::Command;

#[test]
fn no_default_features_pulls_no_tui_or_async_runtime() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["tree", "--no-default-features", "-e", "normal"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree should run");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout);
    for forbidden in ["ratatui", "crossterm", "tokio"] {
        assert!(
            !tree.contains(forbidden),
            "{forbidden} is in the --no-default-features dependency tree"
        );
    }
}

#[test]
fn main_rs_stays_thin() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let source = std::fs::read_to_string(&path).expect("src/main.rs should be readable");
    let lines = source.lines().count();
    assert!(lines < 60, "src/main.rs has {lines} lines, limit is 59");
}
