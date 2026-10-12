#![allow(clippy::unwrap_used, clippy::expect_used)]

use claude_monitor::model::{
    DoneError, Event, Liveness, NotificationKind, Session, StartSource, State,
};

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T10:05:00Z";
const T2: &str = "2026-10-12T10:10:00Z";

fn discovered() -> Session {
    Session::new("s1", "claude", "agent-1", "/work/repo", T0)
}

fn in_state(state: State) -> Session {
    let mut s = discovered();
    match state {
        State::Unknown => {}
        State::Idle => {
            s.apply_event(&start(StartSource::Startup), T0);
        }
        State::Working => {
            s.apply_event(&Event::UserPromptSubmit, T0);
        }
        State::NeedsPermission => {
            s.apply_event(&Event::PermissionRequest, T0);
        }
        State::Finished => {
            s.apply_event(&end("logout"), T0);
        }
        State::Interrupted => {
            s.apply_event(&Event::UserPromptSubmit, T0);
            s.reconcile(Liveness::ProcessGone, T0);
        }
    }
    assert_eq!(s.state, state, "fixture must reach {state:?}");
    s
}

fn start(source: StartSource) -> Event {
    Event::SessionStart { source }
}

fn end(reason: &str) -> Event {
    Event::SessionEnd {
        reason: reason.to_owned(),
    }
}

const ALL: [State; 6] = [
    State::Working,
    State::NeedsPermission,
    State::Idle,
    State::Finished,
    State::Interrupted,
    State::Unknown,
];

#[test]
#[ignore = "T011"]
fn passive_discovery_is_unknown_until_first_hook() {
    let mut s = discovered();
    assert_eq!(s.state, State::Unknown);
    s.apply_event(&start(StartSource::Startup), T1);
    assert_eq!(s.state, State::Idle);
}

#[test]
#[ignore = "T011"]
fn session_start_goes_idle() {
    let mut s = discovered();
    let out = s.apply_event(&start(StartSource::Startup), T1);
    assert_eq!(s.state, State::Idle);
    assert!(!out.lineage_continuation);
}

#[test]
#[ignore = "T011"]
fn prompt_and_tool_events_go_working() {
    for ev in [
        Event::UserPromptSubmit,
        Event::PreToolUse,
        Event::PostToolUse,
    ] {
        let mut s = in_state(State::Idle);
        s.apply_event(&ev, T1);
        assert_eq!(s.state, State::Working, "{ev:?}");
    }
}

#[test]
#[ignore = "T011"]
fn permission_request_and_notifications_need_permission() {
    let mut s = in_state(State::Working);
    s.apply_event(&Event::PermissionRequest, T1);
    assert_eq!(s.state, State::NeedsPermission);

    for kind in [
        NotificationKind::PermissionPrompt,
        NotificationKind::ElicitationDialog,
    ] {
        let mut s = in_state(State::Working);
        s.apply_event(&Event::Notification(kind), T1);
        assert_eq!(s.state, State::NeedsPermission, "{kind:?}");
    }
}

#[test]
#[ignore = "T011"]
fn stop_and_idle_notifications_go_idle() {
    let mut s = in_state(State::Working);
    s.apply_event(&Event::Stop, T1);
    assert_eq!(s.state, State::Idle);

    for kind in [
        NotificationKind::IdlePrompt,
        NotificationKind::AgentIdle,
        NotificationKind::AgentCompleted,
    ] {
        let mut s = in_state(State::Working);
        s.apply_event(&Event::Notification(kind), T1);
        assert_eq!(s.state, State::Idle, "{kind:?}");
    }
}

#[test]
#[ignore = "T011"]
fn unrelated_notification_changes_nothing() {
    let mut s = in_state(State::Working);
    s.apply_event(&Event::Notification(NotificationKind::Other), T1);
    assert_eq!(s.state, State::Working);
    assert_eq!(s.state_since, T0);
}

#[test]
#[ignore = "T011"]
fn normal_session_end_is_finished_without_continuation() {
    for reason in ["logout", "prompt_input_exit", "complete"] {
        let mut s = in_state(State::Working);
        let out = s.apply_event(&end(reason), T1);
        assert_eq!(s.state, State::Finished, "{reason}");
        assert_eq!(s.end_reason.as_deref(), Some(reason));
        assert!(!out.lineage_continuation, "{reason}");
    }
}

#[test]
#[ignore = "T011"]
fn session_end_clear_is_finished_with_lineage_continuation() {
    let mut s = in_state(State::Idle);
    let out = s.apply_event(&end("clear"), T1);
    assert_eq!(s.state, State::Finished);
    assert_eq!(s.end_reason.as_deref(), Some("clear"));
    assert!(out.lineage_continuation);
}

#[test]
#[ignore = "T011"]
fn copilot_abnormal_end_is_interrupted() {
    for reason in ["error", "abort", "timeout"] {
        let mut s = Session::new("c1", "copilot", "uuid-1", "/work/repo", T0);
        s.apply_event(&Event::UserPromptSubmit, T0);
        let out = s.apply_event(&end(reason), T1);
        assert_eq!(s.state, State::Interrupted, "{reason}");
        assert_eq!(s.end_reason.as_deref(), Some(reason));
        assert!(!out.lineage_continuation, "{reason}");
    }
}

