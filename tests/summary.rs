//! Summaries (FR-017, FR-017a, FR-020): verbatim text from the session,
//! model-written text kept apart and labelled, loopback-only model calls.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use claude_monitor::hook::{ingest, HookContext, NoGit};
use claude_monitor::model::Session;
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::summary;
use claude_monitor::testing::FakeClock;
use serde_json::{json, Value};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const AGENT_SESSION: &str = "sess-verbatim";

struct Harness {
    _dir: TempDir,
    state_dir: PathBuf,
    transcript: PathBuf,
    clock: FakeClock,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        Self {
            state_dir: dir.path().join("state"),
            transcript: dir.path().join("transcript.jsonl"),
            _dir: dir,
            clock: FakeClock::new(T0),
        }
    }

    fn feed(&self, payload: Value) {
        let ctx = HookContext {
            agent: "claude",
            state_dir: &self.state_dir,
            tmux_pane: None,
            clock: &self.clock,
            git: &NoGit,
            agent_pid: None,
            env: None,
        };
        ingest(&ctx, &payload.to_string());
    }

    fn event(&self, name: &str, extra: Value) {
        let mut payload = json!({
            "session_id": AGENT_SESSION,
            "cwd": "/tmp/work",
            "hook_event_name": name,
        });
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        self.feed(payload);
    }

    fn session(&self) -> Session {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer())
            .unwrap()
            .get_session_by_agent_id("claude", AGENT_SESSION)
            .unwrap()
            .expect("session row exists")
    }
}

fn assistant_line(text: &str) -> String {
    json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
    })
    .to_string()
}

fn write_transcript(path: &Path, lines: &[String]) {
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

mod verbatim {
    use super::*;

    const PROMPT: &str = "  Fix the flaky test in\n  src/foo.rs  \n\n  then open a PR  ";

    #[test]
    fn first_prompt_is_stored_untouched_and_never_replaced() {
        let h = Harness::new();
        h.event("UserPromptSubmit", json!({"prompt": PROMPT}));
        h.event("UserPromptSubmit", json!({"prompt": "a later request"}));
        assert_eq!(h.session().task_description.as_deref(), Some(PROMPT));
    }

    #[test]
    fn stop_message_is_stored_untouched() {
        let h = Harness::new();
        let message = "Done.\n\n  - indented  \n";
        h.event("Stop", json!({"last_assistant_message": message}));
        assert_eq!(h.session().left_off.as_deref(), Some(message));
    }

    #[test]
    fn stop_without_a_message_falls_back_to_the_transcript_tail() {
        let h = Harness::new();
        write_transcript(
            &h.transcript,
            &[
                assistant_line("an older answer"),
                json!({"type": "user", "message": {"role": "user", "content": "go on"}})
                    .to_string(),
                assistant_line("the  latest\nanswer "),
            ],
        );
        h.event(
            "Stop",
            json!({"transcript_path": h.transcript.to_str().unwrap()}),
        );
        assert_eq!(
            h.session().left_off.as_deref(),
            Some("the  latest\nanswer ")
        );
    }

    #[test]
    fn stop_message_beats_the_transcript() {
        let h = Harness::new();
        write_transcript(&h.transcript, &[assistant_line("from the transcript")]);
        h.event(
            "Stop",
            json!({
                "last_assistant_message": "from the payload",
                "transcript_path": h.transcript.to_str().unwrap(),
            }),
        );
        assert_eq!(h.session().left_off.as_deref(), Some("from the payload"));
    }

    #[test]
    fn missing_transcript_leaves_left_off_empty() {
        let h = Harness::new();
        h.event(
            "Stop",
            json!({"transcript_path": h.transcript.to_str().unwrap()}),
        );
        assert_eq!(h.session().left_off, None);
    }

    #[test]
    fn tail_skips_tool_only_and_sidechain_messages() {
        let tool_only = json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Bash"}]},
        })
        .to_string();
        let sidechain = json!({
            "type": "assistant",
            "isSidechain": true,
            "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-agent"}]},
        })
        .to_string();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_line("real answer"),
                sidechain,
                tool_only,
                "not json at all".to_owned(),
            ],
        );
        assert_eq!(
            summary::transcript_tail_message(&path, summary::TRANSCRIPT_TAIL_BYTES).as_deref(),
            Some("real answer")
        );
    }

    #[test]
    fn tail_read_is_bounded_by_the_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.jsonl");
        let filler =
            json!({"type": "user", "message": {"role": "user", "content": "x".repeat(900)}})
                .to_string();
        let mut lines = vec![assistant_line("outside the window")];
        lines.extend(std::iter::repeat_n(filler, 400));
        write_transcript(&path, &lines);
        assert!(std::fs::metadata(&path).unwrap().len() > 300_000);

        assert_eq!(summary::transcript_tail_message(&path, 64 * 1024), None);
        assert_eq!(
            summary::transcript_tail_message(&path, 1024 * 1024).as_deref(),
            Some("outside the window")
        );
    }

    #[test]
    fn tail_window_that_cuts_a_line_in_half_ignores_the_partial_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cut.jsonl");
        let last = assistant_line("kept");
        let first = assistant_line(&"y".repeat(5000));
        write_transcript(&path, &[first, last.clone()]);
        let cap = u64::try_from(last.len()).unwrap() + 10;
        assert_eq!(
            summary::transcript_tail_message(&path, cap).as_deref(),
            Some("kept")
        );
    }
}

