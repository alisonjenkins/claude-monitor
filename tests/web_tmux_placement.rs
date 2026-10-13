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

#[test]
fn the_session_page_and_json_carry_the_session_name() {
    let web = Plain::new();
    web.put("s-named", State::Working);
    web.put("s-plain", State::Working);
    let mut s = web.session("s-named");
    s.session_name = Some("aq <b>uero".to_owned());
    web.save(&s);

    let html = page(&web.rig, "/sessions/s-named").body;
    let json = json_page(&web.rig, "/sessions/s-named").json();
    let plain = page(&web.rig, "/sessions/s-plain").body;

    assert!(html.contains("<dt>Name</dt>"), "{html}");
    assert!(html.contains("aq &lt;b&gt;uero"), "{html}");
    assert_eq!(json["session_name"], "aq <b>uero");
    assert!(!plain.contains("<dt>Name</dt>"), "{plain}");
}

fn live_panel(html: &str) -> &str {
    let start = html
        .find("id=\"live-restore\"")
        .unwrap_or_else(|| panic!("no live panel in page:\n{html}"));
    let rest = &html[start..];
    &rest[..rest.find("</section>").unwrap_or(rest.len())]
}

#[test]
fn the_restore_page_lists_live_sessions_with_the_window_they_would_get() {
    let web = Plain::new();
    web.put("s-live", State::Working);
    web.put("s-bare", State::Working);
    web.put("s-int", State::Interrupted);
    place(&web, "s-live", "luks-unlock", "claude", "2", "1");
    place(&web, "s-int", "other", "gone", "1", "1");

    let html = page(&web.rig, "/restore").body;

    let panel = live_panel(&html);
    assert!(panel.contains("Live sessions"), "{panel}");
    assert!(panel.contains("href=\"/sessions/s-live\""), "{panel}");
    assert!(panel.contains("luks-unlock:claude"), "{panel}");
    assert!(panel.contains("window 2"), "{panel}");
    assert!(panel.contains("pane 1"), "{panel}");
    assert!(
        !panel.contains("s-bare"),
        "no placement, not listed: {panel}"
    );
    assert!(!panel.contains("s-int"), "not live: {panel}");
    assert!(!panel.contains("collide"), "{panel}");
}

#[test]
fn the_live_panel_offers_no_way_to_start_anything() {
    let web = Plain::new();
    web.put("s-live", State::Working);
    place(&web, "s-live", "luks-unlock", "claude", "2", "1");

    let html = page(&web.rig, "/restore").body;

    let panel = live_panel(&html);
    assert!(!panel.contains("checkbox"), "{panel}");
    assert!(!panel.contains("<button"), "{panel}");
    assert!(!panel.contains("<form"), "{panel}");
    assert!(!panel.contains("hx-post"), "{panel}");
    let json = json_page(&web.rig, "/restore").json();
    assert_eq!(json["would_start"].as_array().unwrap().len(), 0, "{json}");
}

#[test]
fn two_live_sessions_in_one_slot_are_flagged() {
    let web = Plain::new();
    web.put("s-one", State::Working);
    web.put("s-two", State::Idle);
    web.put("s-three", State::Working);
    place(&web, "s-one", "luks-unlock", "claude", "2", "1");
    place(&web, "s-two", "luks-unlock", "other-name", "2", "1");
    place(&web, "s-three", "luks-unlock", "claude", "2", "2");

    let html = page(&web.rig, "/restore").body;

    let panel = live_panel(&html);
    assert_eq!(panel.matches("class=\"warn\"").count(), 2, "{panel}");
    assert!(panel.contains("Same tmux slot as s-two"), "{panel}");
    assert!(panel.contains("Same tmux slot as s-one"), "{panel}");
}

#[test]
fn the_live_target_follows_the_explicit_override_like_the_planner() {
    let web = Plain::new();
    web.put("s-live", State::Working);
    place(&web, "s-live", "luks-unlock", "claude", "2", "1");
    web.fake.vars.set("CLAUDE_MONITOR_TMUX_SESSION", "forced");

    let html = page(&web.rig, "/restore").body;

    let panel = live_panel(&html);
    assert!(panel.contains("forced:claude"), "{panel}");
    assert!(!panel.contains("luks-unlock:claude"), "{panel}");
}

#[test]
fn a_hostile_window_name_is_escaped_in_the_live_panel() {
    let web = Plain::new();
    web.put("s-evil", State::Working);
    place(&web, "s-evil", "s", "<script>alert(1)</script>", "0", "0");

    let html = page(&web.rig, "/restore").body;

    assert!(!html.contains("<script>alert(1)"), "{html}");
    assert!(
        live_panel(&html).contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
}

#[test]
fn a_row_carries_a_short_placement_hint_only_when_one_is_recorded() {
    let web = Plain::new();
    web.put("s-placed", State::Working);
    web.put("s-bare", State::Working);
    place(&web, "s-placed", "luks-unlock", "w", "1", "1");

    let html = page(&web.rig, "/").body;

    let placed = html
        .split("<li class=\"row\"")
        .find(|row| row.contains("s-placed"))
        .unwrap();
    let bare = html
        .split("<li class=\"row\"")
        .find(|row| row.contains("s-bare"))
        .unwrap();
    assert!(placed.contains("tmux luks-unlock:1"), "{placed}");
    assert!(!bare.contains("tmux "), "{bare}");
}

#[test]
fn a_hostile_session_name_is_escaped_in_the_row_hint() {
    let web = Plain::new();
    web.put("s-evil", State::Working);
    place(&web, "s-evil", "<script>x</script>", "w", "1", "1");

    let html = page(&web.rig, "/").body;

    assert!(!html.contains("<script>x"), "{html}");
    assert!(
        html.contains("tmux &lt;script&gt;x&lt;/script&gt;:1"),
        "{html}"
    );
}

#[test]
fn the_list_json_keeps_its_exact_shape() {
    let web = Plain::new();
    web.put("s-placed", State::Working);
    place(&web, "s-placed", "luks-unlock", "w", "1", "1");

    let json = json_page(&web.rig, "/sessions").json();

    assert!(json["sessions"][0].get("tmux_hint").is_none(), "{json}");
}
