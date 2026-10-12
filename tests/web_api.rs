//! The JSON side of the web view (FR-030): reads mirror `list --json` and
//! `show --json`, writes run the library calls the CLI runs, over real
//! loopback sockets. Respawns go to a private tmux server.
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
use serde_json::{json, Value};
use web_support::{Plain, Request, Response, Rig};

fn get(rig: &Rig, path: &str) -> Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .header("Accept", "application/json")
        .send(rig.addr())
}

fn post(rig: &Rig, path: &str, body: &Value) -> Response {
    Request::new("POST", path)
        .signed_in(rig)
        .json_body(body)
        .send(rig.addr())
}

fn confirmed() -> Value {
    json!({"confirm": "1"})
}

fn list_command() -> Command {
    Command::List {
        states: vec![],
        agent: None,
        repo: None,
        project: None,
        text: None,
        section: None,
        include_dismissed: false,
        expand_finished: false,
        expand_unseen: false,
    }
}

fn section_of(rig: &Rig, id: &str) -> String {
    let sessions = get(rig, "/sessions?include_dismissed=true").json();
    sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("{id} not listed: {sessions}"))["section"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn the_session_list_is_what_list_json_prints() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-wait", State::NeedsPermission);
    web.put("s-fin", State::Finished);

    let response = get(&web.rig, "/sessions");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type").unwrap(), "application/json");
    assert_eq!(response.json(), web.cli(list_command()));
}

#[test]
fn list_filters_arrive_as_query_parameters() {
    let web = Plain::new();
    web.put("s-work", State::Working);
    web.put("s-fin", State::Finished);

    let response = get(&web.rig, "/sessions?state=finished&text=task");

    let ids: Vec<_> = response.json()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["s-fin"]);
}

#[test]
fn a_session_detail_is_what_show_json_prints() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let response = get(&web.rig, "/sessions/s-fin");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.json(),
        web.cli(Command::Show {
            session: "s-fin".to_owned()
        })
    );
}

#[test]
fn an_unknown_session_is_a_404_with_the_refusal_code() {
    let web = Plain::new();

    let response = get(&web.rig, "/sessions/nope");

    assert_eq!(response.status, 404);
    assert_eq!(response.json()["code"], "unknown_session");
    assert_eq!(response.json()["ok"], false);
}

#[test]
fn done_moves_a_finished_session_to_the_done_section_and_undone_returns_it() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    assert_eq!(section_of(&web.rig, "s-fin"), "finished");

    let done = post(&web.rig, "/sessions/s-fin/done", &json!({}));
    assert_eq!(done.status, 200, "{}", done.body);
    assert_eq!(done.json()["changed"], json!(["s-fin"]));
    assert_eq!(section_of(&web.rig, "s-fin"), "done");

    let undone = post(&web.rig, "/sessions/s-fin/undone", &json!({}));
    assert_eq!(undone.status, 200);
    assert_eq!(section_of(&web.rig, "s-fin"), "finished");
}

#[test]
fn done_on_a_session_that_is_not_finished_is_a_409_with_the_reason() {
    let web = Plain::new();
    web.put("s-work", State::Working);

    let response = post(&web.rig, "/sessions/s-work/done", &json!({}));

    assert_eq!(response.status, 409);
    assert_eq!(response.json()["code"], "not_finished");
    assert!(response.json()["error"]
        .as_str()
        .unwrap()
        .contains("s-work"));
}

#[test]
fn a_write_without_the_csrf_header_changes_nothing() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let response = Request::new("POST", "/sessions/s-fin/done")
        .header("Cookie", web.rig.cookie())
        .json_body(&json!({}))
        .send(web.rig.addr());

    assert_eq!(response.status, 403);
    assert_eq!(section_of(&web.rig, "s-fin"), "finished");
}

#[test]
fn dismiss_hides_a_session_by_default_and_undismiss_brings_it_back() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    assert_eq!(
        post(&web.rig, "/sessions/s-fin/dismiss", &confirmed()).status,
        200
    );
    let hidden = get(&web.rig, "/sessions").json();
    assert!(hidden["sessions"].as_array().unwrap().is_empty());
    assert_eq!(section_of(&web.rig, "s-fin"), "dismissed");

    assert_eq!(
        post(&web.rig, "/sessions/s-fin/undismiss", &json!({})).status,
        200
    );
    assert_eq!(section_of(&web.rig, "s-fin"), "finished");
}

mod confirmation {
    use super::*;