mod endpoint {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread::JoinHandle;

    use claude_monitor::summary::{
        ConfigError, Endpoint, EndpointError, ModelClient, ModelError, MODEL_VAR, URL_VAR,
    };
    use claude_monitor::testing::FakeEnv;

    use super::*;

    pub(super) struct Captured {
        pub(super) request_line: String,
        pub(super) headers: String,
        pub(super) body: Value,
    }

    /// Serves exactly one connection on a loopback port with `response`.
    pub(super) fn fake_server(response: String) -> (SocketAddr, JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            let (head_end, length) = loop {
                let n = stream.read(&mut chunk).unwrap();
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (end + 4, length);
                }
                assert!(n > 0, "client closed before finishing the request");
            };
            while raw.len() < head_end + length {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "client closed mid-body");
                raw.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
            let body = serde_json::from_slice(&raw[head_end..head_end + length]).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            let _ = stream.flush();
            Captured {
                request_line: head.lines().next().unwrap().to_owned(),
                headers: head.to_ascii_lowercase(),
                body,
            }
        });
        (addr, handle)
    }

    fn http(status_line: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        )
    }

    pub(super) fn completion(text: &str) -> String {
        http(
            "200 OK",
            "",
            &json!({"choices": [{"message": {"role": "assistant", "content": text}}]}).to_string(),
        )
    }

    fn client_for(addr: SocketAddr) -> ModelClient {
        let endpoint = Endpoint::parse(&format!("http://{addr}")).unwrap();
        ModelClient::new(endpoint, "test-model")
    }

    #[test]
    fn loopback_endpoints_are_accepted() {
        for (url, addr, path) in [
            (
                "http://127.0.0.1:8081",
                "127.0.0.1:8081",
                "/v1/chat/completions",
            ),
            ("http://127.1.2.3:1/", "127.1.2.3:1", "/v1/chat/completions"),
            ("http://localhost:9", "127.0.0.1:9", "/v1/chat/completions"),
            (
                "http://LOCALHOST:9/api/",
                "127.0.0.1:9",
                "/api/v1/chat/completions",
            ),
            ("http://[::1]:8080/", "[::1]:8080", "/v1/chat/completions"),
            ("http://127.0.0.1", "127.0.0.1:80", "/v1/chat/completions"),
        ] {
            let parsed = Endpoint::parse(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(parsed.addr(), addr.parse::<SocketAddr>().unwrap(), "{url}");
            assert_eq!(parsed.chat_path(), path, "{url}");
        }
    }

    #[test]
    fn anything_not_provably_loopback_is_rejected_before_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let bad = [
            "http://example.com".to_owned(),
            "http://192.168.1.5:8080".to_owned(),
            "http://0.0.0.0:8080".to_owned(),
            "http://10.0.0.1".to_owned(),
            "http://[::2]:8080".to_owned(),
            "http://[::ffff:127.0.0.1]:8080".to_owned(),
            "http://2130706433:8080".to_owned(),
            "http://127.1:8080".to_owned(),
            format!("http://127.0.0.1@evil.example:{port}"),
            format!("http://evil.example@127.0.0.1:{port}"),
            format!("http://user:pw@127.0.0.1:{port}"),
            format!("http://127.0.0.1.evil.example:{port}"),
            format!("http://localhost.evil.example:{port}"),
            format!("http://evil.example:{port}/127.0.0.1"),
            format!("http://127.0.0.1:{port}?next=http://evil.example"),
            format!("http://127.0.0.1:{port}#x"),
            format!("http://127.0.0.1:{port}/a b"),
            format!("http://127.0.0.1:{port}/\r\nX-Evil: 1"),
            "http://127.0.0.1:99999".to_owned(),
            "https://127.0.0.1:8080".to_owned(),
            "ftp://127.0.0.1".to_owned(),
            "127.0.0.1:8080".to_owned(),
            "http://".to_owned(),
            String::new(),
        ];
        for url in &bad {
            assert!(Endpoint::parse(url).is_err(), "{url:?} was accepted");
        }
        let err = Endpoint::parse("http://example.com").unwrap_err();
        assert!(matches!(err, EndpointError::NotLoopback { .. }), "{err}");
        let err = Endpoint::parse("http://127.0.0.1@evil.example").unwrap_err();
        assert!(matches!(err, EndpointError::Invalid { .. }), "{err}");

        let err = listener.accept().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn no_endpoint_is_configured_by_default() {
        let fake = FakeEnv::new();
        let err = ModelClient::from_vars(fake.env.vars.as_ref()).unwrap_err();
        assert!(matches!(err, ConfigError::NotConfigured { var } if var == URL_VAR));

        fake.vars.set(URL_VAR, "http://127.0.0.1:8081");
        let err = ModelClient::from_vars(fake.env.vars.as_ref()).unwrap_err();
        assert!(matches!(err, ConfigError::NotConfigured { var } if var == MODEL_VAR));

        fake.vars.set(MODEL_VAR, "tiny");
        let client = ModelClient::from_vars(fake.env.vars.as_ref()).unwrap();
        assert_eq!(client.model(), "tiny");
        assert_eq!(client.endpoint().addr().port(), 8081);
    }

    #[test]
    fn a_remote_url_in_the_environment_is_a_config_error() {
        let fake = FakeEnv::new();
        fake.vars.set(URL_VAR, "http://api.example.com");
        fake.vars.set(MODEL_VAR, "tiny");
        let err = ModelClient::from_vars(fake.env.vars.as_ref()).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Endpoint(EndpointError::NotLoopback { .. })
            ),
            "{err}"
        );
    }

    #[test]
    fn summarize_posts_the_session_text_and_returns_the_reply() {
        let (addr, server) = fake_server(completion("Fixed the flaky test."));
        let text = client_for(addr)
            .summarize(Some("Fix the flaky test"), Some("Test now passes"))
            .unwrap();
        assert_eq!(text, "Fixed the flaky test.");

        let captured = server.join().unwrap();
        assert_eq!(captured.request_line, "POST /v1/chat/completions HTTP/1.0");
        assert!(captured.headers.contains(&format!("host: {addr}")));
        assert!(captured.headers.contains("content-type: application/json"));
        assert_eq!(captured.body["model"], "test-model");
        assert_eq!(captured.body["stream"], false);
        let messages = captured.body["messages"].as_array().unwrap();
        assert_eq!(messages.first().unwrap()["role"], "system");
        let user = messages.last().unwrap();
        assert_eq!(user["role"], "user");
        let content = user["content"].as_str().unwrap();
        assert!(content.contains("Fix the flaky test"), "{content}");
        assert!(content.contains("Test now passes"), "{content}");
    }

    #[test]
    fn redirects_are_not_followed() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let location = format!("http://{}/elsewhere", target.local_addr().unwrap());
        let (addr, server) =
            fake_server(http("302 Found", &format!("Location: {location}\r\n"), ""));
        let err = client_for(addr).summarize(Some("t"), None).unwrap_err();
        assert!(matches!(err, ModelError::Redirect { status: 302 }), "{err}");
        server.join().unwrap();
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn error_statuses_and_garbage_answers_are_errors() {
        let (addr, server) = fake_server(http("500 Internal Server Error", "", "{}"));
        let err = client_for(addr).summarize(Some("t"), None).unwrap_err();
        assert!(matches!(err, ModelError::Status { status: 500 }), "{err}");
        server.join().unwrap();

        for body in [
            "not json",
            "{}",
            r#"{"choices": []}"#,
            r#"{"choices":[{"message":{"content":"  "}}]}"#,
        ] {
            let (addr, server) = fake_server(http("200 OK", "", body));
            let err = client_for(addr).summarize(Some("t"), None).unwrap_err();
            assert!(matches!(err, ModelError::BadResponse(_)), "{body}: {err}");
            server.join().unwrap();
        }
    }

    #[test]
    fn nothing_listening_is_an_io_error() {
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let err = client_for(addr).summarize(Some("t"), None).unwrap_err();
        assert!(matches!(err, ModelError::Io { .. }), "{err}");
        drop(TcpStream::connect(addr).err());
    }
}

