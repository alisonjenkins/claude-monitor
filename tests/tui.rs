#![cfg(feature = "tui")]
// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;
use std::time::Duration;

use claude_monitor::feed::{ChangeFeed, FakeHandle, FakeSource, ManualClock};
use claude_monitor::hook::{ingest, GitInfo, GitProbe, HookContext};
use claude_monitor::model::State;
use claude_monitor::store::queries::{Flag, Overview, OverviewEntry, Section, SectionName};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeClock;
use claude_monitor::tui::app::{update, App, Effect, Msg, Snapshot};
use claude_monitor::tui::dashboard::{pump, warning_line};
use claude_monitor::tui::view::view;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const TWO_MINUTES_AGO: &str = "2026-10-12T11:58:00Z";
const WIDE: (u16, u16) = (100, 30);

fn entry(id: &str, state: State, section: SectionName) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: "claude".to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: Some("nix-config".to_owned()),
        branch: Some("main".to_owned()),
        cwd: "/work/nix-config".to_owned(),
        state,
        running: state.is_active(),
        last_activity_at: TWO_MINUTES_AGO.to_owned(),
        task_description: Some(format!("task of {id}")),
        left_off: None,
        project: None,
        flags: Vec::new(),
        section,
        error: None,
    }
}

fn active(id: &str, state: State) -> OverviewEntry {
    entry(id, state, SectionName::Active)
}

fn overview(parts: Vec<(SectionName, Vec<OverviewEntry>)>) -> Overview {
    Overview {
        sections: parts
            .into_iter()
            .map(|(name, sessions)| Section { name, sessions })
            .collect(),
    }
}

fn active_only(entries: Vec<OverviewEntry>) -> Overview {
    overview(vec![(SectionName::Active, entries)])
}

fn snapshot(overview: Overview, panes: &[(&str, &str)]) -> Snapshot {
    Snapshot {
        overview,
        panes: panes
            .iter()
            .map(|(id, pane)| ((*id).to_owned(), (*pane).to_owned()))
            .collect(),
        now: NOW.to_owned(),
    }
}

fn refresh(app: App, overview: Overview) -> (App, Vec<Effect>) {
    update(app, Msg::Refreshed(snapshot(overview, &[])))
}

fn loaded(overview: Overview) -> App {
    refresh(App::new(), overview).0
}

fn press(app: App, code: KeyCode) -> (App, Vec<Effect>) {
    update(app, Msg::Key(code, KeyModifiers::NONE))
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|e| e.id.clone()).collect()
}

fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view(app, frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn wide(app: &App) -> String {
    render(app, WIDE.0, WIDE.1)
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not rendered in:\n{text}"))
}

#[test]
fn sections_render_in_order_with_headers() {
    let app = loaded(overview(vec![
        (SectionName::Active, vec![active("a", State::Working)]),
        (
            SectionName::Interrupted,
            vec![entry("i", State::Interrupted, SectionName::Interrupted)],
        ),
        (
            SectionName::Finished,
            vec![entry("f", State::Finished, SectionName::Finished)],
        ),
        (
            SectionName::Done,
            vec![entry("d", State::Finished, SectionName::Done)],
        ),
    ]));

    let text = wide(&app);

    let order = [
        position(&text, "Active (1)"),
        position(&text, "Interrupted (1)"),
        position(&text, "Finished (1)"),
        position(&text, "Done (1)"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}\n{text}");
}

#[test]
fn needs_permission_rows_come_first_and_are_marked() {
    let app = loaded(active_only(vec![
        active("p", State::NeedsPermission),
        active("w", State::Working),
        active("i", State::Idle),
    ]));

    let text = wide(&app);

    assert_eq!(ids(&app), ["p", "w", "i"]);
    assert!(position(&text, "PERMISSION") < position(&text, "WORKING"));
    assert!(text.contains("IDLE"), "{text}");
}

#[test]
fn every_state_has_its_own_label() {
    let app = loaded(overview(vec![
        (
            SectionName::Active,
            vec![active("w", State::Working), active("u", State::Unknown)],
        ),
        (
            SectionName::Interrupted,
            vec![entry("i", State::Interrupted, SectionName::Interrupted)],
        ),
        (
            SectionName::Finished,
            vec![entry("f", State::Finished, SectionName::Finished)],
        ),
    ]));

    let text = wide(&app);

    for label in ["WORKING", "UNKNOWN", "INTERRUPTED", "FINISHED"] {
        assert!(text.contains(label), "{label} missing:\n{text}");
    }
    assert!(!text.contains("PERMISSION"), "{text}");
}

#[test]
fn rows_show_agent_repo_branch_state_age_and_task() {
    let mut e = active("a", State::Idle);
    e.agent_kind = "copilot".to_owned();
    e.repo = Some("claude-monitor".to_owned());
    e.branch = Some("feat-tui".to_owned());
    e.task_description = Some("Fix the build".to_owned());
    let app = loaded(active_only(vec![e]));

    let text = wide(&app);

    for expected in [
        "copilot",
        "claude-monitor/feat-tui",
        "IDLE",
        "2m",
        "Fix the build",
    ] {
        assert!(text.contains(expected), "{expected} missing:\n{text}");
    }
}

#[test]
fn a_row_without_a_repo_shows_its_directory() {
    let mut e = active("a", State::Idle);
    e.repo = None;
    e.branch = None;
    e.cwd = "/tmp/scratch".to_owned();
    let app = loaded(active_only(vec![e]));

    assert!(wide(&app).contains("/tmp/scratch"));
}

#[test]
fn j_k_and_arrows_move_the_selection_and_clamp_at_both_ends() {
    let app = loaded(active_only(vec![
        active("a", State::Idle),
        active("b", State::Idle),
        active("c", State::Idle),
    ]));
    assert_eq!(app.selected_id(), Some("a"));

    let steps = [
        (KeyCode::Char('j'), "b"),
        (KeyCode::Down, "c"),
        (KeyCode::Char('j'), "c"),
        (KeyCode::Char('k'), "b"),
        (KeyCode::Up, "a"),
        (KeyCode::Char('k'), "a"),
    ];
    let mut app = app;
    for (code, expected) in steps {
        let (next, effects) = press(app, code);
        assert_eq!(next.selected_id(), Some(expected), "after {code:?}");
        assert!(effects.is_empty());
        app = next;
    }
}

#[test]
fn selection_skips_section_headers() {
    let app = loaded(overview(vec![
        (SectionName::Active, vec![active("a", State::Idle)]),
        (
            SectionName::Finished,
            vec![entry("f", State::Finished, SectionName::Finished)],
        ),
    ]));

    let (app, _) = press(app, KeyCode::Char('j'));

    assert_eq!(app.selected_id(), Some("f"));
}

#[test]
fn enter_on_a_session_with_a_pane_jumps_to_it() {
    let (app, _) = update(
        App::new(),
        Msg::Refreshed(snapshot(
            active_only(vec![active("a", State::Idle), active("b", State::Idle)]),
            &[("a", "%3")],
        )),
    );

    let (_, effects) = press(app, KeyCode::Enter);

    assert_eq!(
        effects,
        [Effect::JumpToPane {
            pane: "%3".to_owned()
        }]
    );
}

#[test]
fn enter_without_a_pane_sets_a_status_message_and_no_effect() {
    let app = loaded(active_only(vec![active("a", State::Idle)]));

    let (app, effects) = press(app, KeyCode::Enter);

    assert!(effects.is_empty());
    let status = app.status().expect("a status message").to_owned();
    assert!(status.contains("pane"), "{status}");
    assert!(wide(&app).contains(&status));
}

#[test]
fn enter_and_navigation_on_an_empty_list_do_not_panic() {
    let mut app = loaded(active_only(Vec::new()));
    for code in [
        KeyCode::Enter,
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('d'),
    ] {
        let (next, effects) = press(app, code);
        assert!(effects.is_empty(), "{code:?}");
        app = next;
    }
    assert_eq!(app.selected_id(), None);
}

#[test]
fn d_hides_the_selected_session_until_its_state_changes() {
    let both = || active_only(vec![active("a", State::Idle), active("b", State::Idle)]);
    let (app, _) = press(loaded(both()), KeyCode::Char('d'));
    assert_eq!(ids(&app), ["b"]);
    assert_eq!(app.selected_id(), Some("b"));

    let (app, _) = refresh(app, both());
    assert_eq!(ids(&app), ["b"], "same state stays hidden");

    let (app, _) = refresh(
        app,
        active_only(vec![active("a", State::Working), active("b", State::Idle)]),
    );
    assert_eq!(ids(&app), ["a", "b"], "a state change shows it again");
}

#[test]
fn hiding_is_ui_local_and_a_fresh_dashboard_shows_everything() {
    let both = || active_only(vec![active("a", State::Idle), active("b", State::Idle)]);
    let (_hidden, _) = press(loaded(both()), KeyCode::Char('d'));

    assert_eq!(ids(&loaded(both())), ["a", "b"]);
}

#[test]
fn q_escape_and_ctrl_c_quit() {
    for (code, modifiers) in [
        (KeyCode::Char('q'), KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let app = loaded(active_only(vec![active("a", State::Idle)]));
        let (_, effects) = update(app, Msg::Key(code, modifiers));
        assert_eq!(effects, [Effect::Quit], "{code:?} {modifiers:?}");
    }
}

#[test]
fn control_or_alt_letters_do_not_act_as_plain_keys() {
    let app = loaded(active_only(vec![active("a", State::Idle)]));
    let (app, effects) = update(app, Msg::Key(KeyCode::Char('d'), KeyModifiers::CONTROL));
    let (app, more) = update(app, Msg::Key(KeyCode::Char('q'), KeyModifiers::ALT));

    assert!(effects.is_empty() && more.is_empty());
    assert_eq!(ids(&app), ["a"]);
}

#[test]
fn an_empty_store_renders_an_empty_state_message() {
    let app = loaded(overview(vec![
        (SectionName::Active, Vec::new()),
        (SectionName::Finished, Vec::new()),
    ]));

    let text = wide(&app);

    assert!(text.contains("No sessions"), "{text}");
    assert!(!text.contains("Active"), "{text}");
}

#[test]
fn before_the_first_load_the_dashboard_says_it_is_loading() {
    assert!(wide(&App::new()).contains("Loading"));
}

#[test]
fn degraded_entries_render_as_degraded_rows() {
    let mut broken = active("x", State::Unknown);
    broken.agent_kind = String::new();
    broken.repo = None;
    broken.cwd = String::new();
    broken.task_description = None;
    broken.flags = vec![Flag::Degraded];
    broken.error = Some("unknown state \"zzz\"".to_owned());
    let app = loaded(active_only(vec![broken, active("a", State::Idle)]));

    let text = wide(&app);

    assert!(text.contains("DEGRADED"), "{text}");
    assert!(text.contains("unknown state"), "{text}");
    assert!(
        text.contains("IDLE"),
        "the healthy row still renders:\n{text}"
    );
}

fn notified(effects: &[Effect]) -> Vec<(String, State)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify { project, state } => Some((project.clone(), *state)),
            _ => None,
        })
        .collect()
}