    fn refusal(response: &Response) -> Value {
        assert_eq!(response.status, 409, "{}", response.body);
        let body = response.json();
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "confirmation_required");
        assert!(
            body["error"].as_str().unwrap().contains("confirm"),
            "{body}"
        );
        body
    }

    #[test]
    fn dismissing_a_session_without_confirm_is_a_409_and_changes_nothing() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);

        for body in [
            json!({}),
            json!({"confirm": "0"}),
            json!({"confirm": false}),
        ] {
            refusal(&post(&web.rig, "/sessions/s-fin/dismiss", &body));
        }

        assert_eq!(section_of(&web.rig, "s-fin"), "finished");
    }

    #[test]
    fn confirm_as_json_or_form_performs_the_dismissal() {
        let web = Plain::new();
        web.put("s-a", State::Finished);
        web.put("s-b", State::Finished);

        let json_confirmed = post(&web.rig, "/sessions/s-a/dismiss", &json!({"confirm": true}));
        let form_confirmed = Request::new("POST", "/sessions/s-b/dismiss")
            .signed_in(&web.rig)
            .form_body("confirm=1")
            .send(web.rig.addr());

        assert_eq!(json_confirmed.status, 200, "{}", json_confirmed.body);
        assert_eq!(form_confirmed.status, 200, "{}", form_confirmed.body);
        assert_eq!(section_of(&web.rig, "s-a"), "dismissed");
        assert_eq!(section_of(&web.rig, "s-b"), "dismissed");
    }

    #[test]
    fn dismissing_a_project_without_confirm_is_a_409_and_changes_nothing() {
        let web = Plain::new();
        let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        refusal(&post(
            &web.rig,
            &format!("/projects/{id}/dismiss"),
            &json!({}),
        ));

        assert_eq!(project_status(&web.rig, &id), "active");
    }

    #[test]
    fn unlinking_without_confirm_is_a_409_and_keeps_the_link() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);
        post(
            &web.rig,
            "/sessions/s-fin/link",
            &json!({"cortex": "42", "kind": "task"}),
        );

        refusal(&post(
            &web.rig,
            "/sessions/s-fin/unlink",
            &json!({"cortex": "42"}),
        ));

        assert_eq!(
            get(&web.rig, "/links").json()["links"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn an_unknown_target_is_a_404_whether_or_not_it_is_confirmed() {
        let web = Plain::new();

        for body in [json!({}), confirmed()] {
            let session = post(&web.rig, "/sessions/nope/dismiss", &body);
            assert_eq!(session.status, 404, "{}", session.body);
            let project = post(&web.rig, "/projects/nope/dismiss", &body);
            assert_eq!(project.status, 404, "{}", project.body);
        }
    }

    #[test]
    fn a_get_describes_the_dismissal_as_json_and_changes_nothing() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);

        let response = get(&web.rig, "/sessions/s-fin/dismiss");

        assert_eq!(response.status, 200, "{}", response.body);
        let body = response.json();
        assert_eq!(body["confirmation_required"], true);
        assert_eq!(body["action"], "Dismiss session s-fin");
        assert!(body["consequence"]
            .as_str()
            .unwrap()
            .contains("nothing is deleted"));
        assert_eq!(section_of(&web.rig, "s-fin"), "finished");
    }

    #[test]
    fn a_get_for_an_unknown_target_is_a_404() {
        let web = Plain::new();

        assert_eq!(get(&web.rig, "/sessions/nope/dismiss").status, 404);
        assert_eq!(get(&web.rig, "/projects/nope/dismiss").status, 404);
    }

    #[test]
    fn the_csrf_header_is_still_required_with_confirm() {
        let web = Plain::new();
        web.put("s-fin", State::Finished);

        let response = Request::new("POST", "/sessions/s-fin/dismiss")
            .header("Cookie", web.rig.cookie())
            .json_body(&confirmed())
            .send(web.rig.addr());

        assert_eq!(response.status, 403);
        assert_eq!(section_of(&web.rig, "s-fin"), "finished");
    }
}

#[test]
fn park_and_unpark_set_and_clear_the_flag() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let parked = post(&web.rig, "/sessions/s-fin/park", &json!({}));
    assert_eq!(parked.status, 200);
    assert_eq!(parked.json()["changed"], true);
    let flags = get(&web.rig, "/sessions/s-fin").json()["flags"].clone();
    assert!(flags.as_array().unwrap().contains(&json!("parked")));

    post(&web.rig, "/sessions/s-fin/unpark", &json!({}));
    let flags = get(&web.rig, "/sessions/s-fin").json()["flags"].clone();
    assert!(!flags.as_array().unwrap().contains(&json!("parked")));
}

