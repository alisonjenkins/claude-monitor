//! The HTML side of the web view (FR-030, T116 design): what the overview,
//! project, session and restore pages say, over real loopback sockets.
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

use claude_monitor::cli::Command;
use claude_monitor::model::State;
use common::Fixture;
use serde_json::json;
use web_support::{Plain, Request, Response, Rig};

fn page(rig: &Rig, path: &str) -> Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .send(rig.addr())
}

fn post(rig: &Rig, path: &str, body: &serde_json::Value) -> Response {
    Request::new("POST", path)
        .signed_in(rig)
        .json_body(body)
        .send(rig.addr())
}

/// The markup of the element that starts at `marker` and ends at the first
/// `closing` after it (sections are not nested, so this is the whole section).
fn between<'a>(html: &'a str, marker: &str, closing: &str) -> &'a str {
    let start = html
        .find(marker)
        .unwrap_or_else(|| panic!("{marker} not in page:\n{html}"));
    let rest = &html[start..];
    let end = rest.find(closing).unwrap_or(rest.len());
    &rest[..end]
}

fn section<'a>(html: &'a str, name: &str) -> &'a str {
    between(html, &format!("section-{name}\""), "</section>")
}

fn has_section(html: &str, name: &str) -> bool {
    html.contains(&format!("section-{name}\""))
}

#[test]
fn the_overview_is_html_with_no_external_host_and_its_own_assets() {
    let web = Plain::new();
    web.put("s-work", State::Working);

    let response = page(&web.rig, "/");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let body = &response.body;
    assert!(body.starts_with("<!DOCTYPE html>"), "{body}");
    assert!(body.contains("<html lang=\"en\">"));
    assert!(body.contains("name=\"viewport\""));
    assert!(body.contains("src=\"/assets/htmx.min.js\""));
    for needle in ["http://", "https://", "//cdn", "fonts.googleapis"] {
        assert!(!body.contains(needle), "external reference {needle}");
    }
}

#[test]
fn the_assets_are_served_from_the_server_itself() {
    let web = Plain::new();

    let htmx = page(&web.rig, "/assets/htmx.min.js");
    let css = page(&web.rig, "/assets/app.css");
    let js = page(&web.rig, "/assets/app.js");

    assert_eq!(htmx.status, 200);
    assert!(htmx
        .header("content-type")
        .unwrap()
        .starts_with("text/javascript"));
    assert!(htmx.body.contains("htmx"));
    assert_eq!(css.status, 200);
    assert!(css.header("content-type").unwrap().starts_with("text/css"));
    assert!(css.body.contains("prefers-color-scheme: dark"));
    assert!(css.body.contains("prefers-reduced-motion"));
    assert!(css.body.contains("focus-visible"));
    assert_eq!(js.status, 200);
    assert!(js.body.contains("EventSource(\"/events\")"));
    for ours in [&css, &js] {
        assert!(!ours.body.contains("https://"));
        assert!(!ours.body.contains("@import"));
    }
}

#[test]
fn an_asset_needs_the_token_like_every_other_route() {
    let web = Plain::new();

    let response = Request::get("/assets/app.css").send(web.rig.addr());

    assert_eq!(response.status, 401);
}

#[test]
fn sessions_waiting_on_the_user_are_lifted_into_their_own_band() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-perm", State::NeedsPermission);
    web.put("s-int", State::Interrupted);
    web.put("s-fin", State::Finished);

    let html = page(&web.rig, "/").body;

    let waiting = between(&html, "id=\"waiting\"", "</section>");
    assert!(waiting.contains("s-perm"));
    assert!(waiting.contains("s-int"));
    assert!(!waiting.contains("s-work"));
    assert!(!waiting.contains("s-fin"));
    assert!(waiting.contains("data-state=\"needs_permission\""));
    assert!(waiting.contains("data-state=\"interrupted\""));
    assert!(html.contains("<title>(2) claude-monitor</title>"), "{html}");
    assert!(
        html.find("id=\"waiting\"").unwrap() < html.find("section-active\"").unwrap(),
        "the band comes before every section"
    );
}

#[test]
fn with_nothing_waiting_there_is_no_band_and_no_count() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-fin", State::Finished);

    let html = page(&web.rig, "/").body;

    assert!(!html.contains("id=\"waiting\""));
    assert!(html.contains("<title>claude-monitor</title>"));
}

#[test]
fn the_calm_sections_hold_their_sessions_and_finished_is_never_collapsed() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-fin", State::Finished);
    web.put("s-unseen", State::Unknown);

    let html = page(&web.rig, "/").body;

    assert!(section(&html, "active").contains("s-work"));
    let finished = section(&html, "finished");
    assert!(finished.contains("s-fin"));
    assert!(!html.contains("<details id=\"section-finished\""));
    let unseen_tag = between(&html, "<details id=\"section-unseen\"", ">");
    assert!(!unseen_tag.contains("open"), "unseen starts collapsed");
    assert!(
        section(&html, "unseen").contains("(1)"),
        "its count is visible"
    );
}

#[test]
fn the_page_moves_a_session_to_the_done_section_after_it_is_marked_done() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    assert!(section(&page(&web.rig, "/").body, "finished").contains("s-fin"));

    post(&web.rig, "/sessions/s-fin/done", &json!({}));

    let html = page(&web.rig, "/").body;
    assert!(section(&html, "done").contains("s-fin"));
    assert!(!has_section(&html, "finished") || !section(&html, "finished").contains("s-fin"));
    assert!(section(&html, "done").contains("hx-post=\"/sessions/s-fin/undone\""));
}

