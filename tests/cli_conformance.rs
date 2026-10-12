//! CLI conformance over the library's `Command` enum (`contracts/cli.md`,
//! quickstart S0, FR-035).
//!
//! Every command in `Command::all()` gets two generated tests:
//!
//! - `<command>::error_contract` passes today: whatever the command returns,
//!   `--json` output parses, errors have the `{"ok":false,"error":...}`
//!   shape, and the exit code is 0/1/2/3 and matches the `Error` variant.
//! - `<command>::implemented` is `#[ignore]`d until the command exists: it
//!   additionally requires a real result instead of `NotImplemented`.
//!
//! Rule: the task that implements a command removes that command's
//! `#[ignore]` (its reason names the task) and makes `implemented` pass.
//! `hook`, `dashboard` and `migrate` already exist and are not ignored;
//! `migrate` has its own test because it writes a store.

// Panicking with a message on a failed check is the failure mode of a test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command as Process, Stdio};

use claude_monitor::cli::{run, Command, Output};
use claude_monitor::error::Error;
use claude_monitor::testing::FakeEnv;
use serde_json::Value;

/// Exit code per `contracts/cli.md`, written out independently of
/// `Error::exit_code`.
fn expected_exit(error: &Error) -> u8 {
    match error {
        Error::Usage(_) | Error::Config(_) | Error::NotImplemented(_) => 1,
        Error::Io { .. } | Error::Store { .. } => 1,
        Error::Refused { .. } => 2,
        Error::Partial { .. } => 3,
    }
}

fn run_example(name: &str) -> Result<Output, Error> {
    let info = Command::all()
        .iter()
        .find(|info| info.name == name)
        .unwrap_or_else(|| panic!("{name} is not in Command::all()"));
    let fake = FakeEnv::new();
    run((info.example)(), &fake.env)
}

fn assert_contract(name: &str, result: &Result<Output, Error>) {
    match result {
        Ok(output) => {
            let rendered = output.render(true);
            if !rendered.is_empty() {
                serde_json::from_str::<Value>(&rendered)
                    .unwrap_or_else(|e| panic!("{name}: --json success is not JSON: {e}"));
            }
        }
        Err(error) => {
            let rendered = error.render(true);
            let value: Value = serde_json::from_str(&rendered)
                .unwrap_or_else(|e| panic!("{name}: --json error is not JSON: {e}"));
            assert_eq!(value.get("ok"), Some(&Value::Bool(false)), "{name}");
            assert_eq!(
                value.get("error").and_then(Value::as_str),
                Some(error.to_string().as_str()),
                "{name}"
            );
            assert_eq!(value.as_object().unwrap().len(), 2, "{name}");
            let code = error.exit_code();
            assert!(code <= 3, "{name}: exit code {code} is outside 0..=3");
            assert_eq!(code, expected_exit(error), "{name}: {error}");
        }
    }
}

macro_rules! command_tests {
    ($( $module:ident => $name:literal, $reason:literal; )*) => {
        /// `(command, ignore reason)` for every generated test module.
        const GENERATED: &[(&str, &str)] = &[ $( ($name, $reason) ),* ];

        $(
            mod $module {
                use super::*;

                #[test]
                fn error_contract() {
                    assert_contract($name, &run_example($name));
                }

                #[test]
                #[ignore = $reason]
                fn implemented() {
                    let result = run_example($name);
                    assert!(
                        !matches!(result, Err(Error::NotImplemented(_))),
                        "{} is still NotImplemented",
                        $name
                    );
                    assert_contract($name, &result);
                }
            }
        )*
    };
}

command_tests! {
    list => "list", "implemented by T032";
    show => "show", "implemented by T032";
    reconcile => "reconcile", "implemented by T052";
    watch => "watch", "implemented by T060";
    restore => "restore", "implemented by T073";
    respawn => "respawn", "implemented by T073";
    boot_check => "boot-check", "implemented by T075";
    project => "project", "implemented by T108";
    group => "group", "implemented by T108";
    note => "note", "implemented by T109";
    park => "park", "implemented by T109";
    unpark => "unpark", "implemented by T109";
    dismiss => "dismiss", "implemented by T109";
    undismiss => "undismiss", "implemented by T109";
    done => "done", "implemented by T110";
    undone => "undone", "implemented by T110";
    progress_bind => "progress-bind", "implemented by T088";
    summarize => "summarize", "implemented by T092";
    serve => "serve", "implemented by T117";
    link => "link", "implemented by T100";
    export_cortexiphan => "export-cortexiphan", "implemented by T098";
    links_check => "links-check", "implemented by T101";
    delete => "delete", "implemented by unscheduled (no task in tasks.md)";
}