#[test]
fn notifications_fire_only_on_a_transition_into_permission_or_idle() {
    let (app, effects) = refresh(
        App::new(),
        active_only(vec![
            active("a", State::Working),
            active("b", State::NeedsPermission),
            active("c", State::Idle),
        ]),
    );
    assert!(notified(&effects).is_empty(), "never on first load");

    let (app, effects) = refresh(
        app,
        active_only(vec![
            active("a", State::Idle),
            active("b", State::NeedsPermission),
            active("c", State::Idle),
        ]),
    );
    assert_eq!(notified(&effects), [("nix-config".to_owned(), State::Idle)]);

    let (app, effects) = refresh(
        app,
        active_only(vec![
            active("a", State::Idle),
            active("b", State::NeedsPermission),
            active("c", State::Idle),
        ]),
    );
    assert!(notified(&effects).is_empty(), "unchanged state is silent");

    let (app, effects) = refresh(
        app,
        active_only(vec![
            active("a", State::NeedsPermission),
            active("b", State::NeedsPermission),
            active("c", State::Idle),
        ]),
    );
    assert_eq!(
        notified(&effects),
        [("nix-config".to_owned(), State::NeedsPermission)]
    );

    let (_, effects) = refresh(
        app,
        active_only(vec![
            active("a", State::Working),
            active("b", State::Working),
            active("c", State::Working),
        ]),
    );
    assert!(
        notified(&effects).is_empty(),
        "going back to work is silent"
    );
}

#[test]
fn a_hidden_session_still_notifies_on_its_next_transition() {
    let (app, _) = press(
        loaded(active_only(vec![active("a", State::Idle)])),
        KeyCode::Char('d'),
    );

    let (_, effects) = refresh(app, active_only(vec![active("a", State::NeedsPermission)]));

    assert_eq!(notified(&effects).len(), 1);
}