#[test]
fn dismissed_sessions_show_only_when_asked() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    post(
        &web.rig,
        "/sessions/s-fin/dismiss",
        &json!({"confirm": "1"}),
    );

    let plain = page(&web.rig, "/").body;
    let asked = page(&web.rig, "/?include_dismissed=true").body;

    assert!(!plain.contains("s-fin"));
    assert!(section(&asked, "dismissed").contains("s-fin"));
}

#[test]
fn rows_show_the_verbatim_task_escaped_and_link_to_the_detail_page() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    let mut session = web.session("s-fin");
    session.task_description = Some("fix <script>alert(1)</script> & \"quote\"".to_owned());
    web.save(&session);

    let html = page(&web.rig, "/").body;

    assert!(html.contains("fix &lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quote&quot;"));
    assert!(!html.contains("<script>alert(1)"));
    assert!(html.contains("href=\"/sessions/s-fin\""));
    assert!(html.contains("left off in s-fin"));
}

#[test]
fn the_filter_form_is_a_plain_get_and_filters_apply() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-fin", State::Finished);

    let html = page(&web.rig, "/sessions?state=finished").body;

    assert!(html.contains("<form method=\"get\" action=\"/sessions\""));
    assert!(html.contains("name=\"text\""));
    assert!(html.contains("name=\"state\""));
    assert!(html.contains("s-fin"));
    assert!(!html.contains("s-work"));
}

#[test]
fn the_page_is_wired_for_live_updates_and_the_csrf_header() {
    let web = Plain::new();
    web.put("s-work", State::Working);

    let html = page(&web.rig, "/").body;

    assert!(html.contains("hx-trigger=\"cc-changed from:body"));
    assert!(html.contains("hx-select=\"#sessions\""));
    assert!(html.contains(&format!(
        "<meta name=\"cc-csrf\" content=\"{}\">",
        web.rig.csrf()
    )));
    assert!(html.contains("href=\"#sessions\""), "skip link");
    assert!(html.contains("role=\"status\""), "flash line");
    assert!(html.contains("<main"));
}

#[test]
fn the_session_page_has_the_verbatim_text_notes_lineage_and_the_right_actions() {
    let web = Plain::new();
    web.put("s-old", State::Finished);
    web.put("s-new", State::Interrupted);
    let mut new = web.session("s-new");
    new.resumed_from = Some("s-old".to_owned());
    web.save(&new);
    post(
        &web.rig,
        "/sessions/s-new/note",
        &json!({"text": "check the <b>CI</b>"}),
    );

    let html = page(&web.rig, "/sessions/s-new").body;

    assert!(html.contains("task of s-new"));
    assert!(html.contains("left off in s-new"));
    assert!(
        html.contains("href=\"/sessions/s-old\""),
        "resumed from link"
    );
    assert!(html.contains("check the &lt;b&gt;CI&lt;/b&gt;"));
    assert!(html.contains("hx-post=\"/sessions/s-new/park\""));
    assert!(
        !html.contains("/sessions/s-new/done"),
        "not finished, so no done"
    );
    assert!(!html.to_lowercase().contains("transcript"));
    let old = page(&web.rig, "/sessions/s-old").body;
    assert!(old.contains("hx-post=\"/sessions/s-old/done\""));
}

#[test]
fn an_unknown_session_page_is_a_404_page() {
    let web = Plain::new();

    let response = page(&web.rig, "/sessions/nope");

    assert_eq!(response.status, 404);
    assert!(response
        .header("content-type")
        .unwrap()
        .starts_with("text/html"));
    assert!(response.body.contains("no session matches"));
}

#[test]
fn progress_shows_its_figure_with_the_named_source() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    let plan = web.dir.path().join("plan.md");
    web.fake.fs.add_file(&plan, "- [x] first\n- [ ] second\n");
    web.cli(Command::ProgressBind {
        owner: "s-fin".to_owned(),
        file: Some(plan.clone()),
        source: None,
    });

    let overview = page(&web.rig, "/").body;
    let detail = page(&web.rig, "/sessions/s-fin").body;

    assert!(overview.contains("1 of 2"), "{overview}");
    assert!(detail.contains("1 of 2"));
    assert!(detail.contains("Markdown checklist"));
    assert!(detail.contains("plan.md"));
}

#[test]
fn the_project_page_has_its_sections_and_only_its_own_done_sessions() {
    let web = Plain::new();
    web.put("in-a", State::Finished);
    web.put("in-b", State::Finished);
    web.put("a-live", State::Working);
    let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    post(&web.rig, "/projects", &json!({"name": "beta"}));
    for (session, project) in [("in-a", "alpha"), ("a-live", "alpha"), ("in-b", "beta")] {
        post(
            &web.rig,
            &format!("/sessions/{session}/group"),
            &json!({"project": project}),
        );
    }
    post(&web.rig, "/sessions/in-a/done", &json!({}));
    post(&web.rig, "/sessions/in-b/done", &json!({}));

    let html = page(&web.rig, &format!("/projects/{id}")).body;

    assert!(html.contains("<h1>alpha</h1>"));
    assert!(section(&html, "active").contains("a-live"));
    assert!(section(&html, "done").contains("in-a"));
    assert!(!html.contains("in-b"));
}

