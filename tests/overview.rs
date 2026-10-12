//! Overview queries: sections, ordering, flags and filters (FR-001..FR-003,
//! FR-019, FR-021). The done, dismissed and project sections are asserted in
//! the later project-grouping tasks.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use claude_monitor::model::{Session, State};
use claude_monitor::store::queries::{Flag, Overview, OverviewQuery, SectionName};
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
        Self { _dir: dir, store }
    }

    fn put(&self, id: &str, state: State, last_activity_at: &str) -> Session {
        self.put_with(id, state, last_activity_at, |_| {})
    }

    fn put_with(
        &self,
        id: &str,
        state: State,
        last_activity_at: &str,
        edit: impl FnOnce(&mut Session),
    ) -> Session {
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &format!("/work/{id}"),
            "2026-10-01T00:00:00Z",
        );
        session.state = state;
        session.last_activity_at = last_activity_at.to_owned();
        edit(&mut session);
        self.store.upsert_session(&session).unwrap();
        session
    }

    fn overview(&self, query: &OverviewQuery) -> Overview {
        self.store.overview(query).unwrap()
    }
}

fn ids(overview: &Overview, name: SectionName) -> Vec<&str> {
    overview
        .sections
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.sessions.iter().map(|e| e.id.as_str()).collect())
        .unwrap_or_default()
}

fn all_ids(overview: &Overview) -> Vec<&str> {
    let mut out: Vec<&str> = overview
        .sections
        .iter()
        .flat_map(|s| s.sessions.iter().map(|e| e.id.as_str()))
        .collect();
    out.sort_unstable();
    out
}

fn three_worktrees(fx: &Fixture) {
    fx.put_with("wt-a", State::Working, "2026-10-12T11:00:00Z", |s| {
        s.repo_root = Some("/home/u/git/nix-config".into());
        s.branch = Some("feature-a".into());
    });
    fx.put_with(
        "wt-b",
        State::NeedsPermission,
        "2026-10-12T10:00:00Z",
        |s| {
            s.repo_root = Some("/home/u/git/nix-config".into());
            s.branch = Some("feature-b".into());
        },
    );
    fx.put_with("wt-c", State::Interrupted, "2026-10-12T09:00:00Z", |s| {
        s.repo_root = Some("/home/u/git/nix-config".into());
        s.branch = Some("feature-c".into());
    });
    fx.put("old", State::Finished, "2026-10-12T08:00:00Z");
    fx.put("idle", State::Idle, "2026-10-12T07:00:00Z");
}

#[test]
fn mixed_states_land_in_exactly_one_of_active_interrupted_finished() {
    let fx = Fixture::new();
    three_worktrees(&fx);
    let overview = fx.overview(&OverviewQuery::default());

    assert_eq!(ids(&overview, SectionName::Interrupted), ["wt-c"]);
    assert_eq!(ids(&overview, SectionName::Finished), ["old"]);
    assert_eq!(
        all_ids(&overview),
        ["idle", "old", "wt-a", "wt-b", "wt-c"],
        "every session appears exactly once"
    );
    let names: Vec<_> = overview.sections.iter().map(|s| s.name).collect();
    assert_eq!(
        &names[..3],
        &[
            SectionName::Active,
            SectionName::Interrupted,
            SectionName::Finished
        ]
    );
}

#[test]
fn needs_permission_sorts_first_then_most_recent_activity() {
    let fx = Fixture::new();
    three_worktrees(&fx);
    let overview = fx.overview(&OverviewQuery::default());
    assert_eq!(
        ids(&overview, SectionName::Active),
        ["wt-b", "wt-a", "idle"],
        "needs_permission first even though wt-a is more recent"
    );
}

#[test]
fn entries_carry_repo_branch_running_and_description() {
    let fx = Fixture::new();
    fx.put_with("s1", State::Working, "2026-10-12T11:00:00Z", |s| {
        s.repo_root = Some("/home/u/git/nix-config".into());
        s.branch = Some("main".into());
        s.task_description = Some("fix the thing".into());
        s.left_off = Some("running tests".into());
    });
    fx.put("s2", State::Finished, "2026-10-12T11:00:00Z");
    let overview = fx.overview(&OverviewQuery::default());
    let active = &overview
        .sections
        .iter()
        .find(|s| s.name == SectionName::Active)
        .unwrap()
        .sessions;
    let entry = active.first().unwrap();
    assert_eq!(entry.id, "s1");
    assert_eq!(entry.agent_kind, "claude");
    assert_eq!(entry.agent_session_id, "agent-s1");
    assert_eq!(entry.repo.as_deref(), Some("nix-config"));
    assert_eq!(entry.branch.as_deref(), Some("main"));
    assert_eq!(entry.state, State::Working);
    assert!(entry.running);
    assert_eq!(entry.task_description.as_deref(), Some("fix the thing"));
    assert_eq!(entry.left_off.as_deref(), Some("running tests"));
    assert_eq!(entry.section, SectionName::Active);
    assert!(entry.flags.is_empty());
    let finished = overview
        .sections
        .iter()
        .find(|s| s.name == SectionName::Finished)
        .unwrap()
        .sessions
        .first()
        .unwrap();
    assert!(!finished.running);
}

