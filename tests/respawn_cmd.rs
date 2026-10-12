//! `execute` and the `respawn` command (T073): what starts, where, and what is
//! recorded. The agent is `sleep 30`; the tmux server is private.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command as Process;

use claude_monitor::agents::{Launch, LaunchMode};
use claude_monitor::cli::{run_in, Command};
use claude_monitor::error::Error;
use claude_monitor::model::{Author, Session, State};
use claude_monitor::respawn::{
    execute, launcher_from_env, plan_respawn, ExecuteError, Options, Plan,
};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::{FakeEnv, TmuxCall};
use common::{Fixture, T0};
use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T11:00:00Z";

fn respawn(sessions: &[&str]) -> Command {
    Command::Respawn {
        sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
        fresh: false,
        in_dir: None,
        recreate_worktree: false,
    }
}

fn json(result: Result<claude_monitor::cli::Output, Error>) -> Value {
    let output = result.unwrap_or_else(|e| panic!("respawn failed: {e}"));
    serde_json::from_str(&output.render(true)).unwrap()
}

mod execute_with_a_fake_tmux {
    use super::*;

    struct Setup {
        _dir: TempDir,
        store: Store,
        fake: FakeEnv,
    }

    fn setup() -> Setup {
        let dir = TempDir::new().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
        let mut session = Session::new("s1", "claude", "agent-s1", "/w/proj", T0);
        session.state = State::Interrupted;
        store.upsert_session(&session).unwrap();
        Setup {
            _dir: dir,
            store,
            fake: FakeEnv::new(),
        }
    }

    fn plan() -> Plan {
        Plan {
            session_id: "s1".to_owned(),
            mode: LaunchMode::Resume,
            launch: Launch {
                program: "sleep".to_owned(),
                args: vec!["30".to_owned()],
                cwd: PathBuf::from("/w/proj"),
                env: Vec::new(),
            },
            setup: Vec::new(),
            notices: Vec::new(),
            label: "proj".to_owned(),
        }
    }

    #[test]
    fn ensures_the_session_then_opens_one_window_with_the_plans_argv_cwd_and_label() {
        let s = setup();

        let started = execute(&s.store, &s.fake.env, &plan(), "agents", NOW).unwrap();

        let calls = s.fake.tmux.calls();
        assert_eq!(
            calls,
            [
                TmuxCall::EnsureSession {
                    session: "agents".to_owned(),
                    cwd: PathBuf::from("/w/proj"),
                },
                TmuxCall::SpawnWindow {
                    session: "agents".to_owned(),
                    label: "proj".to_owned(),
                    cwd: PathBuf::from("/w/proj"),
                    argv: vec!["sleep".to_owned(), "30".to_owned()],
                },
            ]
        );
        assert_eq!(started.session_id, "s1");
        assert!(started.pane_id.starts_with('%'));
    }

    #[test]
    fn records_an_open_request_for_the_new_pane() {
        let s = setup();

        let started = execute(&s.store, &s.fake.env, &plan(), "agents", NOW).unwrap();

        let request = s
            .store
            .find_open_respawn_request(&started.pane_id)
            .unwrap()
            .expect("an open request for the pane");
        assert_eq!(request.id, started.request_id);
        assert_eq!(request.session_id, "s1");
        assert_eq!(request.requested_at, NOW);
    }

    #[test]
    fn appends_a_user_authored_respawn_requested_event() {
        let s = setup();

        let started = execute(&s.store, &s.fake.env, &plan(), "agents", NOW).unwrap();

        let events = s.store.events_since(0, 100).unwrap();
        let event = events
            .iter()
            .find(|e| e.kind == "session.respawn_requested")
            .expect("a respawn_requested event");
        assert_eq!(event.entity_kind, "session");
        assert_eq!(event.entity_id, "s1");
        assert_eq!(event.at, NOW);
        assert!(
            matches!(event.author, Author::User { .. }),
            "{:?}",
            event.author
        );
        assert_eq!(event.payload["pane"], started.pane_id.as_str());
        assert_eq!(event.payload["mode"], "resume");
    }

    #[test]
    fn does_not_add_or_change_session_rows() {
        let s = setup();
        let before = s.store.list_sessions().unwrap();

        execute(&s.store, &s.fake.env, &plan(), "agents", NOW).unwrap();

        assert_eq!(s.store.list_sessions().unwrap(), before);
    }