#[test]
fn the_projects_page_is_the_overview_now() {
    let web = Plain::new();
    post(&web.rig, "/projects", &json!({"name": "alpha"}));

    let response = page(&web.rig, "/projects");

    assert_eq!(response.status, 302);
    assert_eq!(response.header("location"), Some("/"));
    let home = page(&web.rig, "/").body;
    assert!(
        !home.contains("href=\"/projects\""),
        "no Projects entry in the nav"
    );
    assert!(home.contains("alpha"));
}

fn make_project(web: &Plain, name: &str) -> String {
    post(&web.rig, "/projects", &json!({"name": name})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn group(web: &Plain, session: &str, project: &str) {
    post(
        &web.rig,
        &format!("/sessions/{session}/group"),
        &json!({"project": project}),
    );
}

/// One project's node: from its opening tag to the next node's.
fn node<'a>(html: &'a str, id: &str) -> &'a str {
    let start = html
        .find(&format!("id=\"n-{id}\""))
        .unwrap_or_else(|| panic!("no node n-{id} in page:\n{html}"));
    let rest = &html[start..];
    let end = [
        "<details class=\"node",
        "<details id=\"section-unseen\"",
        "<section id=\"section-dismissed\"",
    ]
    .iter()
    .filter_map(|marker| rest[1..].find(marker).map(|at| at + 1))
    .min()
    .unwrap_or(rest.len());
    &rest[..end]
}

fn node_tag<'a>(html: &'a str, id: &str) -> &'a str {
    let start = html.find(&format!("id=\"n-{id}\"")).unwrap();
    let from = html[..start].rfind("<details").unwrap();
    let rest = &html[from..];
    &rest[..rest.find('>').unwrap()]
}

#[test]
fn sessions_are_nested_under_their_projects_in_alphabetical_order() {
    let web = Plain::new();
    web.put("in-b", State::Working);
    web.put("in-a", State::Finished);
    web.put("loose", State::Working);
    let beta = make_project(&web, "beta");
    let alpha = make_project(&web, "alpha");
    group(&web, "in-b", "beta");
    group(&web, "in-a", "alpha");

    let html = page(&web.rig, "/").body;

    let (a, b, none) = (
        html.find(&format!("id=\"n-{alpha}\"")).unwrap(),
        html.find(&format!("id=\"n-{beta}\"")).unwrap(),
        html.find("id=\"n-none\"").unwrap(),
    );
    assert!(a < b && b < none, "alphabetical, then No project");
    assert!(node(&html, &alpha).contains("in-a"));
    assert!(!node(&html, &alpha).contains("in-b"));
    assert!(section(node(&html, &beta), "active").contains("in-b"));
    assert!(section(node(&html, "none"), "active").contains("loose"));
    assert!(
        node(&html, &alpha).contains("<h2"),
        "the project name is a heading"
    );
    assert!(node(&html, &alpha).contains("alpha"));
}

#[test]
fn a_node_is_open_by_default_and_says_its_state_summary() {
    let web = Plain::new();
    web.put("w1", State::Working);
    web.put("w2", State::Working);
    web.put("i1", State::Interrupted);
    web.put("f1", State::Finished);
    let id = make_project(&web, "alpha");
    for s in ["w1", "w2", "i1", "f1"] {
        group(&web, s, "alpha");
    }

    let html = page(&web.rig, "/").body;

    let tag = node_tag(&html, &id);
    assert!(tag.contains(" open"), "{tag}");
    assert!(tag.contains("data-node"), "{tag}");
    assert!(node(&html, &id).contains("2 working · 1 interrupted · 1 finished"));
    assert!(node(&html, &id).contains("aria-expanded=\"true\""));
}

#[test]
fn a_project_with_no_sessions_stays_visible_and_muted() {
    let web = Plain::new();
    web.put("loose", State::Working);
    let id = make_project(&web, "empty-one");

    let html = page(&web.rig, "/").body;

    assert!(
        node_tag(&html, &id).contains("class=\"node empty\""),
        "muted class"
    );
    assert!(node(&html, &id).contains("no sessions"));
}

#[test]
fn the_no_project_node_is_left_out_when_nothing_is_loose() {
    let web = Plain::new();
    web.put("s1", State::Working);
    make_project(&web, "alpha");
    group(&web, "s1", "alpha");

    let html = page(&web.rig, "/").body;

    assert!(!html.contains("id=\"n-none\""));
}

#[test]
fn waiting_sessions_repeat_in_the_band_with_their_project_and_stay_in_their_node() {
    let web = Plain::new();
    web.put("s-perm", State::NeedsPermission);
    web.put("s-int", State::Interrupted);
    let id = make_project(&web, "alpha");
    group(&web, "s-perm", "alpha");
    group(&web, "s-int", "alpha");

    let html = page(&web.rig, "/").body;

    let band = between(&html, "id=\"waiting\"", "</section>");
    assert!(band.contains("s-perm") && band.contains("s-int"));
    assert!(band.contains("alpha"), "the band names the project: {band}");
    let tree = node(&html, &id);
    assert!(tree.contains("s-perm") && tree.contains("s-int"));
    assert!(section(tree, "interrupted").contains("s-int"));
    assert!(section(tree, "active").contains("s-perm"));
}

