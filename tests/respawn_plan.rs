//! The respawn planner: checks, refusal codes, launcher templates (T063, T068).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use claude_monitor::agents::{
    AgentKind, Launch, LaunchError, LaunchMode, LauncherConfig, ParseError, ParsedHook,
};
use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{
    plan_respawn, plan_respawn_with, Options, PanePlacement, Plan, Refusal, RefusalCode,
};
use claude_monitor::testing::FakeEnv;

const AT: &str = "2026-10-12T00:00:00Z";
const CWD: &str = "/w/proj";
const TRANSCRIPT: &str = "/home/u/.claude/projects/-w-proj/agent-1.jsonl";

fn session(state: State) -> Session {
    let mut s = Session::new("s1", "claude", "agent-1", CWD, AT);
    s.state = state;
    s.repo_root = Some("/repos/proj".to_owned());
    s.branch = Some("feature".to_owned());
    s
}

/// An environment where a Claude session in `CWD` can be resumed.
fn ready() -> FakeEnv {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(CWD);
    fake.fs.add_file(TRANSCRIPT, "{}\n");
    fake
}

fn plan(fake: &FakeEnv, session: &Session, options: &Options) -> Result<Plan, Refusal> {
    plan_respawn(session, &fake.env, options)
}

fn refusal(result: Result<Plan, Refusal>) -> Refusal {
    result.expect_err("expected a refusal")
}

#[test]
fn interrupted_claude_session_resumes_in_its_recorded_directory() {
    let fake = ready();
    let plan = plan(&fake, &session(State::Interrupted), &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Resume);
    assert_eq!(plan.launch.program, "claude");
    assert_eq!(plan.launch.args, ["--resume", "agent-1"]);
    assert_eq!(plan.launch.cwd, Path::new(CWD));
    assert!(plan.setup.is_empty());
    assert!(plan.notices.is_empty(), "{:?}", plan.notices);
}

#[test]
fn copilot_session_resumes_with_its_session_id() {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(CWD);
    fake.fs.add_dir("/home/u/.copilot/session-state/agent-1");
    let mut s = session(State::Interrupted);
    s.agent_kind = "copilot".to_owned();
    let plan = plan(&fake, &s, &Options::default()).unwrap();
    assert_eq!(plan.launch.program, "copilot");
    assert_eq!(plan.launch.args, ["--resume", "agent-1"]);
    assert_eq!(plan.launch.cwd, Path::new(CWD));
}

#[test]
fn copilot_session_without_its_state_directory_is_refused() {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(CWD);
    let mut s = session(State::Interrupted);
    s.agent_kind = "copilot".to_owned();
    let r = refusal(plan(&fake, &s, &Options::default()));
    assert_eq!(r.code, RefusalCode::MissingTranscript);
}

#[test]
fn missing_directory_is_refused_and_names_the_path() {
    let fake = ready();
    let mut s = session(State::Interrupted);
    s.cwd = "/w/gone".to_owned();
    s.repo_root = None;
    s.branch = None;
    let r = refusal(plan(&fake, &s, &Options::default()));
    assert_eq!(r.code, RefusalCode::MissingDirectory);
    assert!(r.message.contains("/w/gone"), "{}", r.message);
}

#[test]
fn missing_transcript_is_refused_and_offers_a_fresh_start() {
    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.fs.add_dir(CWD);
    let r = refusal(plan(
        &fake,
        &session(State::Interrupted),
        &Options::default(),
    ));
    assert_eq!(r.code, RefusalCode::MissingTranscript);
    assert!(r.message.contains(TRANSCRIPT), "{}", r.message);
    assert!(r.alternatives.iter().any(|a| a.command.contains("--fresh")));
}

#[test]
fn gone_branch_with_a_missing_worktree_is_refused_as_branch_gone() {
    let fake = ready();
    let mut s = session(State::Interrupted);
    s.cwd = "/w/gone".to_owned();
    let r = refusal(plan(&fake, &s, &Options::default()));
    assert_eq!(r.code, RefusalCode::BranchGone);
    assert!(r.message.contains("feature"), "{}", r.message);
}

#[test]
fn live_process_means_already_running() {
    let fake = ready();
    fake.processes.spawn(4242, 99);
    let mut s = session(State::Working);
    s.pid = Some(4242);
    s.pid_start = Some(99);
    s.tmux_pane = Some("%7".to_owned());
    let r = refusal(plan(&fake, &s, &Options::default()));
    assert_eq!(r.code, RefusalCode::AlreadyRunning);
    assert!(r.message.contains("%7"), "{}", r.message);
}

#[test]
fn recycled_pid_is_not_running() {
    let fake = ready();
    fake.processes.spawn(4242, 100);
    let mut s = session(State::Interrupted);
    s.pid = Some(4242);
    s.pid_start = Some(99);
    assert!(plan(&fake, &s, &Options::default()).is_ok());
}

