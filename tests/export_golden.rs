//! Golden file for the cortexiphan export (SC-009): a project with 20 steps and
//! linked sessions. Any change to `tests/fixtures/export/project20.jsonl` is a
//! reviewed schema change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod export_support;

use std::path::PathBuf;

use export_support::{options, parse, project20};
use serde_json::Value;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export/project20.jsonl")
}

#[test]
#[ignore = "implemented by T097"]
fn project20_matches_the_golden_file() {
    let actual = project20().export(&options());
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    assert_eq!(
        actual,
        golden,
        "export differs from {}; if the change is intended, regenerate the file and review it",
        golden_path().display()
    );
}

#[test]
#[ignore = "implemented by T097"]
fn two_exports_of_unchanged_data_are_byte_identical() {
    let fix = project20();
    assert_eq!(fix.export(&options()), fix.export(&options()));
}

#[test]
#[ignore = "implemented by T097"]
fn the_golden_file_has_the_expected_records() {
    let records = parse(&std::fs::read_to_string(golden_path()).unwrap());
    let count = |kind: &str| records.iter().filter(|r| r["kind"] == kind).count();
    assert_eq!(count("project"), 1);
    assert_eq!(count("task"), 20);
    assert_eq!(count("session"), 4);
    assert_eq!(count("memory"), 1);
    // 20 task part_of + 4 session part_of + 1 resumed_from.
    assert_eq!(count("edge"), 25);
}

#[test]
#[ignore = "implemented by T097"]
fn the_golden_file_is_stable_through_parse_and_reserialise() {
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    let rewritten: String = golden
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            format!("{value}\n")
        })
        .collect();
    assert_eq!(rewritten, golden);
}

#[test]
#[ignore = "implemented by T097"]
fn export_parse_reexport_is_byte_identical() {
    let out = project20().export(&options());
    let reserialised: String = parse(&out).iter().map(|v| format!("{v}\n")).collect();
    assert_eq!(reserialised, out);
}
