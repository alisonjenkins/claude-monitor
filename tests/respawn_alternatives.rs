//! Refusal alternatives and the flags that act on them (T064, T069): nothing
//! is created unless the user asks, and the planner itself never creates.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use claude_monitor::agents::LaunchMode;
use claude_monitor::env::Fs;
use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{plan_respawn, Options, Plan, Refusal, RefusalCode};
use claude_monitor::testing::{FakeEnv, TmuxCall};

const AT: &str = "2026-10-12T00:00:00Z";
const WORKTREE: &str = "/w/proj-wt";
const REPO: &str = "/repos/proj";
const TRANSCRIPT: &str = "/home/u/.claude/projects/-w-proj-wt/agent-1.jsonl";

fn session() -> Session {
    let mut s = Session::new("s1", "claude", "agent-1", WORKTREE, AT);
    s.state = State::Interrupted;
    s.repo_root = Some(REPO.to_owned());
    s.branch = Some("feature".to_owned());
    s
}

/// The repository exists and the worktree does not; the transcript is intact.
fn worktree_missing() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(format!("{REPO}/.git/refs/heads"));
    fake.fs.add_file(TRANSCRIPT, "{}\n");
    fake
}

fn with_branch(fake: &FakeEnv) {
    fake.fs
        .add_file(format!("{REPO}/.git/refs/heads/feature"), "abc\n");
}

fn plan(fake: &FakeEnv, options: &Options) -> Result<Plan, Refusal> {
    plan_respawn(&session(), &fake.env, options)
}

fn recreate() -> Options {
    Options {
        recreate_worktree: true,
        ..Options::default()
    }
}

#[test]
fn missing_worktree_with_its_branch_suggests_git_worktree_add() {
    let fake = worktree_missing();
    with_branch(&fake);
    let r = plan(&fake, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r.message.contains(WORKTREE), "{}", r.message);
    let suggestion = r
        .alternatives
        .iter()
        .find(|a| a.command.contains("worktree add"))
        .expect("a git worktree add suggestion");
    assert_eq!(
        suggestion.command,
        format!("git -C {REPO} worktree add {WORKTREE} feature")
    );
    assert!(r
        .alternatives
        .iter()
        .any(|a| a.command.contains("--recreate-worktree")));
}

#[test]
fn branch_packed_into_packed_refs_still_counts_as_existing() {
    let fake = worktree_missing();
    fake.fs.add_file(
        format!("{REPO}/.git/packed-refs"),
        "# pack-refs\n1234 refs/heads/main\n5678 refs/heads/feature\n",
    );
    let r = plan(&fake, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r
        .alternatives
        .iter()
        .any(|a| a.command.contains("worktree add")));
}