#[test]
fn a_note_is_added_from_a_json_or_a_form_body() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let json_note = post(
        &web.rig,
        "/sessions/s-fin/note",
        &json!({"text": "from json"}),
    );
    let form_note = Request::new("POST", "/sessions/s-fin/note")
        .signed_in(&web.rig)
        .form_body("text=from+form%21")
        .send(web.rig.addr());

    assert_eq!(json_note.status, 200, "{}", json_note.body);
    assert_eq!(form_note.status, 200, "{}", form_note.body);
    let notes = get(&web.rig, "/sessions/s-fin").json()["notes"].clone();
    let bodies: Vec<_> = notes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(bodies, ["from json", "from form!"]);
}

#[test]
fn an_empty_note_is_a_400() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let response = post(&web.rig, "/sessions/s-fin/note", &json!({"text": "  "}));

    assert_eq!(response.status, 400);
}

#[test]
fn projects_are_created_grouped_renamed_and_listed() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);
    web.put("s-work", State::Working);

    let created = post(&web.rig, "/projects", &json!({"name": "alpha"}));
    assert_eq!(created.status, 200, "{}", created.body);
    let id = created.json()["project"]["id"].as_str().unwrap().to_owned();
    let duplicate = post(&web.rig, "/projects", &json!({"name": "alpha"}));
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.json()["code"], "project_exists");

    let grouped = post(
        &web.rig,
        &format!("/projects/{id}"),
        &json!({"sessions": ["s-fin"]}),
    );
    assert_eq!(grouped.status, 200, "{}", grouped.body);
    let one_more = post(
        &web.rig,
        "/sessions/s-work/group",
        &json!({"project": "alpha"}),
    );
    assert_eq!(one_more.status, 200, "{}", one_more.body);

    let page = get(&web.rig, &format!("/projects/{id}")).json();
    assert_eq!(page["project"]["name"], "alpha");
    let ids: Vec<_> = page["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s-work", "s-fin"]);

    let renamed = post(
        &web.rig,
        &format!("/projects/{id}"),
        &json!({"name": "beta"}),
    );
    assert_eq!(renamed.status, 200, "{}", renamed.body);
    let listed = get(&web.rig, "/projects").json();
    assert_eq!(listed["projects"][0]["name"], "beta");

    let cleared = post(&web.rig, "/sessions/s-work/group", &json!({"project": "-"}));
    assert_eq!(cleared.json()["changed"], json!(["s-work"]));
}

#[test]
fn a_projects_done_section_holds_only_its_own_done_sessions() {
    let web = Plain::new();
    web.put("in-a", State::Finished);
    web.put("in-b", State::Finished);
    let a = post(&web.rig, "/projects", &json!({"name": "a"})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    post(&web.rig, "/projects", &json!({"name": "b"}));
    post(&web.rig, "/sessions/in-a/group", &json!({"project": "a"}));
    post(&web.rig, "/sessions/in-b/group", &json!({"project": "b"}));
    post(&web.rig, "/sessions/in-a/done", &json!({}));
    post(&web.rig, "/sessions/in-b/done", &json!({}));

    let page = get(&web.rig, &format!("/projects/{a}")).json();

    let done: Vec<_> = page["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["section"] == "done")
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(done, ["in-a"]);
}

#[test]
fn an_unknown_project_is_a_404() {
    let web = Plain::new();

    let read = get(&web.rig, "/projects/nope");
    assert_eq!(read.status, 404);
    assert_eq!(read.json()["code"], "unknown_project");
    let write = post(&web.rig, "/projects/nope", &json!({"name": "x"}));
    assert_eq!(write.status, 404);
    assert_eq!(write.json()["code"], "unknown_project");
}

#[test]
fn a_project_note_is_stored() {
    let web = Plain::new();
    let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let response = post(
        &web.rig,
        &format!("/projects/{id}/note"),
        &json!({"text": "plan B"}),
    );

    assert_eq!(response.status, 200, "{}", response.body);
    let page = get(&web.rig, &format!("/projects/{id}")).json();
    assert_eq!(page["notes"][0]["body"], "plan B");
}

fn project_status(rig: &Rig, id: &str) -> String {
    get(rig, &format!("/projects/{id}")).json()["project"]["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_project_is_parked_unparked_dismissed_and_undismissed() {
    let web = Plain::new();
    let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let parked = post(&web.rig, &format!("/projects/{id}/park"), &json!({}));
    assert_eq!(parked.status, 200, "{}", parked.body);
    assert_eq!(parked.json()["target"]["kind"], "project");
    assert_eq!(project_status(&web.rig, &id), "parked");

    post(&web.rig, &format!("/projects/{id}/unpark"), &json!({}));
    assert_eq!(project_status(&web.rig, &id), "active");

    post(&web.rig, &format!("/projects/{id}/dismiss"), &confirmed());
    assert_eq!(project_status(&web.rig, &id), "archived");

    post(&web.rig, &format!("/projects/{id}/undismiss"), &json!({}));
    assert_eq!(project_status(&web.rig, &id), "active");
}

#[test]
fn a_project_marker_route_for_an_unknown_project_is_a_404() {
    let web = Plain::new();

    for action in ["park", "unpark", "dismiss", "undismiss", "summarize"] {
        let response = post(&web.rig, &format!("/projects/nope/{action}"), &json!({}));
        assert_eq!(response.status, 404, "{action}: {}", response.body);
        assert_eq!(response.json()["code"], "unknown_project");
    }
}

#[test]
fn summarizing_a_project_without_a_model_is_refused_like_a_session() {
    let web = Plain::new();
    let id = post(&web.rig, "/projects", &json!({"name": "alpha"})).json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let response = post(&web.rig, &format!("/projects/{id}/summarize"), &json!({}));

    assert_eq!(response.status, 409);
    assert_eq!(response.json()["code"], "summary_not_configured");
}

#[test]
fn the_progress_source_can_be_set_to_none() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let response = post(
        &web.rig,
        "/sessions/s-fin/progress-source",
        &json!({"source": "none"}),
    );

    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.json()["status"], "none");
}

#[test]
fn summarize_without_a_configured_model_is_refused_not_a_server_error() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let response = post(&web.rig, "/sessions/s-fin/summarize", &json!({}));

    assert_eq!(response.status, 409);
    assert_eq!(response.json()["code"], "summary_not_configured");
}

