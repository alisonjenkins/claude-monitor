//! Restoring sessions that shared a tmux window as split panes: how a batch
//! groups them, orders them and opens them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{
    plan_batch, start_batch, Item, PlanFlags, Planned, StartedItem, SPAWN_FAILED,
};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::{FakeEnv, TmuxCall};
use tempfile::TempDir;

const AT: &str = "2026-10-12T00:00:00Z";
const LAYOUT: &str = "b25d,200x50,0,0{100x50,0,0,0,99x50,101,0,1}";

struct Rig {
    _dir: TempDir,
    store: Store,
    fake: FakeEnv,
}

impl Rig {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        Self {
            store: Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap(),
            _dir: dir,
            fake,
        }
    }

    /// Stores an interrupted session in `/w/<id>` that can be resumed.
    fn add(&self, id: &str, place: impl FnOnce(&mut Session)) {
        self.fake.fs.add_dir(format!("/w/{id}"));
        self.fake.fs.add_file(
            format!("/home/u/.claude/projects/-w-{id}/agent-{id}.jsonl"),
            "{}\n",
        );
        self.add_missing_directory(id, place);
    }

    /// Like `add`, but the session's directory does not exist, so it is refused.
    fn add_missing_directory(&self, id: &str, place: impl FnOnce(&mut Session)) {
        let mut s = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &format!("/w/{id}"),
            AT,
        );
        s.state = State::Interrupted;
        place(&mut s);
        self.store.upsert_session(&s).unwrap();
    }

    fn plan(&self, ids: &[&str]) -> Planned {
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        plan_batch(&self.store, &self.fake.env, &ids, &PlanFlags::default()).unwrap()
    }

    fn start(&self, ids: &[&str]) -> Vec<Item> {
        start_batch(&self.store, &self.fake.env, self.plan(ids))
    }

    /// The tmux calls that open and shape windows, without the lookups.
    fn calls(&self) -> Vec<TmuxCall> {
        self.fake
            .tmux
            .calls()
            .into_iter()
            .filter(|call| !matches!(call, TmuxCall::ListPanes | TmuxCall::EnsureSession { .. }))
            .collect()
    }
}

fn started(item: &Item) -> &StartedItem {
    match item {
        Item::Started(item) => item,
        Item::Refused(refused) => panic!("expected a start, got {refused:?}"),
    }
}

fn argv(id: &str) -> Vec<String> {
    ["claude", "--resume", &format!("agent-{id}")]
        .map(str::to_owned)
        .to_vec()
}

fn spawn(session: &str, label: &str, id: &str) -> TmuxCall {
    TmuxCall::SpawnWindow {
        session: session.to_owned(),
        label: label.to_owned(),
        cwd: format!("/w/{id}").into(),
        argv: argv(id),
    }
}

fn split(first_pane: &str, id: &str) -> TmuxCall {
    TmuxCall::SplitPane {
        target: first_pane.to_owned(),
        cwd: format!("/w/{id}").into(),
        argv: argv(id),
    }
}

fn layout(first_pane: &str, layout: &str) -> TmuxCall {
    TmuxCall::SelectLayout {
        target: first_pane.to_owned(),
        layout: layout.to_owned(),
    }
}

fn pane_of(
    tmux_session: &'static str,
    window: &'static str,
    index: &'static str,
    pane: &'static str,
    panes: &'static str,
) -> impl FnOnce(&mut Session) {
    move |s| {
        s.tmux_session = Some(tmux_session.to_owned());
        s.tmux_window_name = Some(window.to_owned());
        s.tmux_window_index = Some(index.to_owned());
        s.tmux_pane_index = Some(pane.to_owned());
        s.tmux_window_panes = Some(panes.to_owned());
        s.tmux_window_layout = Some(LAYOUT.to_owned());
    }
}

fn ids(planned: &Planned) -> Vec<&str> {
    planned.iter().map(|(s, _)| s.id.as_str()).collect()
}

fn targets(planned: &Planned) -> Vec<String> {
    planned
        .iter()
        .map(|(_, plan)| plan.as_ref().unwrap().tmux_target())
        .collect()
}

#[test]
fn panes_of_one_window_are_ordered_by_pane_index_and_marked_grouped() {
    let rig = Rig::new();
    rig.add("c", pane_of("work", "api", "1", "2", "3"));
    rig.add("a", pane_of("work", "api", "1", "0", "3"));
    rig.add("b", pane_of("work", "api", "1", "1", "3"));

    let planned = rig.plan(&["c", "a", "b"]);

    assert_eq!(ids(&planned), ["a", "b", "c"]);
    assert_eq!(
        targets(&planned),
        ["work:api.0", "work:api.1", "work:api.2"]
    );
}