#[test]
fn a_40_column_terminal_truncates_rows_without_panicking() {
    let mut e = active("a", State::NeedsPermission);
    e.repo = Some("a-very-long-repository-name".to_owned());
    e.branch = Some("an-equally-long-branch-name".to_owned());
    e.task_description =
        Some("a task description that cannot possibly fit in forty columns".to_owned());
    let app = loaded(overview(vec![
        (SectionName::Active, vec![e]),
        (SectionName::Finished, Vec::new()),
    ]));

    let text = render(&app, 40, 12);

    assert!(text.contains("PERMISSION"), "{text}");
    assert!(text.contains('…'), "{text}");
    assert!(!text.contains("forty columns"), "{text}");
}

#[test]
fn tiny_terminals_do_not_panic() {
    let app = loaded(active_only(vec![active("a", State::Idle)]));
    for (width, height) in [(0, 0), (1, 1), (10, 3), (20, 5), (200, 2)] {
        render(&app, width, height);
    }
    render(&App::new(), 10, 3);
}

#[test]
fn the_selected_row_stays_on_screen_in_a_short_terminal() {
    let entries: Vec<OverviewEntry> = (0..30)
        .map(|n| active(&format!("s{n:02}"), State::Idle))
        .collect();
    let mut app = loaded(active_only(entries));
    for _ in 0..25 {
        app = press(app, KeyCode::Char('j')).0;
    }

    let text = render(&app, 80, 10);

    assert!(text.contains("task of s25"), "{text}");
}

const AGENT_A: &str = "sess-a";
const AGENT_B: &str = "sess-b";

struct FixedGit;

impl GitProbe for FixedGit {
    fn probe(&self, _cwd: &str) -> Option<GitInfo> {
        Some(GitInfo {
            root: "/work/claude-monitor".to_owned(),
            branch: Some("main".to_owned()),
        })
    }
}

fn payload(name: &str, id: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/claude/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    value["session_id"] = Value::String(id.to_owned());
    value.to_string()
}

struct Rig {
    _dir: TempDir,
    state_dir: PathBuf,
    clock: FakeClock,
    store: Store,
    feed: ChangeFeed<FakeSource, ManualClock>,
    wake: FakeHandle,
    feed_clock: ManualClock,
}

impl Rig {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let state_dir = dir.path().join("state");
        let clock = FakeClock::new("2026-10-12T11:55:00Z");
        let store = Store::open_in_dir(&state_dir, &StoreOptions::viewer()).unwrap();
        let (source, wake) = FakeSource::new();
        let feed_clock = ManualClock::new();
        let feed = ChangeFeed::new(
            Store::open_in_dir(&state_dir, &StoreOptions::viewer()).unwrap(),
            source,
            feed_clock.clone(),
        )
        .unwrap();
        Self {
            _dir: dir,
            state_dir,
            clock,
            store,
            feed,
            wake,
            feed_clock,
        }
    }

    fn hook(&self, pane: Option<&str>, raw: &str) {
        ingest(
            &HookContext {
                agent: "claude",
                state_dir: &self.state_dir,
                tmux_pane: pane,
                clock: &self.clock,
                git: &FixedGit,
                agent_pid: None,
                env: None,
            },
            raw,
        );
    }

    fn settle(&mut self, app: App) -> (App, Vec<Effect>) {
        self.wake.wake();
        let (app, _) = pump(app, &mut self.feed, &self.store, NOW);
        self.feed_clock.advance(Duration::from_millis(60));
        pump(app, &mut self.feed, &self.store, NOW)
    }
}

#[test]
fn a_feed_wake_refreshes_the_overview_and_keeps_the_selection_on_the_same_session() {
    let mut rig = Rig::new();
    rig.hook(Some("%1"), &payload("SessionStart-startup", AGENT_A));
    rig.hook(Some("%2"), &payload("SessionStart-startup", AGENT_B));
    let snapshot = Snapshot::load(&rig.store, NOW).unwrap();
    let (mut app, _) = update(App::new(), Msg::Refreshed(snapshot));
    for _ in 0..app.visible().len() {
        let on_b = app
            .visible()
            .iter()
            .any(|e| Some(e.id.as_str()) == app.selected_id() && e.agent_session_id == AGENT_B);
        if on_b {
            break;
        }
        app = press(app, KeyCode::Char('j')).0;
    }
    let selected = app.selected_id().unwrap().to_owned();
    assert!(app
        .visible()
        .iter()
        .any(|e| e.id == selected && e.agent_session_id == AGENT_B));
    assert!(!render_has_permission(&app));

    rig.clock.set("2026-10-12T11:59:00Z");
    rig.hook(Some("%1"), &payload("PermissionRequest", AGENT_A));
    let (app, _) = rig.settle(app);

    let first = app.visible()[0];
    assert_eq!(first.agent_session_id, AGENT_A);
    assert_eq!(first.state, State::NeedsPermission);
    assert_eq!(app.selected_id(), Some(selected.as_str()));
    assert!(render_has_permission(&app));
}