mod storage {
    use claude_monitor::store::summaries::GENERATED_LABEL;

    use super::*;

    const PROMPT: &str = "Fix the flaky test";
    const LEFT_OFF: &str = "Test now passes";

    fn seeded() -> (Harness, Store, String) {
        let h = Harness::new();
        h.event("UserPromptSubmit", json!({"prompt": PROMPT}));
        h.event("Stop", json!({"last_assistant_message": LEFT_OFF}));
        let store = Store::open_in_dir(&h.state_dir, &StoreOptions::viewer()).unwrap();
        let id = h.session().id;
        (h, store, id)
    }

    #[test]
    fn model_text_is_stored_with_model_timestamp_and_label() {
        let (_h, store, id) = seeded();
        let made = store
            .insert_model_summary("session", &id, "A short summary.", "tiny-model", T0)
            .unwrap();
        assert_eq!(made.label(), Some(GENERATED_LABEL));
        assert_eq!(GENERATED_LABEL, "generated");

        let listed = store.list_summaries("session", &id).unwrap();
        assert_eq!(listed, vec![made]);
        let row = &listed[0];
        assert_eq!(row.body, "A short summary.");
        assert_eq!(row.model.as_deref(), Some("tiny-model"));
        assert_eq!(row.created_at, T0);
        assert!(row.model_written);
    }