mod hook {
    use super::*;

    /// `hook` always exits 0 and never writes stdout, with or without
    /// `--json`. Run as a process because it reads stdin to EOF.
    #[test]
    fn exits_0_and_prints_nothing() {
        for args in [&["hook"][..], &["--json", "hook"][..]] {
            let out = Process::new(env!("CARGO_BIN_EXE_claude-monitor"))
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(0), "{args:?}");
            assert!(out.stdout.is_empty(), "{args:?}");
        }
    }

    #[test]
    fn is_not_a_tui_action() {
        let info = Command::Hook { agent: None }.info().unwrap();
        assert!(!info.is_interactive());
    }
}

mod migrate {
    use super::*;
    use claude_monitor::cli::run_in;

    /// Runs against a temporary state directory so the test never touches
    /// the real store; the legacy files are only read.
    #[test]
    fn implemented() {
        let state = tempfile::tempdir().unwrap();
        let info = Command::Migrate.info().unwrap();
        let fake = FakeEnv::new();
        let result = run_in((info.example)(), &fake.env, Some(state.path()));
        assert!(
            !matches!(result, Err(Error::NotImplemented(_))),
            "migrate is still NotImplemented"
        );
        assert_contract("migrate", &result);
    }
}

mod dashboard {
    use super::*;

    /// Without the `tui` feature the dashboard is a usage error (exit 1,
    /// JSON shape). With it the dashboard takes over the terminal, so it is
    /// not run here.
    #[cfg(not(feature = "tui"))]
    #[test]
    fn error_contract() {
        let result = run_example("dashboard");
        assert!(matches!(result, Err(Error::Usage(_))));
        assert_contract("dashboard", &result);
    }

    #[test]
    fn is_the_interactive_entry_point() {
        let info = Command::Dashboard.info().unwrap();
        assert!(info.is_interactive());
        assert_eq!(info.tui_action, Some("open dashboard"));
    }
}

#[test]
fn every_command_has_generated_tests_with_the_right_task() {
    for info in Command::all() {
        match info.name {
            "hook" | "dashboard" | "migrate" => {}
            name => {
                let reason = GENERATED
                    .iter()
                    .find(|(n, _)| *n == name)
                    .unwrap_or_else(|| panic!("{name} has no conformance tests"))
                    .1;
                assert_eq!(
                    reason,
                    format!("implemented by {}", info.implemented_by),
                    "{name}: ignore reason and CommandInfo::implemented_by disagree"
                );
            }
        }
    }
    assert_eq!(GENERATED.len() + 3, Command::all().len());
}

#[test]
fn every_tui_action_maps_to_a_command() {
    // The actions the TUI offers, from contracts/cli.md.
    let actions = [
        "respawn",
        "restore selection",
        "done",
        "dismiss",
        "park",
        "note",
        "group",
        "choose progress source",
        "summarize",
        "link",
    ];
    for action in actions {
        let name = Command::for_tui_action(action)
            .unwrap_or_else(|| panic!("TUI action {action:?} has no Command"));
        assert!(
            Command::all().iter().any(|info| info.name == name),
            "{action:?} maps to unknown command {name}"
        );
    }
    for info in Command::all() {
        if let Some(action) = info.tui_action {
            assert_eq!(Command::for_tui_action(action), Some(info.name));
        }
    }
}

#[test]
fn exit_codes_are_only_0_to_3() {
    let errors = [
        Error::Usage(String::new()),
        Error::Config(String::new()),
        Error::Refused {
            code: "x",
            message: String::new(),
        },
        Error::Partial {
            operation: String::new(),
            detail: String::new(),
        },
        Error::Io {
            operation: String::new(),
            path: None,
            source: std::io::Error::other("x"),
        },
        Error::Store {
            operation: String::new(),
            path: None,
            detail: String::new(),
        },
        Error::NotImplemented(String::new()),
    ];
    for error in &errors {
        assert!(error.exit_code() <= 3);
        assert_eq!(error.exit_code(), expected_exit(error), "{error}");
    }
}