fn render_has_permission(app: &App) -> bool {
    wide(app).contains("PERMISSION")
}

#[test]
fn without_a_wake_the_feed_leaves_the_view_alone() {
    let mut rig = Rig::new();
    rig.hook(None, &payload("SessionStart-startup", AGENT_A));
    let (app, effects) = pump(App::new(), &mut rig.feed, &rig.store, NOW);

    assert!(effects.is_empty());
    assert!(app.snapshot().is_none());
}

#[test]
fn a_scripted_session_goes_from_hook_to_store_to_feed_to_screen() {
    let mut rig = Rig::new();
    rig.hook(Some("%1"), &payload("SessionStart-startup", AGENT_A));
    rig.hook(Some("%2"), &payload("SessionStart-startup", AGENT_B));
    rig.hook(Some("%2"), &payload("UserPromptSubmit", AGENT_B));
    rig.hook(Some("%1"), &payload("PermissionRequest", AGENT_A));

    let (app, effects) = rig.settle(App::new());
    assert!(effects.is_empty(), "the first load never notifies");
    let screen = wide(&app);
    assert!(position(&screen, "PERMISSION") < position(&screen, "WORKING"));
    assert!(screen.contains("Say hi"), "{screen}");
    assert!(screen.contains("claude-monitor/main"), "{screen}");

    let (app, effects) = press(app, KeyCode::Enter);
    assert_eq!(
        effects,
        [Effect::JumpToPane {
            pane: "%1".to_owned()
        }]
    );

    let (app, _) = press(app, KeyCode::Char('j'));
    let (app, _) = press(app, KeyCode::Char('d'));
    assert_eq!(ids(&app).len(), 1);
    assert!(!wide(&app).contains("WORKING"));

    rig.hook(Some("%2"), &payload("Stop", AGENT_B));
    let (app, effects) = rig.settle(app);
    assert_eq!(
        notified(&effects),
        [("claude-monitor".to_owned(), State::Idle)]
    );
    assert_eq!(ids(&app).len(), 2, "the state change showed it again");
    assert!(wide(&app).contains("IDLE"));

    let (_, effects) = press(app, KeyCode::Char('q'));
    assert_eq!(effects, [Effect::Quit]);
}

#[test]
fn no_warnings_means_no_footer_line() {
    assert_eq!(warning_line(&[]), None);
}

#[test]
fn the_footer_names_the_first_warning_and_counts_the_rest() {
    let one = ["claude registry: /x does not exist".to_owned()];
    assert_eq!(
        warning_line(&one).as_deref(),
        Some("warning: claude registry: /x does not exist")
    );
    let three = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
    assert_eq!(
        warning_line(&three).as_deref(),
        Some("warning: a (+2 more)")
    );
}

fn with_unseen() -> App {
    loaded(overview(vec![
        (SectionName::Active, vec![active("a", State::Idle)]),
        (
            SectionName::Unseen,
            vec![
                entry("u1", State::Unknown, SectionName::Unseen),
                entry("u2", State::Unknown, SectionName::Unseen),
            ],
        ),
    ]))
}

#[test]
fn the_unseen_section_is_collapsed_to_a_count_by_default() {
    let app = with_unseen();

    let text = wide(&app);

    assert!(text.contains("Unseen (2)"), "{text}");
    assert!(!text.contains("task of u1"), "{text}");
    assert_eq!(ids(&app), ["a"], "collapsed rows are not selectable");
}

#[test]
fn u_expands_and_collapses_the_unseen_section() {
    let (app, effects) = press(with_unseen(), KeyCode::Char('u'));

    assert!(effects.is_empty());
    assert_eq!(ids(&app), ["a", "u1", "u2"]);
    assert!(wide(&app).contains("task of u1"));

    let (app, _) = press(app, KeyCode::Char('u'));
    assert_eq!(ids(&app), ["a"]);
}

#[test]
fn expanded_unseen_sessions_are_selectable() {
    let (app, _) = press(with_unseen(), KeyCode::Char('u'));
    let (app, _) = press(app, KeyCode::Char('j'));
    let (app, _) = press(app, KeyCode::Char('j'));

    assert_eq!(app.selected_id(), Some("u2"));
}