    #[test]
    fn model_text_never_overwrites_the_verbatim_text() {
        let (h, store, id) = seeded();
        store
            .insert_model_summary("session", &id, "Rewritten by a model", "m", T0)
            .unwrap();
        store
            .insert_model_summary("session", &id, "Rewritten again", "m", T0)
            .unwrap();
        let session = h.session();
        assert_eq!(session.task_description.as_deref(), Some(PROMPT));
        assert_eq!(session.left_off.as_deref(), Some(LEFT_OFF));
    }

    #[test]
    fn summaries_accumulate_newest_first_and_stay_with_their_owner() {
        let (_h, store, id) = seeded();
        store
            .insert_model_summary("session", &id, "first", "m", "2026-10-12T10:00:00Z")
            .unwrap();
        store
            .insert_model_summary("session", &id, "second", "m", "2026-10-12T10:05:00Z")
            .unwrap();
        store
            .insert_model_summary("session", "other", "not mine", "m", "2026-10-12T10:09:00Z")
            .unwrap();
        store
            .insert_model_summary("project", &id, "wrong kind", "m", "2026-10-12T10:09:00Z")
            .unwrap();
        let bodies: Vec<String> = store
            .list_summaries("session", &id)
            .unwrap()
            .into_iter()
            .map(|s| s.body)
            .collect();
        assert_eq!(bodies, ["second", "first"]);
    }

    #[test]
    fn nothing_is_generated_without_an_explicit_request() {
        let (_h, store, id) = seeded();
        assert!(store.list_summaries("session", &id).unwrap().is_empty());
    }
}

mod summarize_cli {
    use std::net::TcpListener;
    use std::process::Command as Process;

    use claude_monitor::cli::{run_in, Command, Output};
    use claude_monitor::error::Error;
    use claude_monitor::summary::{MODEL_VAR, URL_VAR};
    use claude_monitor::testing::FakeEnv;

    use super::endpoint::{completion, fake_server};
    use super::*;

    const PROMPT: &str = "Fix the flaky test";
    const LEFT_OFF: &str = "Test now passes";

    fn seeded() -> (Harness, String) {
        let h = Harness::new();
        h.event("UserPromptSubmit", json!({"prompt": PROMPT}));
        h.event("Stop", json!({"last_assistant_message": LEFT_OFF}));
        let id = h.session().id;
        (h, id)
    }

    fn summarize(h: &Harness, fake: &FakeEnv, target: &str) -> Result<Output, Error> {
        run_in(
            Command::Summarize {
                target: target.to_owned(),
            },
            &fake.env,
            Some(&h.state_dir),
        )
    }

    fn stored(h: &Harness, id: &str) -> Vec<claude_monitor::store::summaries::Summary> {
        Store::open_in_dir(&h.state_dir, &StoreOptions::viewer())
            .unwrap()
            .list_summaries("session", id)
            .unwrap()
    }

