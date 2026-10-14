//! Restoring a named Claude session passes the name back with `--name`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use claude_monitor::agents::{kinds, LaunchMode, LauncherConfig};
use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{plan_respawn, Options, Plan};
use claude_monitor::testing::FakeEnv;

const AT: &str = "2026-10-12T00:00:00Z";
const ROOT: &str = "/w/proj";
const WORKTREE: &str = "/w/proj/.claude/worktrees/deep-wishing-squirrel";

fn named(cwd: &str, agent: &str, name: Option<&str>) -> Session {
    let mut s = Session::new("s1", agent, "agent-1", cwd, AT);
    s.state = State::Interrupted;
    s.session_name = name.map(str::to_owned);
    s
}

fn ready(cwd: &str) -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(cwd);
    let encoded: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    fake.fs.add_file(
        format!("/home/u/.claude/projects/{encoded}/agent-1.jsonl"),
        "{}\n",
    );
    fake
}

fn plan(session: &Session, options: &Options) -> Plan {
    plan_respawn(session, &ready(&session.cwd).env, options).unwrap()
}

fn fresh() -> Options {
    Options {
        fresh: true,
        ..Options::default()
    }
}

fn with_template(template: &str) -> Options {
    Options {
        launcher: LauncherConfig {
            template: Some(template.to_owned()),
        },
        ..Options::default()
    }
}

#[test]
fn resume_passes_the_name_before_resume() {
    let plan = plan(&named(ROOT, "claude", Some("aquero")), &Options::default());
    assert_eq!(
        plan.launch.args,
        ["--name", "aquero", "--resume", "agent-1"]
    );
}

#[test]
fn the_name_follows_worktree_and_precedes_resume() {
    let plan = plan(
        &named(WORKTREE, "claude", Some("aquero")),
        &Options::default(),
    );
    assert_eq!(
        plan.launch.args,
        [
            "--worktree",
            "deep-wishing-squirrel",
            "--name",
            "aquero",
            "--resume",
            "agent-1"
        ]
    );
}

#[test]
fn a_fresh_launch_passes_the_name_too() {
    let plain = plan(&named(ROOT, "claude", Some("aquero")), &fresh());
    assert_eq!(plain.launch.args, ["--name", "aquero"]);

    let in_worktree = plan(&named(WORKTREE, "claude", Some("aquero")), &fresh());
    assert_eq!(
        in_worktree.launch.args,
        ["--worktree", "deep-wishing-squirrel", "--name", "aquero"]
    );
}

#[test]
fn a_name_with_a_space_and_a_quote_is_one_argument_and_one_quoted_word() {
    let plan = plan(
        &named(ROOT, "claude", Some("it's a name")),
        &Options::default(),
    );
    assert_eq!(
        plan.launch.args,
        ["--name", "it's a name", "--resume", "agent-1"]
    );
    assert_eq!(
        plan.command_line(),
        "cd -- /w/proj && claude --name 'it'\\''s a name' --resume agent-1"
    );
}

#[test]
fn an_unnamed_session_launches_exactly_as_before() {
    let resume = plan(&named(ROOT, "claude", None), &Options::default());
    assert_eq!(resume.launch.args, ["--resume", "agent-1"]);
    assert_eq!(
        resume.command_line(),
        "cd -- /w/proj && claude --resume agent-1"
    );
    let fresh = plan(&named(ROOT, "claude", None), &fresh());
    assert!(fresh.launch.args.is_empty());
}

#[test]
fn a_template_expands_name_args_and_a_template_without_it_runs_as_written() {
    let session = named(ROOT, "claude", Some("my name"));

    let with = plan(
        &session,
        &with_template("wrap -- claude {name_args} --resume {session_id}"),
    );
    assert_eq!(
        with.launch.args,
        ["--", "claude", "--name", "my name", "--resume", "agent-1"]
    );

    let without = plan(
        &session,
        &with_template("wrap -- claude --resume {session_id}"),
    );
    assert_eq!(without.launch.args, ["--", "claude", "--resume", "agent-1"]);
}

#[test]
fn name_args_vanish_for_an_unnamed_session() {
    let plan = plan(
        &named(ROOT, "claude", None),
        &with_template("claude {name_args} --resume {session_id}"),
    );
    assert_eq!(plan.launch.args, ["--resume", "agent-1"]);
}

#[test]
fn copilot_never_gets_the_name() {
    let session = named(ROOT, "copilot", Some("aquero"));
    let copilot = kinds().into_iter().find(|k| k.id() == "copilot").unwrap();

    for mode in [LaunchMode::Resume, LaunchMode::Fresh] {
        let launch = copilot
            .launch(&session, mode, &LauncherConfig::default())
            .unwrap();
        assert!(!launch.args.iter().any(|a| a == "--name"), "{launch:?}");
    }
}