#[test]
fn finished_is_never_collapsed_and_done_is_a_nested_collapsed_details() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    web.put("s-done", State::Finished);
    let id = make_project(&web, "alpha");
    group(&web, "s-fin", "alpha");
    group(&web, "s-done", "alpha");
    post(&web.rig, "/sessions/s-done/done", &json!({}));

    let html = page(&web.rig, "/").body;

    let tree = node(&html, &id);
    let finished = section(tree, "finished");
    assert!(finished.contains("s-fin") && !finished.contains("<details"));
    let done = section(tree, "done");
    assert!(done.contains("s-done"));
    let tag = between(done, "<details", ">");
    assert!(!tag.contains("open"), "done starts collapsed: {tag}");
}

#[test]
fn unseen_sessions_stay_in_one_collapsed_node_after_the_projects() {
    let web = Plain::new();
    web.put("s-unseen", State::Unknown);
    web.put("s-work", State::Working);
    make_project(&web, "alpha");

    let html = page(&web.rig, "/").body;

    let tag = between(&html, "<details id=\"section-unseen\"", ">");
    assert!(!tag.contains("open"));
    assert!(section(&html, "unseen").contains("s-unseen"));
    assert!(html.find("id=\"n-none\"").unwrap() < html.find("section-unseen").unwrap());
    assert!(!node(&html, "none").contains("s-unseen"));
}

#[test]
fn filters_apply_to_the_tree() {
    let web = Plain::new();
    web.put("in-work", State::Working);
    web.put("in-fin", State::Finished);
    let id = make_project(&web, "alpha");
    group(&web, "in-work", "alpha");
    group(&web, "in-fin", "alpha");

    let html = page(&web.rig, "/?state=finished").body;

    assert!(node(&html, &id).contains("in-fin"));
    assert!(!html.contains("in-work"));
    assert!(node(&html, &id).contains("1 finished"));
}

#[test]
fn each_row_has_a_move_panel_listing_the_existing_projects() {
    let web = Plain::new();
    web.put("s-one", State::Working);
    web.put("s-two", State::Finished);
    make_project(&web, "alpha");
    make_project(&web, "b&c");
    group(&web, "s-one", "alpha");

    let html = page(&web.rig, "/").body;

    let row = between(&html, "id=\"s-s-one\"", "</li>");
    assert!(row.contains("data-move-toggle"));
    assert!(row.contains("aria-expanded=\"false\""));
    let panel = between(row, "<form", "</form>");
    assert!(
        panel.contains("hx-post=\"/sessions/s-one/group\""),
        "{panel}"
    );
    assert!(panel.contains("hidden"));
    assert!(panel.contains("<label"), "labelled controls");
    assert!(panel.contains("name=\"project\""));
    assert!(
        panel.contains("<option value=\"b&amp;c\">"),
        "escaped: {panel}"
    );
    assert!(panel.contains("<option value=\"-\""), "ungroup option");
    assert!(panel.contains("New project"), "new project option");
    assert!(panel.contains("data-move-new"), "name input");
    let current = between(panel, "<option value=\"alpha\"", ">");
    assert!(
        current.contains("selected") && current.contains("disabled"),
        "{current}"
    );
    let loose = between(&html, "id=\"s-s-two\"", "</li>");
    let none_option = between(loose, "<option value=\"-\"", ">");
    assert!(
        none_option.contains("disabled"),
        "no project is the current one"
    );
}

#[test]
fn the_project_page_is_the_same_node_for_one_project() {
    let web = Plain::new();
    web.put("in-a", State::Working);
    web.put("in-b", State::Working);
    let a = make_project(&web, "alpha");
    make_project(&web, "beta");
    group(&web, "in-a", "alpha");
    group(&web, "in-b", "beta");

    let html = page(&web.rig, &format!("/projects/{a}")).body;

    assert!(html.contains("<h1>alpha</h1>"));
    assert!(html.contains("in-a") && !html.contains("in-b"));
    assert!(html.contains("data-move-toggle"), "rows can still be moved");
    assert!(html.contains("hx-select=\"#sessions\""));
}

#[test]
fn a_project_node_offers_rename_status_note_and_park_in_a_menu() {
    let web = Plain::new();
    web.put("s1", State::Working);
    let id = make_project(&web, "alpha");
    group(&web, "s1", "alpha");

    let html = page(&web.rig, "/").body;

    let tree = node(&html, &id);
    assert!(tree.contains("class=\"menu\""));
    assert!(tree.contains(&format!("hx-post=\"/projects/{id}\"")));
    assert!(tree.contains(&format!("hx-post=\"/projects/{id}/note\"")));
    assert!(tree.contains(&format!("hx-post=\"/projects/{id}/park\"")));
}

#[test]
fn the_links_page_lists_stored_links() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    post(
        &web.rig,
        "/sessions/s-fin/link",
        &json!({"cortex": "42", "kind": "task"}),
    );

    let html = page(&web.rig, "/links").body;

    assert!(html.contains("s-fin"));
    assert!(html.contains("42"));
}

#[test]
fn the_restore_page_previews_with_every_interrupted_session_selected() {
    let Some(fx) = Fixture::new() else { return };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    fx.add_session("fin1", State::Finished, "c");
    let gone = fx.add_session("gone", State::Interrupted, "gone-dir");
    std::fs::remove_dir_all(&gone.cwd).unwrap();
    let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

    let html = page(&rig, "/restore").body;

    for id in ["int1", "int2"] {
        assert!(
            html.contains(&format!("name=\"ids\" value=\"{id}\" checked")),
            "{id} is a checked box"
        );
    }
    assert!(
        !html.contains("value=\"fin1\""),
        "finished is not pre-selected"
    );
    assert!(html.contains("missing_directory"), "the refusal is listed");
    assert!(html.contains("hx-post=\"/respawn\""));
    assert!(html.contains("Restore"));
    assert!(!fx.server_has_sessions(), "previewing starts nothing");
}