#[test]
fn flags_mark_missing_worktree_parked_and_unknown_state() {
    let fx = Fixture::new();
    fx.put_with("gone", State::Interrupted, "2026-10-12T11:00:00Z", |s| {
        s.worktree_exists = false;
    });
    fx.put_with("parked", State::Idle, "2026-10-12T10:00:00Z", |s| {
        s.parked = true;
    });
    fx.put("passive", State::Unknown, "2026-10-12T09:00:00Z");
    let overview = fx.overview(&OverviewQuery::default());

    let flags_of = |name, id: &str| {
        overview
            .sections
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .sessions
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .flags
            .clone()
    };
    assert_eq!(
        flags_of(SectionName::Interrupted, "gone"),
        [Flag::WorktreeMissing]
    );
    assert_eq!(flags_of(SectionName::Active, "parked"), [Flag::Parked]);
    assert_eq!(
        flags_of(SectionName::Active, "passive"),
        [Flag::StateUnknown],
        "unknown state is shown with the active group but flagged"
    );
}

#[test]
fn state_filter_keeps_only_the_listed_states() {
    let fx = Fixture::new();
    three_worktrees(&fx);
    let overview = fx.overview(&OverviewQuery {
        states: vec![State::Working, State::Interrupted],
        ..OverviewQuery::default()
    });
    assert_eq!(all_ids(&overview), ["wt-a", "wt-c"]);
}

#[test]
fn agent_filter_matches_the_agent_kind() {
    let fx = Fixture::new();
    fx.put("c1", State::Working, "2026-10-12T11:00:00Z");
    fx.put_with("p1", State::Working, "2026-10-12T11:00:00Z", |s| {
        s.agent_kind = "copilot".into();
    });
    let overview = fx.overview(&OverviewQuery {
        agent: Some("copilot".into()),
        ..OverviewQuery::default()
    });
    assert_eq!(all_ids(&overview), ["p1"]);
}

#[test]
fn repo_filter_matches_the_repository_name() {
    let fx = Fixture::new();
    three_worktrees(&fx);
    fx.put_with("other", State::Working, "2026-10-12T11:30:00Z", |s| {
        s.repo_root = Some("/home/u/git/claude-monitor".into());
    });
    let overview = fx.overview(&OverviewQuery {
        repo: Some("claude-monitor".into()),
        ..OverviewQuery::default()
    });
    assert_eq!(all_ids(&overview), ["other"]);
}

#[test]
fn text_filter_searches_task_description_and_cwd_ignoring_case() {
    let fx = Fixture::new();
    fx.put_with("by-task", State::Working, "2026-10-12T11:00:00Z", |s| {
        s.task_description = Some("Migrate the Database".into());
    });
    fx.put_with("by-cwd", State::Working, "2026-10-12T10:00:00Z", |s| {
        s.cwd = "/work/database-tools".into();
    });
    fx.put("neither", State::Working, "2026-10-12T09:00:00Z");
    let overview = fx.overview(&OverviewQuery {
        text: Some("database".into()),
        ..OverviewQuery::default()
    });
    assert_eq!(all_ids(&overview), ["by-cwd", "by-task"]);
}

#[test]
fn dismissed_sessions_are_hidden_unless_requested() {
    let fx = Fixture::new();
    fx.put("shown", State::Working, "2026-10-12T11:00:00Z");
    fx.put_with("hidden", State::Working, "2026-10-12T10:00:00Z", |s| {
        s.dismissed = true;
    });
    let default = fx.overview(&OverviewQuery::default());
    assert_eq!(all_ids(&default), ["shown"]);

    let included = fx.overview(&OverviewQuery {
        include_dismissed: true,
        ..OverviewQuery::default()
    });
    assert_eq!(all_ids(&included), ["hidden", "shown"]);
    let hidden = included
        .sections
        .iter()
        .flat_map(|s| &s.sessions)
        .find(|e| e.id == "hidden")
        .unwrap();
    assert!(hidden.flags.contains(&Flag::Dismissed));
}