#[test]
#[ignore = "T011"]
fn late_events_do_not_reopen_a_finished_session() {
    let mut s = in_state(State::Finished);
    s.apply_event(&Event::Stop, T1);
    s.apply_event(&Event::PostToolUse, T1);
    assert_eq!(s.state, State::Finished);
}

#[test]
#[ignore = "T011"]
fn hook_on_interrupted_session_proves_it_is_alive() {
    let mut s = in_state(State::Interrupted);
    s.apply_event(&Event::PostToolUse, T1);
    assert_eq!(s.state, State::Working);
}

#[test]
#[ignore = "T011"]
fn process_gone_interrupts_every_non_finished_state() {
    for state in ALL {
        let mut s = in_state(state);
        s.reconcile(Liveness::ProcessGone, T1);
        let expected = if state == State::Finished {
            State::Finished
        } else {
            State::Interrupted
        };
        assert_eq!(s.state, expected, "from {state:?}");
    }
}

#[test]
#[ignore = "T011"]
fn boot_change_interrupts_every_non_finished_state() {
    for state in ALL {
        let mut s = in_state(state);
        s.reconcile(Liveness::BootChanged, T1);
        let expected = if state == State::Finished {
            State::Finished
        } else {
            State::Interrupted
        };
        assert_eq!(s.state, expected, "from {state:?}");
    }
}

#[test]
#[ignore = "T011"]
fn alive_reconcile_changes_nothing() {
    for state in ALL {
        let mut s = in_state(state);
        let before = s.clone();
        s.reconcile(Liveness::Alive, T1);
        assert_eq!(s, before, "from {state:?}");
    }
}

#[test]
#[ignore = "T011"]
fn reconcile_is_idempotent() {
    for liveness in [Liveness::ProcessGone, Liveness::BootChanged] {
        for state in ALL {
            let mut s = in_state(state);
            s.reconcile(liveness, T1);
            let once = s.clone();
            s.reconcile(liveness, T2);
            assert_eq!(s, once, "{liveness:?} from {state:?}");
        }
    }
}

#[test]
#[ignore = "T011"]
fn resume_returns_interrupted_and_finished_to_idle_on_the_same_record() {
    for state in [State::Interrupted, State::Finished] {
        let mut s = in_state(state);
        let id = s.id.clone();
        let agent_session_id = s.agent_session_id.clone();
        let out = s.apply_event(&start(StartSource::Resume), T1);
        assert_eq!(s.state, State::Idle, "from {state:?}");
        assert_eq!(s.id, id);
        assert_eq!(s.agent_session_id, agent_session_id);
        assert_eq!(s.end_reason, None);
        assert!(!out.lineage_continuation);
    }
}

#[test]
#[ignore = "T011"]
fn state_since_is_kept_when_state_does_not_change() {
    let mut s = in_state(State::Working);
    s.apply_event(&Event::PreToolUse, T1);
    assert_eq!(s.state, State::Working);
    assert_eq!(s.state_since, T0);
    assert_eq!(s.last_activity_at, T1);

    s.apply_event(&Event::Stop, T2);
    assert_eq!(s.state, State::Idle);
    assert_eq!(s.state_since, T2);
}

#[test]
#[ignore = "T011"]
fn reconcile_sets_state_since_only_on_change() {
    let mut s = in_state(State::Working);
    s.reconcile(Liveness::ProcessGone, T1);
    assert_eq!(s.state_since, T1);

    let mut f = in_state(State::Finished);
    f.reconcile(Liveness::BootChanged, T1);
    assert_eq!(f.state_since, T0);
}

#[test]
#[ignore = "T011"]
fn running_is_true_only_for_live_active_states() {
    for state in ALL {
        let s = in_state(state);
        let active = matches!(state, State::Working | State::NeedsPermission | State::Idle);
        assert_eq!(s.running(Liveness::Alive), active, "alive, {state:?}");
        assert!(!s.running(Liveness::ProcessGone), "gone, {state:?}");
        assert!(!s.running(Liveness::BootChanged), "reboot, {state:?}");
    }
}

#[test]
#[ignore = "T011"]
fn done_can_be_set_only_on_a_finished_session() {
    let mut s = in_state(State::Finished);
    s.mark_done(T1).unwrap();
    assert_eq!(s.done_at.as_deref(), Some(T1));
    assert_eq!(s.state, State::Finished);

    for state in ALL.into_iter().filter(|st| *st != State::Finished) {
        let mut s = in_state(state);
        let err = s.mark_done(T1).unwrap_err();
        assert_eq!(err, DoneError::NotFinished { state }, "from {state:?}");
        assert_eq!(s.done_at, None);
        assert!(err.to_string().contains("finished"));
    }
}

#[test]
#[ignore = "T011"]
fn respawn_request_clears_done() {
    let mut s = in_state(State::Finished);
    s.mark_done(T1).unwrap();
    s.clear_done_on_respawn();
    assert_eq!(s.done_at, None);
}

#[test]
#[ignore = "T011"]
fn reconcile_never_touches_done() {
    let mut s = in_state(State::Finished);
    s.mark_done(T1).unwrap();
    s.reconcile(Liveness::BootChanged, T2);
    assert_eq!(s.done_at.as_deref(), Some(T1));
    assert_eq!(s.state, State::Finished);
}