#[test]
fn unseen_sessions_never_count_toward_the_title() {
    let app = with_unseen();

    let text = wide(&app);

    assert!(
        text.contains("0 need permission · 1 idle · 0 working"),
        "{text}"
    );
}

mod restore_view_model {
    use super::*;
    use claude_monitor::tui::restore::{Group, RestoreView};

    fn mixed() -> Overview {
        overview(vec![
            (SectionName::Active, vec![active("act", State::Working)]),
            (
                SectionName::Interrupted,
                vec![
                    entry("i1", State::Interrupted, SectionName::Interrupted),
                    entry("i2", State::Interrupted, SectionName::Interrupted),
                ],
            ),
            (
                SectionName::Finished,
                vec![entry("f1", State::Finished, SectionName::Finished)],
            ),
            (
                SectionName::Done,
                vec![entry("d1", State::Finished, SectionName::Done)],
            ),
            (
                SectionName::Unseen,
                vec![entry("u1", State::Finished, SectionName::Unseen)],
            ),
            (
                SectionName::Dismissed,
                vec![entry("x1", State::Interrupted, SectionName::Dismissed)],
            ),
        ])
    }

    fn selection(view: &RestoreView) -> Vec<(String, bool)> {
        view.rows()
            .iter()
            .map(|r| (r.entry.id.clone(), r.selected))
            .collect()
    }

    #[test]
    fn interrupted_start_selected_finished_and_unseen_do_not() {
        let view = RestoreView::from_overview(&mixed());

        assert_eq!(
            selection(&view),
            [
                ("i1".to_owned(), true),
                ("i2".to_owned(), true),
                ("f1".to_owned(), false),
                ("u1".to_owned(), false),
            ]
        );
        assert_eq!(view.rows()[2].group, Group::Finished);
        assert_eq!(view.rows()[3].group, Group::Unseen);
        assert_eq!(view.selected_ids(), ["i1", "i2"]);
    }

    #[test]
    fn active_done_and_dismissed_sessions_are_not_offered() {
        let view = RestoreView::from_overview(&mixed());
        let ids: Vec<&str> = view.rows().iter().map(|r| r.entry.id.as_str()).collect();
        for hidden in ["act", "d1", "x1"] {
            assert!(!ids.contains(&hidden), "{hidden} offered: {ids:?}");
        }
    }

    #[test]
    fn toggle_flips_the_row_under_the_cursor_and_keeps_it_interrupted() {
        let mut view = RestoreView::from_overview(&mixed());

        view.toggle();
        assert_eq!(view.selected_ids(), ["i2"]);
        let row = &view.rows()[0];
        assert!(!row.selected);
        assert_eq!(row.entry.state, State::Interrupted);
        assert_eq!(row.entry.section, SectionName::Interrupted);

        view.toggle();
        assert_eq!(view.selected_ids(), ["i1", "i2"]);
    }

    #[test]
    fn a_selects_every_interrupted_session_and_n_selects_none() {
        let mut view = RestoreView::from_overview(&mixed());
        view.select_none();
        assert!(view.selected_ids().is_empty());

        view.move_cursor(true);
        view.move_cursor(true);
        view.toggle();
        assert_eq!(view.selected_ids(), ["f1"]);

        view.select_all_interrupted();
        assert_eq!(view.selected_ids(), ["i1", "i2", "f1"]);
    }

    #[test]
    fn the_cursor_stays_within_the_rows() {
        let mut view = RestoreView::from_overview(&mixed());
        view.move_cursor(false);
        assert_eq!(view.cursor(), 0);
        for _ in 0..10 {
            view.move_cursor(true);
        }
        assert_eq!(view.cursor(), 3);
    }

    #[test]
    fn an_overview_with_nothing_to_restore_has_no_rows() {
        let view = RestoreView::from_overview(&active_only(vec![active("a", State::Working)]));
        assert!(view.rows().is_empty());
        assert!(view.selected_ids().is_empty());
    }
}

/// The restore view end to end: real store, real planner, FakeTmux. Effects
/// are carried out the way the runner does, synchronously.
mod restore_flow {
    use super::*;
    use claude_monitor::agents::kinds;
    use claude_monitor::model::Session;
    use claude_monitor::testing::{FakeEnv, TmuxCall};
    use claude_monitor::tui::restore::{preview_restore, run_restore, RestoreResult, Stage};

    struct World {
        _dir: TempDir,
        fake: FakeEnv,
        store: Store,
        added: std::cell::Cell<u32>,
    }