#[test]
fn the_restore_page_says_so_when_nothing_is_interrupted() {
    let web = Plain::new();
    web.put("s-work", State::Working);

    let html = page(&web.rig, "/restore").body;

    assert!(html.contains("Nothing to restore"));
    assert!(!html.contains("hx-post=\"/respawn\""));
}

mod respawn_through_the_restore_preview {
    use super::*;

    fn json_page(rig: &Rig, path: &str) -> Response {
        Request::get(path)
            .header("Cookie", rig.cookie())
            .header("Accept", "application/json")
            .send(rig.addr())
    }

    #[test]
    fn the_overview_button_links_to_the_preview_instead_of_starting() {
        let web = Plain::new();
        web.put("s-int", State::Interrupted);

        let html = page(&web.rig, "/").body;

        assert!(html.contains("href=\"/restore?session=s-int\""), "{html}");
        assert!(
            !html.contains("hx-post=\"/sessions/s-int/respawn\""),
            "{html}"
        );
    }

    #[test]
    fn the_session_page_button_links_to_the_preview_too() {
        let web = Plain::new();
        web.put("s-int", State::Interrupted);
        web.put("s-fin", State::Finished);

        for id in ["s-int", "s-fin"] {
            let html = page(&web.rig, &format!("/sessions/{id}")).body;

            assert!(
                html.contains(&format!("href=\"/restore?session={id}\"")),
                "{html}"
            );
            assert!(
                !html.contains(&format!("hx-post=\"/sessions/{id}/respawn\"")),
                "{html}"
            );
        }
    }

    #[test]
    fn the_preview_is_for_that_one_session_and_starts_nothing() {
        let Some(fx) = Fixture::new() else { return };
        fx.add_session("int1", State::Interrupted, "a");
        fx.add_session("int2", State::Interrupted, "b");
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=int1").body;

        assert!(
            html.contains("name=\"ids\" value=\"int1\" checked"),
            "{html}"
        );
        assert!(!html.contains("value=\"int2\""), "{html}");
        assert!(html.contains("Respawn"), "{html}");
        assert!(html.contains("hx-post=\"/respawn\""), "{html}");
        assert!(
            html.contains("href=\"/sessions/int1\""),
            "cancel link: {html}"
        );
        assert!(!html.contains("Include finished"), "{html}");
        assert!(!fx.server_has_sessions(), "previewing starts nothing");
    }

    #[test]
    fn the_preview_shows_the_original_tmux_window_as_session_and_name() {
        let Some(fx) = Fixture::new() else { return };
        let mut s = fx.add_session("int1", State::Interrupted, "a");
        s.tmux_session = Some("notes".to_owned());
        s.tmux_window_name = Some("docs".to_owned());
        fx.save(&s);
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=int1").body;
        let body = json_page(&rig, "/restore?session=int1").json();

        assert!(html.contains("notes:docs"), "{html}");
        assert_eq!(body["would_start"][0]["tmux_target"], "notes:docs");
    }

    #[test]
    fn the_preview_shows_the_command_that_will_run() {
        let Some(fx) = Fixture::new() else { return };
        fx.add_session("int1", State::Interrupted, "a");
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=int1").body;
        let body = json_page(&rig, "/restore?session=int1").json();

        let command = body["would_start"][0]["command"].as_str().unwrap();
        assert!(command.starts_with("cd "), "{command}");
        let escaped = command.replace('&', "&amp;");
        assert!(html.contains(&format!("run: {escaped}")), "{html}");
    }

    #[test]
    fn the_preview_shows_the_name_flag_of_a_named_session() {
        let Some(fx) = Fixture::new() else { return };
        let mut session = fx.add_session("int1", State::Interrupted, "a");
        session.session_name = Some("aquero".to_owned());
        fx.save(&session);
        fx.vars.set(
            "CLAUDE_MONITOR_LAUNCHER_CLAUDE",
            "claude {name_args} --resume {session_id}",
        );
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=int1").body;
        let body = json_page(&rig, "/restore?session=int1").json();

        let command = body["would_start"][0]["command"].as_str().unwrap();
        assert!(
            command.ends_with("claude --name aquero --resume agent-int1"),
            "{command}"
        );
        let escaped = command.replace('&', "&amp;");
        assert!(html.contains(&format!("run: {escaped}")), "{html}");
    }

    #[test]
    fn a_finished_session_can_be_previewed_too() {
        let Some(fx) = Fixture::new() else { return };
        fx.add_session("fin1", State::Finished, "c");
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=fin1").body;

        assert!(
            html.contains("name=\"ids\" value=\"fin1\" checked"),
            "{html}"
        );
        assert!(!fx.server_has_sessions());
    }

    #[test]
    fn a_session_that_cannot_start_shows_its_refusal_and_no_confirm_form() {
        let Some(fx) = Fixture::new() else { return };
        let gone = fx.add_session("gone", State::Interrupted, "gone-dir");
        std::fs::remove_dir_all(&gone.cwd).unwrap();
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let html = page(&rig, "/restore?session=gone").body;

        assert!(html.contains("missing_directory"), "{html}");
        assert!(!html.contains("hx-post=\"/respawn\""), "{html}");
    }