#[test]
fn links_are_added_listed_and_removed() {
    let web = Plain::new();
    web.put("s-fin", State::Finished);

    let linked = post(
        &web.rig,
        "/sessions/s-fin/link",
        &json!({"cortex": "42", "kind": "task"}),
    );
    assert_eq!(linked.status, 200, "{}", linked.body);
    let listed = get(&web.rig, "/links").json();
    assert_eq!(listed["links"][0]["owner"]["id"], "s-fin");

    let bad = post(
        &web.rig,
        "/sessions/s-fin/link",
        &json!({"cortex": "forty-two", "kind": "task"}),
    );
    assert_eq!(bad.status, 400);

    let removed = post(
        &web.rig,
        "/sessions/s-fin/unlink",
        &json!({"cortex": "42", "confirm": "1"}),
    );
    assert_eq!(removed.status, 200, "{}", removed.body);
    assert!(get(&web.rig, "/links").json()["links"]
        .as_array()
        .unwrap()
        .is_empty());
}

fn tmux_fixture() -> Option<(Fixture, Rig)> {
    let fx = Fixture::new()?;
    let rig = Rig::start_on(&fx.state_dir, fx.env.clone());
    Some((fx, rig))
}

#[test]
fn a_respawn_starts_the_session_in_a_private_tmux_pane() {
    let Some((fx, rig)) = tmux_fixture() else {
        return;
    };
    fx.add_session("int1", State::Interrupted, "a");

    let response = post(&rig, "/sessions/int1/respawn", &json!({"fresh": false}));

    assert_eq!(response.status, 200, "{}", response.body);
    let body = response.json();
    assert_eq!(body["started"][0]["session"], "int1");
    assert_eq!(body["refused"], json!([]));
    assert_eq!(fx.pane_ids().len(), 1, "only the started session's window");
}

#[test]
fn a_respawn_refusal_is_data_in_a_200_response_and_starts_nothing() {
    let Some((fx, rig)) = tmux_fixture() else {
        return;
    };
    let session = fx.add_session("gone", State::Interrupted, "gone-dir");
    std::fs::remove_dir_all(&session.cwd).unwrap();

    let response = post(&rig, "/sessions/gone/respawn", &json!({"fresh": false}));

    assert_eq!(response.status, 200, "{}", response.body);
    let body = response.json();
    assert_eq!(body["started"], json!([]));
    assert_eq!(body["refused"][0]["id"], "gone");
    assert_eq!(body["refused"][0]["code"], "missing_directory");
    assert!(body["refused"][0]["alternatives"].is_array());
    assert!(!fx.server_has_sessions());
}