#[test]
fn unknown_agent_kind_has_no_launcher() {
    let fake = ready();
    let mut s = session(State::Interrupted);
    s.agent_kind = "nonesuch".to_owned();
    let r = refusal(plan(&fake, &s, &Options::default()));
    assert_eq!(r.code, RefusalCode::NoLauncher);
}

#[test]
fn unusable_launcher_template_has_no_launcher() {
    let fake = ready();
    let options = Options {
        launcher: LauncherConfig {
            template: Some("claude --resume \"{session_id}".to_owned()),
        },
        ..Options::default()
    };
    let r = refusal(plan(&fake, &session(State::Interrupted), &options));
    assert_eq!(r.code, RefusalCode::NoLauncher);
}

#[test]
fn missing_tmux_is_refused() {
    let fake = ready();
    fake.tmux.set_available(false);
    let r = refusal(plan(
        &fake,
        &session(State::Interrupted),
        &Options::default(),
    ));
    assert_eq!(r.code, RefusalCode::NoTmux);
}

#[test]
fn launcher_template_fills_session_id_cwd_and_name() {
    let fake = ready();
    let options = Options {
        launcher: LauncherConfig {
            template: Some(
                "wrap --dir {cwd} --name '{name}' -- claude --resume {session_id}".to_owned(),
            ),
        },
        ..Options::default()
    };
    let plan = plan(&fake, &session(State::Interrupted), &options).unwrap();
    assert_eq!(plan.launch.program, "wrap");
    assert_eq!(
        plan.launch.args,
        ["--dir", CWD, "--name", "feature", "--", "claude", "--resume", "agent-1"]
    );
}

#[test]
fn explicit_fresh_starts_without_resume_and_says_so() {
    let fake = FakeEnv::new();
    fake.fs.add_dir(CWD);
    let options = Options {
        fresh: true,
        ..Options::default()
    };
    let plan = plan(&fake, &session(State::Interrupted), &options).unwrap();
    assert_eq!(plan.mode, LaunchMode::Fresh);
    assert_eq!(plan.launch.program, "claude");
    assert!(plan.launch.args.is_empty());
    assert_eq!(plan.launch.cwd, Path::new(CWD));
    assert!(
        plan.notices.iter().any(|n| n.contains("not restored")),
        "{:?}",
        plan.notices
    );
}

/// A kind that can start sessions but has no way to continue one.
struct NoResume;

impl AgentKind for NoResume {
    fn id(&self) -> &'static str {
        "noresume"
    }

    fn parse_hook(&self, _raw: &serde_json::Value) -> Result<Vec<ParsedHook>, ParseError> {
        Ok(Vec::new())
    }

    fn launch(
        &self,
        session: &Session,
        mode: LaunchMode,
        _cfg: &LauncherConfig,
    ) -> Result<Launch, LaunchError> {
        match mode {
            LaunchMode::Resume => Err(LaunchError::ResumeUnsupported { kind: "noresume" }),
            LaunchMode::Fresh => Ok(Launch {
                program: "noresume".to_owned(),
                args: Vec::new(),
                cwd: PathBuf::from(&session.cwd),
                env: Vec::new(),
            }),
        }
    }
}

#[test]
fn agent_without_resume_starts_fresh_with_a_notice() {
    let fake = FakeEnv::new();
    fake.fs.add_dir(CWD);
    let mut s = session(State::Interrupted);
    s.agent_kind = "noresume".to_owned();
    let plan = plan_respawn_with(&NoResume, &s, &fake.env, &Options::default()).unwrap();
    assert_eq!(plan.mode, LaunchMode::Fresh);
    assert_eq!(plan.launch.program, "noresume");
    assert_eq!(plan.launch.cwd, Path::new(CWD));
    assert!(
        plan.notices.iter().any(|n| n.contains("not restored")),
        "{:?}",
        plan.notices
    );
}

#[test]
fn finished_session_plans_exactly_like_an_interrupted_one() {
    let fake = ready();
    let interrupted = plan(&fake, &session(State::Interrupted), &Options::default());
    let mut done = session(State::Finished);
    done.end_reason = Some("prompt_input_exit".to_owned());
    done.done_at = Some(AT.to_owned());
    let finished = plan(&fake, &done, &Options::default());
    assert!(interrupted.is_ok());
    assert_eq!(finished, interrupted);
}

#[test]
fn in_dir_without_fresh_is_a_usage_error() {
    let fake = ready();
    let options = Options {
        in_dir: Some(PathBuf::from("/repos/proj")),
        ..Options::default()
    };
    let r = refusal(plan(&fake, &session(State::Interrupted), &options));
    assert_eq!(r.code, RefusalCode::InvalidOptions);
    assert_eq!(r.code.exit_code(), 1);
}