    fn refused_code(result: Result<Output, Error>) -> &'static str {
        match result {
            Err(Error::Refused { code, .. }) => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn no_endpoint_configured_is_refused_with_exit_2_and_a_clear_message() {
        let (h, id) = seeded();
        let fake = FakeEnv::new();
        let err = summarize(&h, &fake, &id).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(matches!(
            err,
            Error::Refused {
                code: "summary_not_configured",
                ..
            }
        ));
        let message = err.to_string();
        assert!(
            message.contains(URL_VAR) && message.contains(MODEL_VAR),
            "{message}"
        );
        assert!(stored(&h, &id).is_empty());
    }

    #[test]
    fn a_remote_endpoint_is_refused_before_anything_is_sent() {
        let (h, id) = seeded();
        let fake = FakeEnv::new();
        fake.vars.set(URL_VAR, "http://api.example.com");
        fake.vars.set(MODEL_VAR, "m");
        let err = summarize(&h, &fake, &id).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(matches!(
            err,
            Error::Refused {
                code: "endpoint_not_loopback",
                ..
            }
        ));
        assert!(stored(&h, &id).is_empty());
    }

    #[test]
    fn summarizing_stores_labelled_model_text_and_leaves_verbatim_text_alone() {
        let (h, id) = seeded();
        let (addr, server) = fake_server(completion("Fixed the flaky test."));
        let fake = FakeEnv::new();
        fake.vars.set(URL_VAR, &format!("http://{addr}"));
        fake.vars.set(MODEL_VAR, "tiny-model");
        fake.clock.set("2026-10-12T11:00:00Z");

        let output = summarize(&h, &fake, &id[..8]).unwrap();
        let Output::Dual { text, json } = output else {
            panic!("expected dual output");
        };
        assert!(text.contains("Fixed the flaky test."), "{text}");
        assert!(text.contains("generated"), "{text}");
        assert!(text.contains("tiny-model"), "{text}");
        assert_eq!(json["label"], "generated");
        assert_eq!(json["model"], "tiny-model");
        assert_eq!(json["summary"], "Fixed the flaky test.");
        assert_eq!(json["session"], id);

        let rows = stored(&h, &id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body, "Fixed the flaky test.");
        assert_eq!(rows[0].model.as_deref(), Some("tiny-model"));
        assert_eq!(rows[0].created_at, "2026-10-12T11:00:00Z");
        let session = h.session();
        assert_eq!(session.task_description.as_deref(), Some(PROMPT));
        assert_eq!(session.left_off.as_deref(), Some(LEFT_OFF));

        let request = server.join().unwrap().body.to_string();
        assert!(
            request.contains(PROMPT) && request.contains(LEFT_OFF),
            "{request}"
        );
    }

    #[test]
    fn an_unknown_session_is_refused() {
        let (h, _id) = seeded();
        let fake = FakeEnv::new();
        fake.vars.set(URL_VAR, "http://127.0.0.1:9");
        fake.vars.set(MODEL_VAR, "m");
        assert_eq!(
            refused_code(summarize(&h, &fake, "nope")),
            "unknown_session"
        );
    }

    #[test]
    fn a_model_that_is_down_is_refused_and_stores_nothing() {
        let (h, id) = seeded();
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let fake = FakeEnv::new();
        fake.vars.set(URL_VAR, &format!("http://{addr}"));
        fake.vars.set(MODEL_VAR, "m");
        assert_eq!(refused_code(summarize(&h, &fake, &id)), "summary_failed");
        assert!(stored(&h, &id).is_empty());
    }

    #[test]
    fn nothing_is_summarized_unless_asked() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let fake = FakeEnv::new();
        fake.vars.set(
            URL_VAR,
            &format!("http://{}", listener.local_addr().unwrap()),
        );
        fake.vars.set(MODEL_VAR, "m");

        let (h, id) = seeded();
        for command in [
            Command::Show {
                session: id.clone(),
            },
            Command::BootCheck { banner: false },
        ] {
            run_in(command, &fake.env, Some(&h.state_dir)).ok();
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(stored(&h, &id).is_empty());
    }

    #[test]
    fn the_binary_exits_2_with_a_json_error_when_no_endpoint_is_configured() {
        let (h, id) = seeded();
        let out = Process::new(env!("CARGO_BIN_EXE_claude-monitor"))
            .args(["--json", "--no-refresh", "--state-dir"])
            .arg(&h.state_dir)
            .args(["summarize", &id])
            .env_remove(URL_VAR)
            .env_remove(MODEL_VAR)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains(URL_VAR));
    }
}