    #[test]
    fn a_failing_tmux_records_nothing() {
        let s = setup();
        s.fake.tmux.fail_creates("no server");

        let error = execute(&s.store, &s.fake.env, &plan(), "agents", NOW).unwrap_err();

        assert!(matches!(error, ExecuteError::Tmux(_)), "{error}");
        assert!(s.store.events_since(0, 100).unwrap().is_empty());
    }

    #[test]
    fn a_failing_setup_command_stops_before_any_window_opens() {
        let s = setup();
        let mut plan = plan();
        plan.setup.push(Launch {
            program: "false".to_owned(),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        });

        let error = execute(&s.store, &s.fake.env, &plan, "agents", NOW).unwrap_err();

        assert!(matches!(error, ExecuteError::Setup { .. }), "{error}");
        assert!(
            !s.fake
                .tmux
                .calls()
                .iter()
                .any(|c| matches!(c, TmuxCall::SpawnWindow { .. })),
            "a window opened after the setup failed"
        );
        assert!(s.store.events_since(0, 100).unwrap().is_empty());
    }
}

#[test]
fn the_launcher_template_comes_from_the_agent_kinds_variable() {
    let fake = FakeEnv::new();
    fake.vars.set("CLAUDE_MONITOR_LAUNCHER_CLAUDE", "sleep 30");

    assert_eq!(
        launcher_from_env(fake.env.vars.as_ref(), "claude")
            .template
            .as_deref(),
        Some("sleep 30")
    );
    assert_eq!(
        launcher_from_env(fake.env.vars.as_ref(), "copilot").template,
        None
    );
}

#[test]
fn the_plan_labels_its_window_with_the_branch_else_the_directory() {
    let fake = FakeEnv::new();
    fake.fs.add_dir("/w/proj");
    fake.vars.set("HOME", "/home/u");
    fake.fs
        .add_file("/home/u/.claude/projects/-w-proj/agent-1.jsonl", "{}\n");
    let mut session = Session::new("s1", "claude", "agent-1", "/w/proj", T0);
    session.state = State::Interrupted;

    let plan = plan_respawn(&session, &fake.env, &Options::default()).unwrap();
    assert_eq!(plan.label, "proj");

    session.branch = Some("feature".to_owned());
    let plan = plan_respawn(&session, &fake.env, &Options::default()).unwrap();
    assert_eq!(plan.label, "feature");
}

#[test]
fn a_single_respawn_lands_in_the_recorded_directory() {
    let Some(fx) = Fixture::new() else { return };
    let session = fx.add_session("s1", State::Interrupted, "work-a");

    let out = json(run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)));

    assert_eq!(out["ok"], true);
    let started = &out["started"][0];
    assert_eq!(started["session"], "s1");
    assert_eq!(started["mode"], "resume");
    let pane = started["pane"].as_str().unwrap();
    let (cwd, command) = fx.pane_cwd_and_command(pane);
    assert_eq!(cwd, session.cwd);
    assert_eq!(command, "sleep");
    assert!(out["refused"].as_array().unwrap().is_empty());
}

#[test]
fn five_at_once_land_in_five_directories() {
    let Some(fx) = Fixture::new() else { return };
    let ids = ["s1", "s2", "s3", "s4", "s5"];
    let cwds: Vec<_> = ids
        .iter()
        .map(|id| {
            fx.add_session(id, State::Interrupted, &format!("dir-{id}"))
                .cwd
        })
        .collect();

    let out = json(run_in(respawn(&ids), &fx.env, Some(&fx.state_dir)));

    let started = out["started"].as_array().unwrap();
    assert_eq!(started.len(), 5);
    let panes: BTreeSet<_> = started
        .iter()
        .map(|s| s["pane"].as_str().unwrap())
        .collect();
    assert_eq!(panes.len(), 5);
    for (id, cwd) in ids.iter().zip(&cwds) {
        let entry = started.iter().find(|s| s["session"] == *id).unwrap();
        let (actual, _) = fx.pane_cwd_and_command(entry["pane"].as_str().unwrap());
        assert_eq!(&actual, cwd, "{id}");
    }
}

#[test]
fn a_request_row_and_an_event_are_recorded_and_no_session_row_is_added() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("s1", State::Interrupted, "work-a");
    let before = fx.store().list_sessions().unwrap();

    let out = json(run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)));

    let store = fx.store();
    let pane = out["started"][0]["pane"].as_str().unwrap();
    let request = store.find_open_respawn_request(pane).unwrap().unwrap();
    assert_eq!(request.session_id, "s1");
    assert_eq!(out["started"][0]["request"], request.id.as_str());
    assert_eq!(store.list_sessions().unwrap(), before);
    assert!(store
        .events_since(0, 100)
        .unwrap()
        .iter()
        .any(|e| e.kind == "session.respawn_requested" && e.entity_id == "s1"));
}