#[test]
fn finished_sessions_are_listed_whatever_their_age() {
    let fx = Fixture::new();
    fx.put("recent", State::Finished, "2026-10-10T12:00:00Z");
    fx.put("stale-1", State::Finished, "2026-10-01T12:00:00Z");
    fx.put("ninety-days", State::Finished, "2026-07-14T12:00:00Z");

    let overview = fx.overview(&OverviewQuery::default());

    assert_eq!(
        ids(&overview, SectionName::Finished),
        ["recent", "stale-1", "ninety-days"],
        "most recent first, none dropped"
    );
}

#[test]
fn an_undecodable_row_degrades_instead_of_failing_the_query() {
    let fx = Fixture::new();
    fx.put("good", State::Working, "2026-10-12T11:00:00Z");
    fx.put("bad", State::Working, "2026-10-12T10:00:00Z");
    let conn = fx.store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();

    let overview = fx.overview(&OverviewQuery::default());
    assert_eq!(all_ids(&overview), ["bad", "good"]);
    let bad = overview
        .sections
        .iter()
        .flat_map(|s| &s.sessions)
        .find(|e| e.id == "bad")
        .unwrap();
    assert!(bad.flags.contains(&Flag::Degraded));
    assert!(bad.error.as_deref().unwrap().contains("bogus"));
    let good = overview
        .sections
        .iter()
        .flat_map(|s| &s.sessions)
        .find(|e| e.id == "good")
        .unwrap();
    assert!(!good.flags.contains(&Flag::Degraded));
    assert!(good.error.is_none());
}

#[test]
#[ignore = "T062a: unseen section"]
fn unknown_sessions_get_their_own_unseen_section_newest_first() {
    let fx = Fixture::new();
    fx.put("live", State::Working, "2026-10-12T11:00:00Z");
    fx.put("old-unknown", State::Unknown, "2026-09-01T00:00:00Z");
    fx.put("new-unknown", State::Unknown, "2026-10-11T00:00:00Z");

    let overview = fx.overview(&OverviewQuery::default());

    assert_eq!(ids(&overview, SectionName::Active), ["live"]);
    assert_eq!(
        ids(&overview, SectionName::Unseen),
        ["new-unknown", "old-unknown"]
    );
    let unseen = overview
        .sections
        .iter()
        .flat_map(|s| &s.sessions)
        .find(|e| e.id == "new-unknown")
        .unwrap();
    assert_eq!(unseen.section, SectionName::Unseen);
    assert!(!unseen.running, "unseen is never counted as active");
}

#[test]
#[ignore = "T062a: unseen section"]
fn a_dismissed_unknown_session_is_dismissed_not_unseen() {
    let fx = Fixture::new();
    fx.put_with("gone", State::Unknown, "2026-10-12T11:00:00Z", |s| {
        s.dismissed = true;
    });

    let overview = fx.overview(&OverviewQuery {
        include_dismissed: true,
        ..OverviewQuery::default()
    });

    assert_eq!(ids(&overview, SectionName::Dismissed), ["gone"]);
    assert!(ids(&overview, SectionName::Unseen).is_empty());
}

#[test]
#[ignore = "T062a: unseen section"]
fn an_undecodable_row_stays_in_active_not_unseen() {
    let fx = Fixture::new();
    fx.put("bad", State::Working, "2026-10-12T10:00:00Z");
    let conn = fx.store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();

    let overview = fx.overview(&OverviewQuery::default());

    assert_eq!(ids(&overview, SectionName::Active), ["bad"]);
    assert!(ids(&overview, SectionName::Unseen).is_empty());
}

#[test]
#[ignore = "benchmark"]
fn five_hundred_sessions_build_an_overview() {
    let fx = Fixture::new();
    let states = [
        State::Working,
        State::Idle,
        State::Interrupted,
        State::Finished,
    ];
    for i in 0..500_usize {
        let state = states[i % states.len()];
        fx.put(&format!("s{i}"), state, "2026-10-12T11:00:00Z");
    }
    let started = std::time::Instant::now();
    let overview = fx.overview(&OverviewQuery::default());
    assert_eq!(all_ids(&overview).len(), 500);
    assert!(started.elapsed() < std::time::Duration::from_millis(250));
}
