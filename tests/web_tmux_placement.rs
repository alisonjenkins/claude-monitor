//! Where a session sits in tmux, as the web view shows it: the stored
//! placement on the session page, so it can be read before a reboot.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;
mod web_support;

use claude_monitor::model::State;
use web_support::{Plain, Request, Response, Rig};

fn page(rig: &Rig, path: &str) -> Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .send(rig.addr())
}

fn json_page(rig: &Rig, path: &str) -> Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .header("Accept", "application/json")
        .send(rig.addr())
}

fn place(web: &Plain, id: &str, session: &str, window: &str, index: &str, pane: &str) {
    let mut s = web.session(id);
    s.tmux_session = Some(session.to_owned());
    s.tmux_window_name = Some(window.to_owned());
    s.tmux_window_index = Some(index.to_owned());
    s.tmux_pane_index = Some(pane.to_owned());
    s.tmux_window_panes = Some("1".to_owned());
    web.save(&s);
}

#[test]
fn the_session_page_says_where_it_would_be_restored_in_the_words_of_show() {
    let web = Plain::new();
    web.put("s-placed", State::Working);
    place(&web, "s-placed", "luks-unlock", ".claude-wrapped", "1", "1");

    let html = page(&web.rig, "/sessions/s-placed").body;

    assert!(html.contains("<dt>tmux</dt>"), "{html}");
    assert!(
        html.contains("luks-unlock:.claude-wrapped (window 1, pane 1 of 1)"),
        "{html}"
    );
}

#[test]
fn a_session_with_no_recorded_window_says_so() {
    let web = Plain::new();
    web.put("s-bare", State::Working);

    let html = page(&web.rig, "/sessions/s-bare").body;

    assert!(html.contains("no recorded tmux window"), "{html}");
}

#[test]
fn a_hostile_window_name_is_escaped_on_the_session_page() {
    let web = Plain::new();
    web.put("s-evil", State::Working);
    place(&web, "s-evil", "s", "<script>alert(1)</script>", "0", "0");

    let html = page(&web.rig, "/sessions/s-evil").body;

    assert!(!html.contains("<script>alert(1)"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
}

#[test]
fn the_session_json_carries_the_placement_fields() {
    let web = Plain::new();
    web.put("s-placed", State::Working);
    place(&web, "s-placed", "luks-unlock", "w", "1", "2");

    let json = json_page(&web.rig, "/sessions/s-placed").json();

    assert_eq!(json["tmux_session"], "luks-unlock");
    assert_eq!(json["tmux_window_name"], "w");
    assert_eq!(json["tmux_window_index"], "1");
    assert_eq!(json["tmux_pane_index"], "2");
    assert_eq!(json["tmux_window_panes"], "1");
}
