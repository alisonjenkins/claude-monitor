//! `GET /events`: server-sent events pushed from the change feed, over a real
//! loopback socket with a fake wake source.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::time::Duration;

use claude_monitor::model::Author;
use claude_monitor::store::events::NewEvent;
use claude_monitor::store::Store;
use serde_json::{json, Value};
use web_support::{Request, Rig, Stream};

const AT: &str = "2026-10-12T14:03:22Z";

fn append(store: &Store, id: &str, kind: &str) -> i64 {
    store
        .append_event(&NewEvent {
            at: AT,
            entity_kind: "session",
            entity_id: id,
            kind,
            payload: &json!({}),
            author: &Author::System {
                component: "hook".to_owned(),
            },
        })
        .unwrap()
}

fn open_events(rig: &Rig, last_event_id: Option<&str>) -> Stream {
    let mut request = Request::get("/events").header("Cookie", rig.cookie());
    if let Some(id) = last_event_id {
        request = request.header("Last-Event-ID", id);
    }
    request.open(rig.addr())
}

/// `(id, data)` of one message, parsed.
fn parse(message: &[String]) -> (i64, Value) {
    let id = message
        .iter()
        .find_map(|l| l.strip_prefix("id: "))
        .unwrap_or_else(|| panic!("no id line in {message:?}"));
    let data = message
        .iter()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or_else(|| panic!("no data line in {message:?}"));
    (id.parse().unwrap(), serde_json::from_str(data).unwrap())
}

fn changed(entity: &str, id: &str, kind: &str) -> Value {
    json!({ "changed": [{ "entity": entity, "id": id, "kind": kind }] })
}

#[test]
#[ignore = "implemented by T118"]
fn the_stream_is_an_event_stream_that_is_not_cached() {
    let rig = Rig::start();

    let stream = open_events(&rig, None);

    assert_eq!(stream.head.status, 200);
    assert!(stream
        .head
        .header("content-type")
        .is_some_and(|v| v.starts_with("text/event-stream")));
    assert_eq!(stream.head.header("cache-control"), Some("no-store"));
}

#[test]
#[ignore = "implemented by T118"]
fn a_pushed_change_carries_its_seq_and_what_changed() {
    let rig = Rig::start();
    let mut stream = open_events(&rig, None);

    let seq = append(&rig.writer, "s-1", "state");
    rig.wake.wake();

    let message = stream.next_message().unwrap();
    let (id, data) = parse(&message);
    assert_eq!(id, seq);
    assert_eq!(data, changed("session", "s-1", "state"));
}

#[test]
#[ignore = "implemented by T118"]
fn several_changes_arrive_in_seq_order_one_message_each() {
    let rig = Rig::start();
    let mut stream = open_events(&rig, None);

    let first = append(&rig.writer, "s-1", "state");
    let second = append(&rig.writer, "s-2", "note");
    rig.wake.wake();

    let (id_a, data_a) = parse(&stream.next_message().unwrap());
    let (id_b, data_b) = parse(&stream.next_message().unwrap());
    assert_eq!((id_a, id_b), (first, second));
    assert_eq!(data_a, changed("session", "s-1", "state"));
    assert_eq!(data_b, changed("session", "s-2", "note"));
}

#[test]
#[ignore = "implemented by T118"]
fn a_new_connection_does_not_replay_history() {
    let rig = Rig::start();
    append(&rig.writer, "old", "state");
    let mut stream = open_events(&rig, None);

    let seq = append(&rig.writer, "new", "state");
    rig.wake.wake();

    let (id, data) = parse(&stream.next_message().unwrap());
    assert_eq!(id, seq);
    assert_eq!(data, changed("session", "new", "state"));
}

#[test]
#[ignore = "implemented by T118"]
fn reconnecting_with_last_event_id_replays_what_was_missed_from_the_table() {
    let rig = Rig::start();
    let seen = append(&rig.writer, "s-1", "state");
    let missed_a = append(&rig.writer, "s-2", "state");
    let missed_b = append(&rig.writer, "s-3", "park");

    let mut stream = open_events(&rig, Some(&seen.to_string()));

    let (id_a, data_a) = parse(&stream.next_message().unwrap());
    let (id_b, data_b) = parse(&stream.next_message().unwrap());
    assert_eq!((id_a, id_b), (missed_a, missed_b));
    assert_eq!(data_a, changed("session", "s-2", "state"));
    assert_eq!(data_b, changed("session", "s-3", "park"));
}

#[test]
#[ignore = "implemented by T118"]
fn after_a_replay_live_changes_follow_without_duplicates() {
    let rig = Rig::start();
    let seen = append(&rig.writer, "s-1", "state");
    let missed = append(&rig.writer, "s-2", "state");
    let mut stream = open_events(&rig, Some(&seen.to_string()));
    assert_eq!(parse(&stream.next_message().unwrap()).0, missed);

    let live = append(&rig.writer, "s-3", "state");
    rig.wake.wake();

    let (id, data) = parse(&stream.next_message().unwrap());
    assert_eq!(id, live, "the feed re-reads old rows; none may repeat");
    assert_eq!(data, changed("session", "s-3", "state"));
}

#[test]
#[ignore = "implemented by T118"]
fn a_last_event_id_that_is_not_a_number_means_live_only() {
    let rig = Rig::start();
    append(&rig.writer, "old", "state");
    let mut stream = open_events(&rig, Some("not-a-number"));
    assert_eq!(stream.head.status, 200);

    let seq = append(&rig.writer, "new", "state");
    rig.wake.wake();

    assert_eq!(parse(&stream.next_message().unwrap()).0, seq);
}

#[test]
#[ignore = "implemented by T118"]
fn an_idle_stream_sends_keepalive_comment_lines() {
    let rig = Rig::start_with(|config| config.keepalive = Duration::from_millis(100));
    let mut stream = open_events(&rig, None);

    let first = stream.next_message().unwrap();
    let second = stream.next_message().unwrap();

    for message in [first, second] {
        assert_eq!(message.len(), 1, "{message:?}");
        assert!(message[0].starts_with(':'), "{message:?}");
        assert!(message[0].contains("keepalive"), "{message:?}");
    }
}

#[test]
#[ignore = "implemented by T118"]
fn shutting_the_server_down_ends_open_streams() {
    let rig = Rig::start();
    let mut stream = open_events(&rig, None);

    rig.server.shutdown();

    assert_eq!(stream.next_line(), None, "the stream must reach EOF");
}
