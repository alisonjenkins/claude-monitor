//! Restoring a session that lives in a Claude-managed worktree: Claude Code
//! resumes it only with `--worktree <name>`, started from the repository.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use claude_monitor::agents::{LaunchMode, LauncherConfig};
use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{plan_respawn, Options, Plan, Refusal, RefusalCode};
use claude_monitor::testing::FakeEnv;

const AT: &str = "2026-10-12T00:00:00Z";
const ROOT: &str = "/w/proj";
const WORKTREE: &str = "/w/proj/.claude/worktrees/deep-wishing-squirrel";

fn session(cwd: &str) -> Session {
    let mut s = Session::new("s1", "claude", "agent-1", cwd, AT);
    s.state = State::Interrupted;
    s
}

fn encoded(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// An environment where the Claude session in `cwd` can be resumed.
fn ready(cwd: &str) -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(cwd);
    fake.fs.add_file(
        format!("/home/u/.claude/projects/{}/agent-1.jsonl", encoded(cwd)),
        "{}\n",
    );
    fake
}

fn plan_for(fake: &FakeEnv, cwd: &str, options: &Options) -> Result<Plan, Refusal> {
    plan_respawn(&session(cwd), &fake.env, options)
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
fn resume_starts_in_the_repo_with_worktree_before_resume() {
    let fake = ready(WORKTREE);
    let plan = plan_for(&fake, WORKTREE, &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
    assert_eq!(plan.launch.program, "claude");
    assert_eq!(
        plan.launch.args,
        ["--worktree", "deep-wishing-squirrel", "--resume", "agent-1"]
    );
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
}

#[test]
fn a_session_in_a_subdirectory_of_the_worktree_resumes_the_worktree() {
    let cwd = format!("{WORKTREE}/scripts");
    let fake = ready(&cwd);
    let plan = plan_for(&fake, &cwd, &Options::default()).unwrap();
    assert_eq!(
        plan.launch.args,
        ["--worktree", "deep-wishing-squirrel", "--resume", "agent-1"]
    );
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
}

#[test]
fn fresh_keeps_the_worktree_and_does_not_resume() {
    let fake = ready(WORKTREE);
    let options = Options {
        fresh: true,
        ..Options::default()
    };
    let plan = plan_for(&fake, WORKTREE, &options).unwrap();
    assert_eq!(plan.mode, LaunchMode::Fresh);
    assert_eq!(plan.launch.args, ["--worktree", "deep-wishing-squirrel"]);
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
}

#[test]
fn fresh_in_another_directory_does_not_use_the_recorded_worktree() {
    let fake = ready(WORKTREE);
    fake.fs.add_dir("/w/other");
    let options = Options {
        fresh: true,
        in_dir: Some("/w/other".into()),
        ..Options::default()
    };
    let plan = plan_for(&fake, WORKTREE, &options).unwrap();
    assert!(plan.launch.args.is_empty(), "{:?}", plan.launch.args);
    assert_eq!(plan.launch.cwd, Path::new("/w/other"));
}

#[test]
fn a_name_with_a_space_and_a_quote_stays_one_argument() {
    let name = "it's odd";
    let cwd = format!("{ROOT}/.claude/worktrees/{name}");
    let fake = ready(&cwd);
    let plan = plan_for(&fake, &cwd, &Options::default()).unwrap();
    assert_eq!(
        plan.launch.args,
        ["--worktree", name, "--resume", "agent-1"]
    );
}

#[test]
fn a_session_outside_a_worktree_is_launched_as_before() {
    let fake = ready(ROOT);
    let plan = plan_for(&fake, ROOT, &Options::default()).unwrap();
    assert_eq!(plan.launch.args, ["--resume", "agent-1"]);
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
    let fresh = Options {
        fresh: true,
        ..Options::default()
    };
    let plan = plan_for(&fake, ROOT, &fresh).unwrap();
    assert!(plan.launch.args.is_empty());
}

#[test]
fn a_missing_worktree_directory_is_still_refused() {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(ROOT);
    let r = plan_for(&fake, WORKTREE, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r.message.contains(WORKTREE), "{}", r.message);
}

#[test]
fn a_template_without_the_placeholder_is_left_alone() {
    let fake = ready(WORKTREE);
    let plan = plan_for(
        &fake,
        WORKTREE,
        &with_template("wrap -- claude --resume {session_id}"),
    )
    .unwrap();
    assert_eq!(plan.launch.program, "wrap");
    assert_eq!(plan.launch.args, ["--", "claude", "--resume", "agent-1"]);
    assert_eq!(plan.launch.cwd, Path::new(WORKTREE));
}

#[test]
fn a_template_with_worktree_args_expands_them_and_starts_in_the_repo() {
    let fake = ready(WORKTREE);
    let plan = plan_for(
        &fake,
        WORKTREE,
        &with_template("wrap -- claude {worktree_args} --resume {session_id}"),
    )
    .unwrap();
    assert_eq!(
        plan.launch.args,
        [
            "--",
            "claude",
            "--worktree",
            "deep-wishing-squirrel",
            "--resume",
            "agent-1"
        ]
    );
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
}

#[test]
fn the_command_line_shows_the_directory_and_the_exact_command() {
    let fake = ready(WORKTREE);
    let plan = plan_for(&fake, WORKTREE, &Options::default()).unwrap();
    assert_eq!(
        plan.command_line(),
        "cd -- /w/proj && claude --worktree deep-wishing-squirrel --resume agent-1"
    );
}

#[test]
fn the_command_line_quotes_what_a_shell_would_split() {
    let cwd = format!("{ROOT}/.claude/worktrees/it's odd");
    let fake = ready(&cwd);
    let plan = plan_for(&fake, &cwd, &Options::default()).unwrap();
    assert_eq!(
        plan.command_line(),
        "cd -- /w/proj && claude --worktree 'it'\\''s odd' --resume agent-1"
    );
}

#[test]
fn worktree_args_vanish_for_a_session_outside_a_worktree() {
    let fake = ready(ROOT);
    let plan = plan_for(
        &fake,
        ROOT,
        &with_template("claude {worktree_args} --resume {session_id}"),
    )
    .unwrap();
    assert_eq!(plan.launch.args, ["--resume", "agent-1"]);
    assert_eq!(plan.launch.cwd, Path::new(ROOT));
}

/// An environment with the worktree directory but no transcript anywhere.
fn without_transcript(cwd: &str) -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(cwd);
    fake
}

fn transcript_in(cwd: &str) -> String {
    format!("/home/u/.claude/projects/{}/agent-1.jsonl", encoded(cwd))
}

#[test]
fn a_transcript_only_under_the_repo_root_still_resumes() {
    let fake = without_transcript(WORKTREE);
    fake.fs.add_file(transcript_in(ROOT), "{}\n");
    let plan = plan_for(&fake, WORKTREE, &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
}

#[test]
fn a_subdirectory_session_finds_the_transcript_under_the_repo_root() {
    let cwd = format!("{WORKTREE}/scripts");
    let fake = without_transcript(&cwd);
    fake.fs.add_file(transcript_in(ROOT), "{}\n");
    let plan = plan_for(&fake, &cwd, &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
}

#[test]
fn a_transcript_only_under_the_worktree_directory_resumes() {
    let fake = without_transcript(WORKTREE);
    fake.fs.add_file(transcript_in(WORKTREE), "{}\n");
    let plan = plan_for(&fake, WORKTREE, &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
}

#[test]
fn no_transcript_anywhere_is_refused_naming_every_path_checked() {
    let fake = without_transcript(WORKTREE);
    let r = plan_for(&fake, WORKTREE, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingTranscript);
    assert!(
        r.message.contains(&transcript_in(WORKTREE)),
        "{}",
        r.message
    );
    assert!(r.message.contains(&transcript_in(ROOT)), "{}", r.message);
    assert!(r.alternatives.iter().any(|a| a.command.contains("--fresh")));
}

#[test]
fn a_plain_directory_does_not_look_in_its_parent() {
    let fake = without_transcript(ROOT);
    fake.fs.add_file(transcript_in("/w"), "{}\n");
    let r = plan_for(&fake, ROOT, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingTranscript);
    assert!(!r.message.contains(&transcript_in("/w")), "{}", r.message);
}
