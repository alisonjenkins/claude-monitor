#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `dismiss` and `unlink` ask before they change anything: a prompt on a
//! terminal, `--yes` to skip it, and a clear refusal when nobody can answer.

use std::io::Cursor;
use std::process::{Command as Process, Stdio};

use claude_monitor::cli::confirm::{confirm_command, Terminal};
use claude_monitor::cli::{run_in, Command, ProjectAction};
use claude_monitor::error::Error;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::Value;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");

struct Fixture {
    state: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let state = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        let mut session = Session::new(
            "s1",
            "claude",
            "agent-s1",
            "/work/s1",
            "2026-07-01T00:00:00Z",
        );
        session.state = State::Idle;
        session.last_activity_at = "2026-10-12T11:00:00Z".to_owned();
        store.upsert_session(&session).unwrap();
        Self { state, fake, store }
    }

    fn run(&self, command: Command) -> Result<claude_monitor::cli::Output, Error> {
        run_in(command, &self.fake.env, Some(self.state.path()))
    }

    fn dismissed(&self, id: &str) -> bool {
        self.store.get_session(id).unwrap().unwrap().dismissed
    }

    fn ask(&self, command: Command, answer: &str) -> (Result<Command, Error>, String) {
        let mut input = Cursor::new(answer.as_bytes().to_vec());
        let mut output = Vec::new();
        let result = confirm_command(
            command,
            &self.fake.env,
            Some(self.state.path()),
            Some(Terminal {
                input: &mut input,
                output: &mut output,
            }),
        );
        (result, String::from_utf8(output).unwrap())
    }

    fn ask_without_terminal(&self, command: Command) -> Result<Command, Error> {
        confirm_command(command, &self.fake.env, Some(self.state.path()), None)
    }
}

fn dismiss(target: &str, yes: bool) -> Command {
    Command::Dismiss {
        target: target.to_owned(),
        yes,
    }
}

fn unlink(owner: &str, yes: bool) -> Command {
    Command::Unlink {
        owner: owner.to_owned(),
        cortex: "42".to_owned(),
        yes,
    }
}

fn link(owner: &str) -> Command {
    Command::Link {
        owner: owner.to_owned(),
        cortex: "42".to_owned(),
        kind: "task".to_owned(),
        doc: None,
    }
}

fn usage_message(result: Result<Command, Error>) -> String {
    match result {
        Err(error @ Error::Usage(_)) => {
            assert_eq!(error.exit_code(), 1);
            error.to_string()
        }
        other => panic!("expected a usage error, got {other:?}"),
    }
}