#[test]
fn recreate_worktree_adds_the_git_command_to_the_plan() {
    let fake = worktree_missing();
    with_branch(&fake);
    let plan = plan(&fake, &recreate()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
    assert_eq!(plan.launch.cwd, Path::new(WORKTREE));
    assert_eq!(plan.setup.len(), 1);
    let git = &plan.setup[0];
    assert_eq!(git.program, "git");
    assert_eq!(git.args, ["worktree", "add", WORKTREE, "feature"]);
    assert_eq!(git.cwd, Path::new(REPO));
}

fn in_subdirectory() -> (FakeEnv, Session) {
    let fake = worktree_missing();
    with_branch(&fake);
    let mut s = session();
    s.cwd = format!("{REPO}/.claude/worktrees/wt/src");
    (fake, s)
}

#[test]
fn recreating_a_worktree_adds_it_at_its_root_not_at_the_recorded_subdirectory() {
    let (fake, s) = in_subdirectory();
    let options = Options {
        fresh: true,
        ..recreate()
    };
    let plan = plan_respawn(&s, &fake.env, &options).unwrap();
    assert_eq!(
        plan.setup[0].args,
        [
            "worktree",
            "add",
            &format!("{REPO}/.claude/worktrees/wt"),
            "feature"
        ]
    );
}

#[test]
fn the_suggested_worktree_add_names_the_worktree_root() {
    let (fake, s) = in_subdirectory();
    let r = plan_respawn(&s, &fake.env, &Options::default()).unwrap_err();
    assert!(
        r.alternatives.iter().any(|a| a.command
            == format!("git -C {REPO} worktree add {REPO}/.claude/worktrees/wt feature")),
        "{:?}",
        r.alternatives
    );
}

#[test]
fn a_missing_subdirectory_of_an_existing_worktree_cannot_be_recreated() {
    let (fake, s) = in_subdirectory();
    fake.fs.add_dir(format!("{REPO}/.claude/worktrees/wt"));
    let r = plan_respawn(&s, &fake.env, &recreate()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
}

#[test]
fn suggested_commands_quote_paths_a_shell_would_split_or_expand() {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    let repo = "/repos/my proj$(touch x)";
    fake.fs.add_dir(format!("{repo}/.git/refs/heads"));
    fake.fs
        .add_file(format!("{repo}/.git/refs/heads/feature"), "abc\n");
    let mut s = session();
    s.repo_root = Some(repo.to_owned());
    s.cwd = "/w/my wt".to_owned();
    let r = plan_respawn(&s, &fake.env, &Options::default()).unwrap_err();
    let commands: Vec<_> = r.alternatives.iter().map(|a| a.command.as_str()).collect();
    assert!(
        commands.contains(&"git -C '/repos/my proj$(touch x)' worktree add '/w/my wt' feature"),
        "{commands:?}"
    );
    assert!(
        commands.contains(&"claude-monitor respawn s1 --fresh --in '/repos/my proj$(touch x)'"),
        "{commands:?}"
    );
}

#[test]
fn recreate_worktree_is_a_no_op_when_the_worktree_exists() {
    let fake = worktree_missing();
    with_branch(&fake);
    fake.fs.add_dir(WORKTREE);
    let plan = plan(&fake, &recreate()).unwrap();
    assert!(plan.setup.is_empty());
}

#[test]
fn without_the_flag_the_plan_never_contains_a_git_command() {
    let fake = worktree_missing();
    with_branch(&fake);
    fake.fs.add_dir(WORKTREE);
    let plan = plan(&fake, &Options::default()).unwrap();
    assert!(plan.setup.is_empty());
}

#[test]
fn recreate_worktree_cannot_help_when_the_branch_is_gone() {
    let fake = worktree_missing();
    let r = plan(&fake, &recreate()).unwrap_err();
    assert_eq!(r.code, RefusalCode::BranchGone);
}

#[test]
fn gone_branch_suggests_a_fresh_session_in_the_repository_root() {
    let fake = worktree_missing();
    let r = plan(&fake, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::BranchGone);
    let suggestion = r
        .alternatives
        .iter()
        .find(|a| a.command.contains("--fresh"))
        .expect("a fresh suggestion");
    assert_eq!(
        suggestion.command,
        format!("claude-monitor respawn s1 --fresh --in {REPO}")
    );
    assert!(
        suggestion.description.contains("not restored"),
        "{}",
        suggestion.description
    );
    assert!(r
        .alternatives
        .iter()
        .all(|a| !a.command.contains("worktree add")));
}

#[test]
fn unrecorded_branch_with_a_known_repository_still_suggests_fresh_in_it() {
    let fake = worktree_missing();
    let mut s = session();
    s.branch = None;
    let r = plan_respawn(&s, &fake.env, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r
        .alternatives
        .iter()
        .any(|a| a.command == format!("claude-monitor respawn s1 --fresh --in {REPO}")));
}

#[test]
fn nothing_to_suggest_without_a_repository() {
    let fake = worktree_missing();
    let mut s = session();
    s.branch = None;
    s.repo_root = None;
    let r = plan_respawn(&s, &fake.env, &Options::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r.alternatives.is_empty());
}

#[test]
fn fresh_in_the_repository_root_starts_there_without_history() {
    let fake = worktree_missing();
    fake.fs.add_dir(REPO);
    let options = Options {
        fresh: true,
        in_dir: Some(PathBuf::from(REPO)),
        ..Options::default()
    };
    let plan = plan(&fake, &options).unwrap();
    assert_eq!(plan.mode, LaunchMode::Fresh);
    assert_eq!(plan.launch.cwd, Path::new(REPO));
    assert!(plan.launch.args.is_empty());
    assert!(plan.setup.is_empty());
    assert!(
        plan.notices.iter().any(|n| n.contains("not restored")),
        "{:?}",
        plan.notices
    );
}

#[test]
fn fresh_in_a_missing_directory_is_refused() {
    let fake = worktree_missing();
    let options = Options {
        fresh: true,
        in_dir: Some(PathBuf::from("/nowhere")),
        ..Options::default()
    };
    let r = plan(&fake, &options).unwrap_err();
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r.message.contains("/nowhere"), "{}", r.message);
}

#[test]
fn fresh_with_recreate_worktree_recreates_then_starts_fresh() {
    let fake = worktree_missing();
    with_branch(&fake);
    let options = Options {
        fresh: true,
        recreate_worktree: true,
        ..Options::default()
    };
    let plan = plan(&fake, &options).unwrap();
    assert_eq!(plan.mode, LaunchMode::Fresh);
    assert_eq!(plan.setup.len(), 1);
    assert_eq!(plan.launch.cwd, Path::new(WORKTREE));
}

#[test]
fn planning_creates_nothing_and_starts_nothing() {
    let fake = worktree_missing();
    with_branch(&fake);
    let _ = plan(&fake, &Options::default());
    let _ = plan(&fake, &recreate());
    assert!(!fake.fs.exists(Path::new(WORKTREE)));
    assert!(
        fake.tmux
            .calls()
            .iter()
            .all(|c| !matches!(c, TmuxCall::NewSession { .. } | TmuxCall::NewWindow { .. })),
        "{:?}",
        fake.tmux.calls()
    );
}