    #[test]
    fn json_lists_only_that_session() {
        let Some(fx) = Fixture::new() else { return };
        fx.add_session("int1", State::Interrupted, "a");
        fx.add_session("int2", State::Interrupted, "b");
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());

        let body = json_page(&rig, "/restore?session=int1").json();

        let start = body["would_start"].as_array().unwrap();
        assert_eq!(start.len(), 1, "{body}");
        assert_eq!(start[0]["session"], "int1");
        assert_eq!(body["executed"], false);
    }

    #[test]
    fn an_unknown_session_is_a_404_page() {
        let web = Plain::new();

        let response = page(&web.rig, "/restore?session=nope");

        assert_eq!(response.status, 404);
        assert!(
            response.body.contains("no session matches"),
            "{}",
            response.body
        );
    }
}

mod confirmation_panel {
    use super::*;

    fn fragment(rig: &Rig, path: &str) -> Response {
        Request::get(path)
            .header("Cookie", rig.cookie())
            .header("HX-Request", "true")
            .send(rig.addr())
    }

    #[test]
    fn the_session_page_asks_for_the_panel_instead_of_dismissing() {
        let web = Plain::new();
        web.put("s-new", State::Idle);

        let html = page(&web.rig, "/sessions/s-new").body;

        assert!(
            html.contains("hx-get=\"/sessions/s-new/dismiss\""),
            "{html}"
        );
        assert!(html.contains("hx-target=\"#confirm-slot\""), "{html}");
        assert!(html.contains("id=\"confirm-slot\""), "{html}");
        assert!(
            !html.contains("hx-post=\"/sessions/s-new/dismiss\""),
            "{html}"
        );
    }

    #[test]
    fn the_dismiss_button_opts_out_of_the_live_region_hx_select() {
        let web = Plain::new();
        web.put("s-new", State::Idle);

        let html = page(&web.rig, "/sessions/s-new").body;

        let button = between(&html, "hx-get=\"/sessions/s-new/dismiss\"", ">");
        assert!(button.contains("hx-select=\"unset\""), "{button}");
    }

    #[test]
    fn the_htmx_fragment_names_the_target_and_consequence_with_confirm_and_cancel() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);

        let response = fragment(&web.rig, "/sessions/s-fin/dismiss");

        assert_eq!(response.status, 200, "{}", response.body);
        let html = response.body;
        assert!(!html.contains("<html"), "a fragment, not a page: {html}");
        assert!(html.contains("Dismiss session s-fin"), "{html}");
        assert!(html.contains("nothing is deleted"), "{html}");
        assert!(
            html.contains("hx-post=\"/sessions/s-fin/dismiss\""),
            "{html}"
        );
        assert!(
            html.contains("<input type=\"hidden\" name=\"confirm\" value=\"1\">"),
            "{html}"
        );
        assert!(html.contains("data-confirm-yes"), "{html}");
        assert!(
            html.contains("href=\"/sessions/s-fin\""),
            "cancel link: {html}"
        );
        assert!(html.contains("Cancel"), "{html}");
        assert!(!html.contains("role=\"dialog\"") && !html.contains("role=\"alertdialog\""));
        assert!(!web.session("s-fin").dismissed);
    }

    #[test]
    fn without_htmx_the_same_route_is_a_whole_page() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);

        let response = page(&web.rig, "/sessions/s-fin/dismiss");

        assert_eq!(response.status, 200, "{}", response.body);
        assert!(response.body.contains("<html"), "{}", response.body);
        assert!(response.body.contains("Dismiss session s-fin"));
        assert!(response
            .body
            .contains("hx-post=\"/sessions/s-fin/dismiss\""));
        assert!(!web.session("s-fin").dismissed);
    }

    #[test]
    fn a_project_panel_says_it_is_archived_and_cancels_back_to_the_project() {
        let web = Plain::new();
        let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let html = fragment(&web.rig, &format!("/projects/{id}/dismiss")).body;

        assert!(html.contains("Dismiss project alpha"), "{html}");
        assert!(html.contains("archived"), "{html}");
        assert!(
            html.contains(&format!("hx-post=\"/projects/{id}/dismiss\"")),
            "{html}"
        );
        assert!(html.contains(&format!("href=\"/projects/{id}\"")), "{html}");
    }

    #[test]
    fn the_unlink_panel_names_the_node_and_carries_it_in_the_form() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);
        post(
            &web.rig,
            "/sessions/s-fin/link",
            &json!({"cortex": "42", "kind": "task"}),
        );

        let html = fragment(&web.rig, "/sessions/s-fin/unlink?cortex=42").body;

        assert!(html.contains("Unlink session s-fin from node 42"), "{html}");
        assert!(
            html.contains("hx-post=\"/sessions/s-fin/unlink\""),
            "{html}"
        );
        assert!(
            html.contains("<input type=\"hidden\" name=\"cortex\" value=\"42\">"),
            "{html}"
        );
        assert!(html.contains("<input type=\"hidden\" name=\"confirm\" value=\"1\">"));
    }

    #[test]
    fn the_script_focuses_the_confirm_button_and_escape_cancels() {
        let web = Plain::new();

        let script = page(&web.rig, "/assets/app.js").body;

        assert!(script.contains("data-confirm-yes"), "{script}");
        assert!(script.contains("\"Escape\""), "{script}");
        assert!(script.contains(".focus()"), "{script}");
    }
}