    impl World {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
            let fake = FakeEnv::new();
            fake.vars.set("HOME", "/home/u");
            fake.vars.set("CLAUDE_MONITOR_LAUNCHER_CLAUDE", "sleep 30");
            Self {
                _dir: dir,
                fake,
                store,
                added: std::cell::Cell::new(0),
            }
        }

        /// A session of `state` in `/w/<id>`; `transcript` false leaves the
        /// resume transcript missing so the planner refuses it.
        fn add(&self, id: &str, state: State, transcript: bool) {
            let cwd = format!("/w/{id}");
            self.fake.fs.add_dir(&cwd);
            let mut session = Session::new(
                id,
                "claude",
                &format!("agent-{id}"),
                &cwd,
                "2026-10-12T10:00:00Z",
            );
            session.state = state;
            // Earlier calls are more recent, so the overview lists them first.
            let n = self.added.get();
            self.added.set(n + 1);
            session.last_activity_at = format!("2026-10-12T10:{:02}:00Z", 59 - n);
            self.store.upsert_session(&session).unwrap();
            if transcript {
                let kind = kinds().into_iter().find(|k| k.id() == "claude").unwrap();
                for need in kind.resume_preconditions(&session, &self.fake.env) {
                    self.fake.fs.add_file(need.required_path, "{}\n");
                }
            }
        }

        fn app(&self) -> App {
            let snapshot = Snapshot::load(&self.store, NOW).unwrap();
            update(App::new(), Msg::Refreshed(snapshot)).0
        }

        fn spawns(&self) -> Vec<(String, String)> {
            self.fake
                .tmux
                .calls()
                .into_iter()
                .filter_map(|call| match call {
                    TmuxCall::SpawnWindow { label, cwd, .. } => {
                        Some((label, cwd.to_string_lossy().into_owned()))
                    }
                    _ => None,
                })
                .collect()
        }

        fn tmux_untouched(&self) -> bool {
            !self.fake.tmux.calls().iter().any(|c| {
                matches!(
                    c,
                    TmuxCall::SpawnWindow { .. } | TmuxCall::EnsureSession { .. }
                )
            })
        }

        /// Feed `msg`, then carry out the effects it causes until none are left.
        fn send(&self, app: App, msg: Msg) -> App {
            let (mut app, mut effects) = update(app, msg);
            while !effects.is_empty() {
                let mut msgs = Vec::new();
                for effect in effects.drain(..) {
                    match effect {
                        Effect::PreviewRestore { ids } => msgs.push(Msg::RestorePreview(
                            preview_restore(&self.store, &self.fake.env, &ids),
                        )),
                        Effect::StartRestore { ids } => {
                            run_restore(&self.store, &self.fake.env, &ids, &mut |r| {
                                msgs.push(Msg::RestoreResult(r));
                            });
                        }
                        other => panic!("unexpected effect {other:?}"),
                    }
                }
                for msg in msgs {
                    let (next, more) = update(app, msg);
                    app = next;
                    effects.extend(more);
                }
            }
            app
        }

