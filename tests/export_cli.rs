//! `claude-monitor export cortexiphan` (T098, FR-025): stdout by default,
//! `--out FILE`, `--project`, `--include-text`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod export_support;

use std::path::PathBuf;
use std::process::Command as Process;

use clap::Parser;
use claude_monitor::cli::{run_in, Cli, Command};
use claude_monitor::error::Error;
use claude_monitor::store::legacy;
use claude_monitor::testing::FakeEnv;
use export_support::{parse, project20, Fixture};

fn export(project: Option<&str>, include_text: bool, out: Option<PathBuf>) -> Command {
    Command::ExportCortexiphan {
        project: project.map(str::to_owned),
        include_text,
        out,
    }
}

/// Records the legacy-import marker against an empty directory so the viewer's
/// first-start import never reads the real runtime directory.
fn sealed(fix: Fixture) -> Fixture {
    let empty = tempfile::TempDir::new().unwrap();
    let fake = FakeEnv::new();
    legacy::migrate(&fix.store, empty.path(), fake.env.clock.as_ref()).unwrap();
    fix
}

fn fake() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOSTNAME", "test-box");
    fake
}

#[test]
fn the_subcommand_parses_into_the_command() {
    let cli = Cli::try_parse_from(["claude-monitor", "export", "cortexiphan"]).unwrap();
    assert_eq!(cli.command(), export(None, false, None));
    let cli = Cli::try_parse_from([
        "claude-monitor",
        "export",
        "cortexiphan",
        "--project",
        "p",
        "--include-text",
        "--out",
        "/tmp/x.jsonl",
    ])
    .unwrap();
    assert_eq!(
        cli.command(),
        export(Some("p"), true, Some(PathBuf::from("/tmp/x.jsonl")))
    );
    assert!(Cli::try_parse_from(["claude-monitor", "export"]).is_err());
}

#[test]
#[ignore = "implemented by T098"]
fn stdout_is_the_jsonl_export_in_both_modes() {
    let fix = sealed(project20());
    let fake = fake();
    let output = run_in(export(None, false, None), &fake.env, Some(fix.dir.path())).unwrap();
    let plain = output.render(false);
    assert_eq!(plain, output.render(true));
    let records = parse(&plain);
    assert_eq!(records.len(), 51);
    assert!(records
        .iter()
        .all(|r| r["external"]["system"] == "ai-control-centre"));
}

#[test]
#[ignore = "implemented by T098"]
fn the_clone_machine_is_this_hosts_name() {
    let fix = sealed(project20());
    let output = run_in(export(None, false, None), &fake().env, Some(fix.dir.path())).unwrap();
    let records = parse(&output.render(false));
    let project = records.iter().find(|r| r["kind"] == "project").unwrap();
    assert_eq!(
        project["node"]["repos"][0]["clones"][0]["machine"],
        "test-box"
    );
}

#[test]
#[ignore = "implemented by T098"]
fn project_and_include_text_reach_the_export() {
    let fix = project20();
    fix.session("sess-x", None, |s| {
        s.task_description = Some("TEXT-X".to_owned())
    });
    fix.session("sess-a2", Some("proj-20"), |s| {
        s.task_description = Some("TEXT-A".to_owned());
    });
    let fix = sealed(fix);
    let run = |command| {
        run_in(command, &fake().env, Some(fix.dir.path()))
            .unwrap()
            .render(false)
    };

    let plain = run(export(Some("ai-control-centre"), false, None));
    assert!(!plain.contains("TEXT-"));
    assert!(!plain.contains("sess-x"));

    let with_text = run(export(Some("ai-control-centre"), true, None));
    assert!(with_text.contains("TEXT-A"));
    assert!(!with_text.contains("TEXT-X"));
}

#[test]
#[ignore = "implemented by T098"]
fn an_unknown_project_is_refused_with_exit_2() {
    let fix = sealed(project20());
    let err = run_in(
        export(Some("nope"), false, None),
        &fake().env,
        Some(fix.dir.path()),
    )
    .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Refused {
                code: "unknown_project",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(err.exit_code(), 2);
}

#[test]
#[ignore = "implemented by T098"]
fn out_writes_the_file_and_prints_a_summary() {
    let fix = sealed(project20());
    let target = tempfile::TempDir::new().unwrap();
    let file = target.path().join("export.jsonl");
    let fake = fake();
    let stdout = run_in(
        export(None, false, Some(file.clone())),
        &fake.env,
        Some(fix.dir.path()),
    )
    .unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(parse(&written).len(), 51);
    let summary = stdout.render(false);
    assert!(summary.contains("51"), "{summary}");
    assert!(summary.contains(file.to_str().unwrap()), "{summary}");
    assert!(
        !summary.contains("ai-control-centre\""),
        "records must not go to stdout"
    );

    let again = run_in(
        export(None, false, Some(file.clone())),
        &fake.env,
        Some(fix.dir.path()),
    )
    .unwrap();
    drop(again);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), written);
}

#[test]
#[ignore = "implemented by T098"]
fn out_into_a_missing_directory_is_an_io_error_and_writes_nothing() {
    let fix = sealed(project20());
    let target = tempfile::TempDir::new().unwrap();
    let file = target.path().join("missing").join("export.jsonl");
    let err = run_in(
        export(None, false, Some(file.clone())),
        &fake().env,
        Some(fix.dir.path()),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err}");
    assert!(!file.exists());
}

#[test]
#[ignore = "implemented by T098"]
fn an_empty_store_exports_an_empty_file_and_exits_0() {
    let fix = sealed(Fixture::new());
    let output = run_in(export(None, false, None), &fake().env, Some(fix.dir.path())).unwrap();
    assert_eq!(output.render(false), "");
}

#[test]
#[ignore = "implemented by T098"]
fn the_binary_prints_jsonl_with_and_without_json_and_never_refreshes() {
    let fix = sealed(project20());
    for args in [
        &["export", "cortexiphan"][..],
        &["--json", "export", "cortexiphan"][..],
    ] {
        let out = Process::new(env!("CARGO_BIN_EXE_claude-monitor"))
            .arg("--state-dir")
            .arg(fix.dir.path())
            .args(args)
            .env("HOSTNAME", "test-box")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(parse(&text).len(), 51, "{args:?}");
        assert!(
            out.stderr.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