mod dismiss_from_a_list_row {
    use super::*;

    #[test]
    fn a_waiting_row_asks_for_the_confirmation_panel_and_never_posts_the_dismissal() {
        let web = Plain::new();
        web.put("s-int", State::Interrupted);
        web.put("s-perm", State::NeedsPermission);

        let html = page(&web.rig, "/").body;

        let band = between(&html, "id=\"waiting\"", "</section>");
        for id in ["s-int", "s-perm"] {
            assert!(
                band.contains(&format!("hx-get=\"/sessions/{id}/dismiss\"")),
                "{id}: {band}"
            );
            assert!(
                !html.contains(&format!("hx-post=\"/sessions/{id}/dismiss\"")),
                "{id} must not dismiss on the first click"
            );
        }
        assert_eq!(html.matches("id=\"confirm-slot\"").count(), 1, "{html}");
    }

    #[test]
    fn the_respawn_link_stays_next_to_it() {
        let web = Plain::new();
        web.put("s-int", State::Interrupted);

        let html = page(&web.rig, "/").body;

        assert!(html.contains("href=\"/restore?session=s-int\""), "{html}");
    }

    #[test]
    fn rows_that_are_not_waiting_get_no_dismiss_control() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);
        web.put("s-work", State::Working);
        web.put("s-dis", State::Interrupted);
        let mut dismissed = web.session("s-dis");
        dismissed.dismissed = true;
        web.save(&dismissed);

        let html = page(&web.rig, "/?include_dismissed=true").body;

        for id in ["s-fin", "s-work", "s-dis"] {
            assert!(
                !html.contains(&format!("hx-get=\"/sessions/{id}/dismiss\"")),
                "{id}: {html}"
            );
        }
    }

    #[test]
    fn the_detail_page_keeps_exactly_one_slot() {
        let web = Plain::new();
        web.put("s-int", State::Interrupted);

        let html = page(&web.rig, "/sessions/s-int").body;

        assert_eq!(html.matches("id=\"confirm-slot\"").count(), 1, "{html}");
    }
}

mod dismiss_selected_on_restore {
    use super::*;

    struct Restorable {
        fx: Fixture,
        rig: Rig,
    }

    impl Restorable {
        fn session(&self, id: &str) -> claude_monitor::model::Session {
            self.fx.store().get_session(id).unwrap().unwrap()
        }
    }

    /// Three interrupted sessions /restore would start; `task` is the
    /// verbatim task of the first. `None` without tmux, as the other restore
    /// tests.
    fn restorable(task: &str) -> Option<Restorable> {
        let fx = Fixture::new()?;
        for (id, dir) in [("int1", "a"), ("int2", "b"), ("int3", "c")] {
            let mut session = fx.add_session(id, State::Interrupted, dir);
            if id == "int1" {
                session.task_description = Some(task.to_owned());
                fx.save(&session);
            }
        }
        let rig = Rig::start_on(&fx.state_dir, fx.env.clone());
        Some(Restorable { fx, rig })
    }

    fn fragment(rig: &Rig, path: &str) -> Response {
        Request::get(path)
            .header("Cookie", rig.cookie())
            .header("HX-Request", "true")
            .send(rig.addr())
    }