#[test]
fn refusal_codes_map_to_contract_strings_and_exit_code_2() {
    for (code, text) in [
        (RefusalCode::AlreadyRunning, "already_running"),
        (RefusalCode::MissingDirectory, "missing_directory"),
        (RefusalCode::MissingTranscript, "missing_transcript"),
        (RefusalCode::BranchGone, "branch_gone"),
        (RefusalCode::NoLauncher, "no_launcher"),
        (RefusalCode::NoTmux, "no_tmux"),
    ] {
        assert_eq!(code.as_str(), text);
        assert_eq!(code.exit_code(), 2);
    }
}

fn placed(tmux_session: &str, window: &str, index: &str) -> Session {
    let mut s = session(State::Interrupted);
    s.tmux_session = Some(tmux_session.to_owned());
    s.tmux_window_name = Some(window.to_owned());
    s.tmux_window_index = Some(index.to_owned());
    s
}

#[test]
fn a_recorded_placement_names_the_target_session_and_window() {
    let fake = ready();

    let plan = plan(
        &fake,
        &placed("my project", "api build", "3"),
        &Options::default(),
    )
    .unwrap();

    assert_eq!(plan.tmux_session, "my project");
    assert_eq!(plan.label, "api build");
    assert_eq!(plan.window_index, Some(3));
}

fn in_pane(session: &mut Session, index: &str, panes: &str, layout: &str) {
    session.tmux_pane_index = Some(index.to_owned());
    session.tmux_window_panes = Some(panes.to_owned());
    session.tmux_window_layout = Some(layout.to_owned());
}

#[test]
fn a_recorded_pane_is_carried_in_the_plan() {
    let fake = ready();
    let mut s = placed("my project", "api build", "3");
    in_pane(
        &mut s,
        "2",
        "3",
        "b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}",
    );

    let plan = plan(&fake, &s, &Options::default()).unwrap();

    assert_eq!(
        plan.pane,
        Some(PanePlacement {
            recorded_session: "my project".to_owned(),
            window_name: "api build".to_owned(),
            window_index: 3,
            pane_index: 2,
            window_panes: Some(3),
            window_layout: Some("b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}".to_owned()),
            grouped: false,
        })
    );
}

#[test]
fn an_override_keeps_the_recorded_session_in_the_pane_placement() {
    let fake = ready();
    fake.vars.set("CLAUDE_MONITOR_TMUX_SESSION", "everything");
    let mut s = placed("work", "api", "3");
    in_pane(&mut s, "0", "2", "l");

    let plan = plan(&fake, &s, &Options::default()).unwrap();

    assert_eq!(plan.tmux_session, "everything");
    assert_eq!(
        plan.pane.map(|p| p.recorded_session),
        Some("work".to_owned())
    );
}

#[test]
fn no_pane_placement_without_a_full_recorded_window_and_pane() {
    let fake = ready();
    let no_pane = placed("work", "api", "3");
    let mut no_window = session(State::Interrupted);
    in_pane(&mut no_window, "1", "2", "l");
    let mut bad_index = placed("work", "api", "x");
    in_pane(&mut bad_index, "1", "2", "l");

    for s in [no_pane, no_window, bad_index] {
        let plan = plan(&fake, &s, &Options::default()).unwrap();
        assert_eq!(plan.pane, None);
    }
}

#[test]
fn an_unreadable_pane_count_or_empty_layout_is_left_out() {
    let fake = ready();
    let mut s = placed("work", "api", "3");
    in_pane(&mut s, "1", "many", "");

    let plan = plan(&fake, &s, &Options::default()).unwrap();

    let pane = plan.pane.unwrap();
    assert_eq!((pane.window_panes, pane.window_layout), (None, None));
}

#[test]
fn without_a_placement_the_default_session_and_the_branch_label_apply() {
    let fake = ready();

    let plan = plan(&fake, &session(State::Interrupted), &Options::default()).unwrap();

    assert_eq!(plan.tmux_session, "agents");
    assert_eq!(plan.label, "feature");
    assert_eq!(plan.window_index, None);
}

#[test]
fn the_env_override_forces_one_session_but_keeps_the_window_name() {
    let fake = ready();
    fake.vars.set("CLAUDE_MONITOR_TMUX_SESSION", "everything");

    let plan = plan(&fake, &placed("work", "api", "3"), &Options::default()).unwrap();

    assert_eq!(plan.tmux_session, "everything");
    assert_eq!(plan.label, "api");
}

#[test]
fn the_config_file_override_forces_one_session_too() {
    let fake = ready();
    fake.fs.add_file(
        "/home/u/.config/claude-monitor/config.json",
        r#"{"tmuxSession":"everything"}"#,
    );

    let plan = plan(&fake, &placed("work", "api", "3"), &Options::default()).unwrap();

    assert_eq!(plan.tmux_session, "everything");
}