        fn keys(&self, mut app: App, keys: &[KeyCode]) -> App {
            for key in keys {
                app = self.send(app, Msg::Key(*key, KeyModifiers::NONE));
            }
            app
        }
    }

    const R: KeyCode = KeyCode::Char('R');
    const SPACE: KeyCode = KeyCode::Char(' ');
    const DOWN: KeyCode = KeyCode::Char('j');

    fn two_interrupted_one_finished() -> World {
        let w = World::new();
        w.add("i1", State::Interrupted, true);
        w.add("i2", State::Interrupted, true);
        w.add("f1", State::Finished, true);
        w
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn r_opens_the_view_with_interrupted_sessions_selected() {
        let w = two_interrupted_one_finished();
        let app = w.keys(w.app(), &[R]);

        let view = app.restore().expect("restore view open");
        assert_eq!(view.selected_ids(), ["i1", "i2"]);
        let text = wide(&app);
        assert!(text.contains("[x]"), "{text}");
        assert!(text.contains("[ ]"), "{text}");
        assert!(text.to_lowercase().contains("finished"), "{text}");
        assert!(w.tmux_untouched());
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn the_help_line_offers_restore() {
        let w = two_interrupted_one_finished();
        let text = wide(&w.app());
        assert!(text.contains("R restore"), "{text}");
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn enter_shows_the_summary_and_starts_nothing() {
        let w = two_interrupted_one_finished();
        let app = w.keys(w.app(), &[R, KeyCode::Enter]);

        let view = app.restore().unwrap();
        assert!(matches!(view.stage(), Stage::Confirm(rows) if rows.len() == 2));
        let text = wide(&app);
        assert!(text.contains("i1") && text.contains("/w/i1"), "{text}");
        assert!(text.contains("i2"), "{text}");
        assert!(!text.contains("f1"), "{text}");
        assert!(w.tmux_untouched(), "started before the second Enter");
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn enter_enter_starts_only_the_selected_sessions() {
        let w = two_interrupted_one_finished();
        // Deselect i2, select the finished f1.
        let app = w.keys(w.app(), &[R, DOWN, SPACE, DOWN, SPACE]);
        assert_eq!(app.restore().unwrap().selected_ids(), ["i1", "f1"]);

        let app = w.keys(app, &[KeyCode::Enter, KeyCode::Enter]);

        let spawned = w.spawns();
        assert_eq!(spawned.len(), 2, "{spawned:?}");
        let cwds: Vec<&str> = spawned.iter().map(|(_, cwd)| cwd.as_str()).collect();
        assert!(
            cwds.contains(&"/w/i1") && cwds.contains(&"/w/f1"),
            "{cwds:?}"
        );
        assert!(!cwds.contains(&"/w/i2"), "{cwds:?}");
        assert!(matches!(
            app.restore().unwrap().stage(),
            Stage::Finished(results) if results.iter().all(|r| matches!(r, RestoreResult::Started { .. }))
        ));
        let text = wide(&app);
        assert!(text.contains("started"), "{text}");
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn deselecting_does_not_dismiss_or_change_the_state() {
        let w = two_interrupted_one_finished();
        let app = w.keys(w.app(), &[R, DOWN, SPACE, KeyCode::Enter, KeyCode::Enter]);
        assert!(app.restore().is_some());

        let skipped = w.store.get_session("i2").unwrap().unwrap();
        assert_eq!(skipped.state, State::Interrupted);
        assert!(!skipped.dismissed);
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn escape_cancels_without_starting_anything() {
        let w = two_interrupted_one_finished();

        let app = w.keys(w.app(), &[R, KeyCode::Esc]);
        assert!(app.restore().is_none());

        let app = w.keys(app, &[R, KeyCode::Enter, KeyCode::Esc]);
        assert!(matches!(app.restore().unwrap().stage(), Stage::Select));
        let app = w.keys(app, &[KeyCode::Esc]);
        assert!(app.restore().is_none());
        assert!(w.tmux_untouched());
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn keys_in_the_view_do_not_quit_the_dashboard() {
        let w = two_interrupted_one_finished();
        let app = w.app();
        let (app, _) = update(app, Msg::Key(R, KeyModifiers::NONE));
        let (_, effects) = update(app, Msg::Key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!effects.contains(&Effect::Quit), "{effects:?}");
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn enter_with_nothing_selected_does_nothing() {
        let w = two_interrupted_one_finished();
        let app = w.keys(w.app(), &[R, KeyCode::Char('n')]);
        let (app, effects) = update(app, Msg::Key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(effects.is_empty(), "{effects:?}");
        assert!(matches!(app.restore().unwrap().stage(), Stage::Select));
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn the_summary_shows_refusals_with_their_alternatives() {
        let w = World::new();
        w.add("ok1", State::Interrupted, true);
        w.add("bad1", State::Interrupted, false);
        let app = w.keys(w.app(), &[R, KeyCode::Enter]);

        let text = wide(&app);
        assert!(text.contains("missing_transcript"), "{text}");
        assert!(text.contains("respawn bad1 --fresh"), "{text}");
        assert!(w.tmux_untouched());

        let app = w.keys(app, &[KeyCode::Enter]);
        assert_eq!(w.spawns().len(), 1);
        assert!(matches!(app.restore().unwrap().stage(), Stage::Finished(_)));
    }

    #[test]
    #[ignore = "T074 restore flow"]
    fn each_result_shows_as_it_arrives() {
        let w = two_interrupted_one_finished();
        let app = w.keys(w.app(), &[R, KeyCode::Enter]);
        let (app, effects) = update(app, Msg::Key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(effects.len(), 1);

        let (app, _) = update(
            app,
            Msg::RestoreResult(RestoreResult::Started {
                session_id: "i1".to_owned(),
                pane: "%7".to_owned(),
            }),
        );

        assert!(matches!(
            app.restore().unwrap().stage(),
            Stage::Running { total: 2, results } if results.len() == 1
        ));
        let text = wide(&app);
        assert!(text.contains("%7"), "{text}");
    }
}