    fn would_start(rig: &Rig) -> Vec<String> {
        let body = Request::get("/restore")
            .header("Cookie", rig.cookie())
            .header("Accept", "application/json")
            .send(rig.addr())
            .json();
        let mut ids: Vec<String> = body["would_start"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["session"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    }

    fn dismiss_post(rig: &Rig, form: &str) -> Response {
        Request::new("POST", "/restore/dismiss")
            .signed_in(rig)
            .form_body(form)
            .send(rig.addr())
    }

    #[test]
    fn the_button_asks_for_the_panel_with_the_same_checkboxes_and_never_posts() {
        let Some(web) = restorable("t") else { return };

        let html = page(&web.rig, "/restore").body;

        let button = between(&html, "hx-get=\"/restore/dismiss\"", ">");
        assert!(button.contains("hx-include=\"closest form\""), "{button}");
        assert!(button.contains("hx-target=\"#confirm-slot\""), "{button}");
        assert!(!html.contains("hx-post=\"/restore/dismiss\""), "{html}");
        assert!(html.contains("Restore selected"), "{html}");
        assert!(html.contains("hx-post=\"/respawn\""), "{html}");
    }

    #[test]
    fn the_confirmation_lists_exactly_the_chosen_sessions_and_changes_nothing() {
        let Some(web) = restorable("t") else { return };

        let response = fragment(&web.rig, "/restore/dismiss?ids=int1&ids=int3");

        assert_eq!(response.status, 200, "{}", response.body);
        let html = response.body;
        assert!(!html.contains("<html"), "a fragment: {html}");
        assert!(html.contains("Dismiss 2 sessions"), "{html}");
        assert!(html.contains("int1") && html.contains("int3"), "{html}");
        assert!(!html.contains("int2"), "{html}");
        assert!(html.contains("hx-post=\"/restore/dismiss\""), "{html}");
        for id in ["int1", "int3"] {
            assert!(
                html.contains(&format!(
                    "<input type=\"hidden\" name=\"ids\" value=\"{id}\">"
                )),
                "{id}: {html}"
            );
        }
        assert!(html.contains("name=\"confirm\" value=\"1\""), "{html}");
        assert!(html.contains("href=\"/restore\""), "cancel: {html}");
        assert_eq!(would_start(&web.rig), ["int1", "int2", "int3"]);
    }

    #[test]
    fn hostile_task_text_stays_escaped_in_the_panel() {
        let Some(web) = restorable("<script>alert(1)</script>") else {
            return;
        };

        let html = fragment(&web.rig, "/restore/dismiss?ids=int1").body;

        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn without_htmx_the_confirmation_is_a_whole_page() {
        let Some(web) = restorable("t") else { return };

        let html = page(&web.rig, "/restore/dismiss?ids=int1").body;

        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains("Dismiss session int1"), "{html}");
    }

    #[test]
    fn json_describes_the_dismissal_and_changes_nothing() {
        let Some(web) = restorable("t") else { return };

        let response = Request::get("/restore/dismiss?ids=int1&ids=int2")
            .header("Cookie", web.rig.cookie())
            .header("Accept", "application/json")
            .send(web.rig.addr());

        assert_eq!(response.status, 200, "{}", response.body);
        let body = response.json();
        assert_eq!(body["confirmation_required"], true);
        assert_eq!(body["confirm_with"]["url"], "/restore/dismiss");
        assert_eq!(would_start(&web.rig).len(), 3);
    }

    #[test]
    fn posting_without_confirm_is_a_409_and_changes_nothing() {
        let Some(web) = restorable("t") else { return };

        let response = dismiss_post(&web.rig, "ids=int1&ids=int2");

        assert_eq!(response.status, 409, "{}", response.body);
        assert_eq!(response.json()["code"], "confirmation_required");
        assert_eq!(would_start(&web.rig).len(), 3);
    }

    #[test]
    fn posting_without_the_csrf_header_is_rejected() {
        let Some(web) = restorable("t") else { return };

        let response = Request::new("POST", "/restore/dismiss")
            .header("Cookie", web.rig.cookie())
            .form_body("ids=int1&confirm=1")
            .send(web.rig.addr());

        assert_eq!(response.status, 403);
        assert_eq!(would_start(&web.rig).len(), 3);
    }

    #[test]
    fn posting_without_a_session_is_rejected() {
        let Some(web) = restorable("t") else { return };

        let response = Request::new("POST", "/restore/dismiss")
            .form_body("ids=int1&confirm=1")
            .send(web.rig.addr());

        assert!(
            response.status == 401 || response.status == 403 || response.status == 302,
            "{}",
            response.status
        );
        assert_eq!(would_start(&web.rig).len(), 3);
    }

    #[test]
    fn a_confirmed_post_dismisses_only_the_chosen_sessions() {
        let Some(web) = restorable("t") else { return };

        let response = dismiss_post(&web.rig, "ids=int1&ids=int3&confirm=1");

        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(would_start(&web.rig), ["int2"]);
        assert!(web.session("int1").dismissed && web.session("int3").dismissed);
        assert!(!web.session("int2").dismissed);
        let html = page(&web.rig, "/?include_dismissed=true").body;
        let dismissed = section(&html, "dismissed");
        assert!(dismissed.contains("int1") && dismissed.contains("int3"));
        assert!(!dismissed.contains("int2"));
    }

    #[test]
    fn a_partial_failure_names_what_was_dismissed_and_what_was_not() {
        let Some(web) = restorable("t") else { return };
        web.fx
            .store()
            .connection()
            .execute_batch(
                "CREATE TRIGGER refuse_int2 BEFORE UPDATE ON sessions WHEN NEW.id = 'int2' \
                 BEGIN SELECT RAISE(ABORT, 'refused for the test'); END;",
            )
            .unwrap();

        let response = dismiss_post(&web.rig, "ids=int1&ids=int2&ids=int3&confirm=1");

        assert_eq!(response.status, 409, "{}", response.body);
        let body = response.json();
        assert_eq!(body["code"], "partly_dismissed");
        assert_eq!(body["dismissed"], json!(["int1", "int3"]));
        assert_eq!(body["failed"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body["failed"][0]["session"], "int2");
        let error = body["error"].as_str().unwrap();
        assert!(
            error.contains("int1") && error.contains("int2") && error.contains("int3"),
            "the page shows this text: {error}"
        );
        assert_eq!(
            response.header("hx-trigger"),
            Some("cc-changed"),
            "the lists must refresh: {response:?}"
        );
        assert!(web.session("int1").dismissed && web.session("int3").dismissed);
        assert!(!web.session("int2").dismissed);
    }

    #[test]
    fn an_unknown_id_is_a_404_and_dismisses_nothing() {
        let Some(web) = restorable("t") else { return };

        let read = fragment(&web.rig, "/restore/dismiss?ids=int1&ids=nope");
        let write = dismiss_post(&web.rig, "ids=int1&ids=nope&confirm=1");

        assert_eq!(read.status, 404, "{}", read.body);
        assert_eq!(write.status, 404, "{}", write.body);
        assert!(read.body.contains("nope"), "{}", read.body);
        assert!(write.body.contains("nope"), "{}", write.body);
        assert!(!web.session("int1").dismissed);
    }

    #[test]
    fn naming_no_session_is_a_usage_error() {
        let Some(web) = restorable("t") else { return };

        let read = fragment(&web.rig, "/restore/dismiss");
        let write = dismiss_post(&web.rig, "confirm=1");

        assert_eq!(read.status, 400, "{}", read.body);
        assert_eq!(write.status, 400, "{}", write.body);
    }
}