#[test]
fn sessions_in_different_windows_stay_separate() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "2"));
    rig.add("b", pane_of("work", "web", "2", "0", "2"));
    rig.add("c", pane_of("work", "api", "1", "1", "2"));
    rig.add("d", pane_of("play", "api", "1", "0", "1"));

    let planned = rig.plan(&["a", "b", "c", "d"]);

    assert_eq!(ids(&planned), ["d", "a", "c", "b"]);
    assert_eq!(
        targets(&planned),
        ["play:api", "work:api.0", "work:api.1", "work:web"]
    );
}

#[test]
fn a_window_that_reopens_before_a_later_one_keeps_the_window_index_order() {
    let rig = Rig::new();
    rig.add("late", pane_of("work", "web", "5", "0", "2"));
    rig.add("late2", pane_of("work", "web", "5", "1", "2"));
    rig.add("early", pane_of("work", "api", "1", "0", "2"));
    rig.add("early2", pane_of("work", "api", "1", "1", "2"));

    let planned = rig.plan(&["late", "late2", "early", "early2"]);

    assert_eq!(ids(&planned), ["early", "early2", "late", "late2"]);
}

#[test]
fn a_session_without_pane_placement_is_unchanged() {
    let rig = Rig::new();
    rig.add("a", |s| {
        s.tmux_session = Some("work".to_owned());
        s.tmux_window_name = Some("api".to_owned());
        s.tmux_window_index = Some("1".to_owned());
    });
    rig.add("b", |s| {
        s.tmux_session = Some("work".to_owned());
        s.tmux_window_name = Some("api".to_owned());
        s.tmux_window_index = Some("1".to_owned());
    });
    rig.add("c", |_| {});

    let planned = rig.plan(&["a", "b", "c"]);

    assert_eq!(ids(&planned), ["c", "a", "b"]);
    assert_eq!(targets(&planned), ["agents:c", "work:api", "work:api"]);
}

#[test]
fn a_refused_sibling_leaves_a_group_of_one_and_keeps_its_place() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "2"));
    rig.add_missing_directory("gone", pane_of("work", "api", "1", "1", "2"));

    let planned = rig.plan(&["gone", "a"]);

    assert_eq!(ids(&planned), ["gone", "a"]);
    assert!(planned[0].1.is_err());
    assert_eq!(planned[1].1.as_ref().unwrap().tmux_target(), "work:api");
}

#[test]
fn an_override_groups_only_panes_recorded_in_the_same_tmux_session() {
    let rig = Rig::new();
    rig.fake
        .vars
        .set("CLAUDE_MONITOR_TMUX_SESSION", "everything");
    rig.add("a", pane_of("work", "api", "1", "0", "2"));
    rig.add("b", pane_of("play", "api", "1", "0", "2"));
    rig.add("c", pane_of("work", "api", "1", "1", "2"));

    let planned = rig.plan(&["a", "b", "c"]);

    assert_eq!(ids(&planned), ["a", "c", "b"]);
    assert_eq!(
        targets(&planned),
        ["everything:api.0", "everything:api.1", "everything:api"]
    );
}

#[test]
fn three_panes_open_as_one_window_in_pane_order_with_the_recorded_layout() {
    let rig = Rig::new();
    rig.add("c", pane_of("work", "api", "1", "2", "3"));
    rig.add("a", pane_of("work", "api", "1", "0", "3"));
    rig.add("b", pane_of("work", "api", "1", "1", "3"));

    let items = rig.start(&["c", "a", "b"]);

    let first = started(&items[0]).started.pane_id.clone();
    assert_eq!(
        rig.calls(),
        [
            spawn("work", "api", "a"),
            split(&first, "b"),
            split(&first, "c"),
            layout(&first, LAYOUT),
        ]
    );
    let sessions: Vec<_> = items
        .iter()
        .map(|i| started(i).started.session_id.as_str())
        .collect();
    assert_eq!(sessions, ["a", "b", "c"]);
    for item in &items {
        let item = started(item);
        let request = rig
            .store
            .find_open_respawn_request(&item.started.pane_id)
            .unwrap()
            .expect("request keyed by the pane's own id");
        assert_eq!(request.session_id, item.started.session_id);
    }
    assert_eq!(
        started(&items[2]).started.window_id,
        started(&items[0]).started.window_id
    );
}

#[test]
fn a_window_that_had_other_panes_is_tiled_not_given_the_recorded_layout() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "4"));
    rig.add("b", pane_of("work", "api", "1", "1", "4"));
    rig.add("c", pane_of("work", "api", "1", "3", "4"));

    let items = rig.start(&["a", "b", "c"]);

    let first = started(&items[0]).started.pane_id.clone();
    assert_eq!(rig.calls().last(), Some(&layout(&first, "tiled")));
}