#[test]
fn a_refusal_starts_nothing_and_exits_2() {
    let Some(fx) = Fixture::new() else { return };
    let session = fx.add_session("s1", State::Interrupted, "work-a");
    std::fs::remove_dir(&session.cwd).unwrap();

    let error = run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(
        matches!(
            &error,
            Error::Refused {
                code: "missing_directory",
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(error.exit_code(), 2);
    assert!(error.to_string().contains(&session.cwd), "{error}");
    assert!(!fx.server_has_sessions(), "tmux was started for a refusal");
    let events = fx.respawn_events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_refusal_prints_its_alternatives() {
    let Some(fx) = Fixture::new() else { return };
    let mut session = fx.add_session("s1", State::Interrupted, "work-a");
    session.repo_root = Some(fx.root().join("repo").to_str().unwrap().to_owned());
    session.cwd = fx.root().join("gone").to_str().unwrap().to_owned();
    fx.save(&session);

    let error = run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(error.to_string().contains("--fresh --in"), "{error}");
}

#[test]
fn some_started_and_some_refused_exits_3_and_still_starts_the_good_ones() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("good", State::Interrupted, "work-a");
    let bad = fx.add_session("bad", State::Interrupted, "work-b");
    std::fs::remove_dir(&bad.cwd).unwrap();

    let error = run_in(respawn(&["good", "bad"]), &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(matches!(error, Error::Partial { .. }), "{error}");
    assert_eq!(error.exit_code(), 3);
    assert_eq!(
        fx.pane_ids().len(),
        2,
        "the shell window plus the good session"
    );
    assert!(error.to_string().contains("bad"), "{error}");
}

#[test]
fn an_unknown_session_is_refused() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("s1", State::Interrupted, "work-a");

    let error = run_in(respawn(&["nope"]), &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(
        matches!(
            &error,
            Error::Refused {
                code: "unknown_session",
                ..
            }
        ),
        "{error}"
    );
    assert!(!fx.server_has_sessions());
}

#[test]
fn in_without_fresh_is_a_usage_error() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("s1", State::Interrupted, "work-a");
    let command = Command::Respawn {
        sessions: vec!["s1".to_owned()],
        fresh: false,
        in_dir: Some(fx.root()),
        recreate_worktree: false,
    };

    let error = run_in(command, &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(matches!(error, Error::Usage(_)), "{error}");
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn an_already_running_session_is_refused() {
    let Some(fx) = Fixture::new() else { return };
    let mut session = fx.add_session("s1", State::Working, "work-a");
    session.pid = Some(4242);
    session.pid_start = Some(77);
    fx.save(&session);
    fx.processes.spawn(4242, 77);

    let error = run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)).unwrap_err();

    assert!(
        matches!(
            &error,
            Error::Refused {
                code: "already_running",
                ..
            }
        ),
        "{error}"
    );
    assert!(!fx.server_has_sessions());
}

fn git(dir: &std::path::Path, args: &[&str]) -> bool {
    Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_missing_worktree_is_recreated_only_with_the_flag() {
    let Some(fx) = Fixture::new() else { return };
    let repo = fx.root().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    if !(git(&repo, &["init", "-q"])
        && git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
        && git(&repo, &["branch", "feature"]))
    {
        eprintln!("skipping: git is not usable");
        return;
    }
    let worktree = fx.root().join("wt");
    let mut session = fx.add_session("s1", State::Interrupted, "placeholder");
    session.cwd = worktree.to_str().unwrap().to_owned();
    session.repo_root = Some(repo.to_str().unwrap().to_owned());
    session.branch = Some("feature".to_owned());
    fx.save(&session);

    let error = run_in(respawn(&["s1"]), &fx.env, Some(&fx.state_dir)).unwrap_err();
    assert!(matches!(error, Error::Refused { .. }), "{error}");
    assert!(
        !worktree.exists(),
        "the worktree was created without the flag"
    );

    let command = Command::Respawn {
        sessions: vec!["s1".to_owned()],
        fresh: false,
        in_dir: None,
        recreate_worktree: true,
    };
    let out = json(run_in(command, &fx.env, Some(&fx.state_dir)));
    assert!(worktree.join(".git").exists());
    let (cwd, _) = fx.pane_cwd_and_command(out["started"][0]["pane"].as_str().unwrap());
    assert_eq!(cwd, session.cwd);
}