mod the_question {
    use super::*;

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn names_a_session_and_its_consequence_and_ends_in_the_proceed_prompt() {
        let fx = Fixture::new();

        let (result, shown) = fx.ask(dismiss("s1", false), "y\n");

        assert_eq!(result.unwrap(), dismiss("s1", true));
        assert!(shown.contains("session s1"), "{shown}");
        assert!(shown.contains("nothing is deleted"), "{shown}");
        assert!(shown.trim_end().ends_with("Proceed? [y/N]"), "{shown}");
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn names_a_project_and_says_it_is_archived() {
        let fx = Fixture::new();
        fx.run(Command::Project {
            action: ProjectAction::Add,
            name: "side".to_owned(),
            value: None,
        })
        .unwrap();

        let (result, shown) = fx.ask(dismiss("side", false), "y\n");

        assert_eq!(result.unwrap(), dismiss("side", true));
        assert!(shown.contains("project side"), "{shown}");
        assert!(shown.contains("archive"), "{shown}");
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn names_the_link_that_unlink_removes() {
        let fx = Fixture::new();
        fx.run(link("s1")).unwrap();

        let (result, shown) = fx.ask(unlink("s1", false), "y\n");

        assert_eq!(result.unwrap(), unlink("s1", true));
        assert!(shown.contains("session s1"), "{shown}");
        assert!(shown.contains("node 42"), "{shown}");
        assert!(shown.trim_end().ends_with("Proceed? [y/N]"), "{shown}");
    }
}

mod the_answer {
    use super::*;

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn y_and_yes_agree_in_any_case() {
        let fx = Fixture::new();
        for answer in ["y\n", "Y\n", "yes\n", " YES \n"] {
            let (result, _) = fx.ask(dismiss("s1", false), answer);
            assert_eq!(result.unwrap(), dismiss("s1", true), "{answer:?}");
        }
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn anything_else_declines_and_changes_nothing() {
        let fx = Fixture::new();
        for answer in ["n\n", "\n", "", "yep\n", "no\n", "maybe\n"] {
            let (result, _) = fx.ask(dismiss("s1", false), answer);
            let message = usage_message(result);
            assert!(message.contains("nothing was changed"), "{message}");
            assert!(!fx.dismissed("s1"), "{answer:?}");
        }
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn declining_unlink_keeps_the_link() {
        let fx = Fixture::new();
        fx.run(link("s1")).unwrap();

        let (result, _) = fx.ask(unlink("s1", false), "n\n");

        usage_message(result);
        let removed = fx.run(unlink("s1", true)).unwrap().render(true);
        let removed: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(removed["changed"], true);
    }
}

mod skipping_the_prompt {
    use super::*;

    #[test]
    fn yes_passes_through_without_reading_an_answer() {
        let fx = Fixture::new();

        let (result, shown) = fx.ask(dismiss("s1", true), "");

        assert_eq!(result.unwrap(), dismiss("s1", true));
        assert_eq!(shown, "");
    }

    #[test]
    fn commands_that_need_no_confirmation_are_untouched() {
        let fx = Fixture::new();
        let undismiss = Command::Undismiss {
            target: "s1".to_owned(),
        };

        let (result, shown) = fx.ask(undismiss.clone(), "");

        assert_eq!(result.unwrap(), undismiss);
        assert_eq!(shown, "");
    }
}

mod with_nobody_to_ask {
    use super::*;

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn no_terminal_and_no_yes_is_refused_naming_the_flag() {
        let fx = Fixture::new();

        let message = usage_message(fx.ask_without_terminal(dismiss("s1", false)));

        assert!(message.contains("--yes"), "{message}");
        assert!(!fx.dismissed("s1"));
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn running_a_command_directly_without_yes_changes_nothing() {
        let fx = Fixture::new();
        fx.run(link("s1")).unwrap();

        let dismissed = fx.run(dismiss("s1", false));
        let unlinked = fx.run(unlink("s1", false));

        for result in [dismissed, unlinked] {
            match result {
                Err(Error::Usage(message)) => assert!(message.contains("--yes"), "{message}"),
                other => panic!("expected a usage error, got {other:?}"),
            }
        }
        assert!(!fx.dismissed("s1"));
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn an_unknown_target_is_reported_before_any_question() {
        let fx = Fixture::new();

        let (result, shown) = fx.ask(dismiss("nothing", false), "y\n");

        assert!(
            matches!(
                result,
                Err(Error::Refused {
                    code: "unknown_target",
                    ..
                })
            ),
            "{result:?}"
        );
        assert_eq!(shown, "");
    }
}

mod the_binary {
    use super::*;

    fn run(fx: &Fixture, args: &[&str]) -> std::process::Output {
        let home = TempDir::new().unwrap();
        Process::new(BIN)
            .args(["--no-refresh", "--state-dir"])
            .arg(fx.state.path())
            .args(args)
            .env_clear()
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn dismiss_without_yes_and_without_a_terminal_exits_1_and_changes_nothing() {
        let fx = Fixture::new();

        let out = run(&fx, &["dismiss", "s1"]);

        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("--yes"), "{stderr}");
        assert!(!fx.dismissed("s1"));
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn json_mode_is_refused_the_same_way_with_the_error_on_stdout() {
        let fx = Fixture::new();

        let out = run(&fx, &["--json", "dismiss", "s1"]);

        assert_eq!(out.status.code(), Some(1));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("--yes"), "{body}");
        assert!(!fx.dismissed("s1"));
    }

    #[test]
    fn yes_and_short_y_proceed() {
        let fx = Fixture::new();
        let out = run(&fx, &["dismiss", "s1", "--yes"]);
        assert_eq!(out.status.code(), Some(0), "{:?}", out);
        assert!(fx.dismissed("s1"));

        let fx = Fixture::new();
        fx.run(link("s1")).unwrap();
        let out = run(&fx, &["unlink", "s1", "42", "-y"]);
        assert_eq!(out.status.code(), Some(0), "{:?}", out);
    }

    #[test]
    #[ignore = "confirmation is not implemented yet"]
    fn unlink_without_yes_keeps_the_link() {
        let fx = Fixture::new();
        fx.run(link("s1")).unwrap();

        let out = run(&fx, &["unlink", "s1", "42"]);

        assert_eq!(out.status.code(), Some(1));
        let removed = fx.run(unlink("s1", true)).unwrap().render(true);
        let removed: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(removed["changed"], true);
    }
}