#[test]
fn a_refused_pane_sibling_makes_the_rest_tiled() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "3"));
    rig.add("b", pane_of("work", "api", "1", "1", "3"));
    rig.add_missing_directory("c", pane_of("work", "api", "1", "2", "3"));

    let items = rig.start(&["a", "b", "c"]);

    assert!(matches!(items[2], Item::Refused(_)));
    let first = started(&items[0]).started.pane_id.clone();
    assert_eq!(
        rig.calls(),
        [
            spawn("work", "api", "a"),
            split(&first, "b"),
            layout(&first, "tiled")
        ]
    );
}

#[test]
fn sessions_in_different_windows_each_open_their_own_window() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "1"));
    rig.add("b", pane_of("work", "web", "2", "0", "1"));

    rig.start(&["a", "b"]);

    assert_eq!(
        rig.calls(),
        [spawn("work", "api", "a"), spawn("work", "web", "b")]
    );
}

#[test]
fn a_session_without_placement_opens_a_plain_window() {
    let rig = Rig::new();
    rig.add("a", |_| {});

    rig.start(&["a"]);

    assert_eq!(rig.calls(), [spawn("agents", "a", "a")]);
}

#[test]
fn when_the_first_pane_fails_the_siblings_open_their_own_windows() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "3"));
    rig.add("b", pane_of("work", "api", "1", "1", "3"));
    rig.add("c", pane_of("work", "api", "1", "2", "3"));
    rig.fake.tmux.fail_next_spawn_window("no server");

    let items = rig.start(&["a", "b", "c"]);

    match &items[0] {
        Item::Refused(refused) => assert_eq!(refused.code, SPAWN_FAILED),
        Item::Started(_) => panic!("the first pane should have failed"),
    }
    started(&items[1]);
    started(&items[2]);
    assert_eq!(
        rig.calls(),
        [
            spawn("work", "api", "a"),
            spawn("work", "api", "b"),
            spawn("work", "api", "c"),
        ]
    );
}

#[test]
fn a_failed_split_refuses_that_pane_and_still_shapes_what_opened() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "3"));
    rig.add("b", pane_of("work", "api", "1", "1", "3"));
    rig.add("c", pane_of("work", "api", "1", "2", "3"));
    rig.fake.tmux.fail_splits("no space for new pane");

    let items = rig.start(&["a", "b", "c"]);

    started(&items[0]);
    for item in &items[1..] {
        match item {
            Item::Refused(refused) => {
                assert_eq!(refused.code, SPAWN_FAILED);
                assert!(
                    refused.message.contains("no space for new pane"),
                    "{refused:?}"
                );
            }
            Item::Started(_) => panic!("the split should have failed"),
        }
    }
    assert!(!rig
        .calls()
        .iter()
        .any(|call| matches!(call, TmuxCall::SelectLayout { .. })));
}

#[test]
fn a_layout_failure_neither_fails_nor_undoes_the_respawn() {
    let rig = Rig::new();
    rig.add("a", pane_of("work", "api", "1", "0", "2"));
    rig.add("b", pane_of("work", "api", "1", "1", "2"));
    rig.fake.tmux.fail_layouts("bad layout");

    let items = rig.start(&["a", "b"]);

    let notices: Vec<_> = items
        .iter()
        .map(|item| started(item).plan.notices.clone())
        .collect();
    assert!(notices[0].is_empty(), "{notices:?}");
    assert_eq!(notices[1].len(), 1, "{notices:?}");
    assert!(notices[1][0].contains("bad layout"), "{notices:?}");
    for item in &items {
        let pane = &started(item).started.pane_id;
        assert!(rig.store.find_open_respawn_request(pane).unwrap().is_some());
    }
}

#[test]
fn an_override_opens_one_window_per_recorded_session_with_panes_inside() {
    let rig = Rig::new();
    rig.fake
        .vars
        .set("CLAUDE_MONITOR_TMUX_SESSION", "everything");
    rig.add("a", pane_of("work", "api", "1", "0", "2"));
    rig.add("b", pane_of("play", "api", "1", "0", "1"));
    rig.add("c", pane_of("work", "api", "1", "1", "2"));

    let items = rig.start(&["a", "b", "c"]);

    let first = started(&items[0]).started.pane_id.clone();
    assert_eq!(
        rig.calls(),
        [
            spawn("everything", "api", "a"),
            split(&first, "c"),
            layout(&first, LAYOUT),
            spawn("everything", "api", "b"),
        ]
    );
}