#[test]
fn a_batch_respawn_takes_ids_or_all_interrupted_except() {
    let Some((fx, rig)) = tmux_fixture() else {
        return;
    };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("int2", State::Interrupted, "b");
    fx.add_session("fin1", State::Finished, "c");

    let some = post(&rig, "/respawn", &json!({"ids": ["fin1"]}));
    assert_eq!(some.json()["started"][0]["session"], "fin1");

    let rest = post(
        &rig,
        "/respawn",
        &json!({"all_interrupted": true, "except": ["int2"]}),
    );
    let started: Vec<_> = rest.json()["started"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(started, ["int1"]);
    assert_eq!(rest.json()["skipped"], json!(["int2"]));
    assert_eq!(fx.pane_ids().len(), 2, "one window per started session");
}

#[test]
fn a_batch_respawn_that_names_nothing_is_a_400() {
    let Some((_fx, rig)) = tmux_fixture() else {
        return;
    };

    let response = post(&rig, "/respawn", &json!({}));

    assert_eq!(response.status, 400);
}

#[test]
fn the_restore_preview_lists_interrupted_sessions_and_starts_nothing() {
    let Some((fx, rig)) = tmux_fixture() else {
        return;
    };
    fx.add_session("int1", State::Interrupted, "a");
    fx.add_session("fin1", State::Finished, "b");
    let mut dismissed = fx.add_session("dis1", State::Interrupted, "c");
    dismissed.dismissed = true;
    fx.save(&dismissed);

    let response = get(&rig, "/restore");

    assert_eq!(response.status, 200);
    let body = response.json();
    assert_eq!(body["executed"], false);
    let ids: Vec<_> = body["would_start"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["int1"]);
    assert!(!fx.server_has_sessions());
}

fn project_named<'a>(tree: &'a Value, name: &str) -> &'a Value {
    tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no project {name} in {tree}"))
}

fn session_ids(node: &Value) -> Vec<&str> {
    node["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect()
}

#[test]
fn grouping_into_a_missing_project_creates_it_and_a_second_group_moves_the_session() {
    let web = Plain::new();
    web.put("s-one", State::Working);

    let created = post(
        &web.rig,
        "/sessions/s-one/group",
        &json!({"project": "fresh"}),
    );
    assert_eq!(created.status, 200, "{}", created.body);
    assert_eq!(created.json()["project"], "fresh");
    post(&web.rig, "/projects", &json!({"name": "other"}));
    let moved = post(
        &web.rig,
        "/sessions/s-one/group",
        &json!({"project": "other"}),
    );
    assert_eq!(moved.json()["changed"], json!(["s-one"]));
    let again = post(
        &web.rig,
        "/sessions/s-one/group",
        &json!({"project": "other"}),
    );
    assert_eq!(again.json()["unchanged"], json!(["s-one"]));

    let listed = get(&web.rig, "/sessions").json();
    assert_eq!(listed["sessions"][0]["project"], "other");
    let ungrouped = post(&web.rig, "/sessions/s-one/group", &json!({"project": "-"}));
    assert_eq!(ungrouped.json()["project"], Value::Null);
    let listed = get(&web.rig, "/sessions").json();
    assert!(listed["sessions"][0]["project"].is_null());
}

#[test]
fn the_overview_json_gains_a_project_tree_and_keeps_the_flat_fields() {
    let web = Plain::new();
    web.put("a-work", State::Working);
    web.put("a-int", State::Interrupted);
    web.put("a-fin", State::Finished);
    web.put("loose", State::Finished);
    post(&web.rig, "/projects", &json!({"name": "zeta"}));
    post(&web.rig, "/projects", &json!({"name": "alpha"}));
    for id in ["a-work", "a-int", "a-fin"] {
        post(
            &web.rig,
            &format!("/sessions/{id}/group"),
            &json!({"project": "alpha"}),
        );
    }

    let tree = get(&web.rig, "/").json();

    let names: Vec<_> = tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["alpha", "zeta"],
        "alphabetical, empty ones included"
    );
    let alpha = project_named(&tree, "alpha");
    assert_eq!(alpha["summary"], "1 working · 1 interrupted · 1 finished");
    assert_eq!(alpha["status"], "active");
    assert!(alpha["id"].as_str().is_some());
    assert_eq!(session_ids(alpha).len(), 3);
    assert!(session_ids(project_named(&tree, "zeta")).is_empty());
    assert_eq!(tree["no_project"]["sessions"][0]["id"], "loose");
    assert_eq!(
        tree["sessions"].as_array().unwrap().len(),
        4,
        "flat list kept"
    );
    assert_eq!(
        get(&web.rig, "/sessions").json(),
        web.cli(list_command()),
        "/sessions keeps the exact list --json shape"
    );
}
