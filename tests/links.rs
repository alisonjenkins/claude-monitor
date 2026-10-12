//! Links between sessions or projects and external graph nodes (T096, T100,
//! T101, FR-026, FR-028): rows and events, the `link`/`unlink` commands, and
//! `links check` against a fake `cortexiphan` executable on a temporary PATH.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use clap::Parser;
use claude_monitor::cli::{run_in, Cli, Command, Output, ProjectAction};
use claude_monitor::error::Error;
use claude_monitor::export::links::{
    check_links, remote_status, statuses_agree, Check, LinkCheck, Outcome, LOOKUP_TIMEOUT,
};
use claude_monitor::model::{Author, OwnerKind, Session, State};
use claude_monitor::store::links::NewLink;
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T11:00:00Z";

fn user() -> Author {
    Author::User {
        name: "ali".to_owned(),
    }
}

fn open(dir: &TempDir) -> Store {
    Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap()
}

fn new_link<'a>(kind: OwnerKind, owner: &'a str, node: &'a str) -> NewLink<'a> {
    NewLink {
        owner_kind: kind,
        owner_id: owner,
        node_id: node,
        doc_id: Some("7"),
        cortex_kind: "task",
    }
}

fn link_events(store: &Store) -> Vec<(String, String, Author)> {
    store
        .events_since(0, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind.starts_with("link."))
        .map(|e| (e.kind, e.at, e.author))
        .collect()
}

mod rows {
    use super::*;

    #[test]
    fn a_link_records_owner_node_kind_and_an_event_with_author_and_time() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);

        let added = store
            .add_link(&new_link(OwnerKind::Session, "s1", "42"), T0, &user())
            .unwrap();

        assert!(added.created);
        assert_eq!(added.link.owner_kind, OwnerKind::Session);
        assert_eq!(added.link.owner_id, "s1");
        assert_eq!(added.link.cortex_node_id, "42");
        assert_eq!(added.link.cortex_doc_id.as_deref(), Some("7"));
        assert_eq!(added.link.cortex_kind, "task");
        assert_eq!(added.link.last_agreed_at, None);
        assert_eq!(added.link.last_remote_status, None);
        assert_eq!(
            store.links_for(OwnerKind::Session, "s1").unwrap(),
            vec![added.link]
        );
        assert_eq!(
            link_events(&store),
            vec![("link.added".to_owned(), T0.to_owned(), user())]
        );
    }

    #[test]
    fn linking_the_same_pair_again_changes_nothing_and_adds_no_event() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        let first = store
            .add_link(&new_link(OwnerKind::Project, "p1", "42"), T0, &user())
            .unwrap();

        let again = store
            .add_link(&new_link(OwnerKind::Project, "p1", "42"), T1, &user())
            .unwrap();

        assert!(!again.created);
        assert_eq!(again.link, first.link);
        assert_eq!(store.all_links().unwrap().len(), 1);
        assert_eq!(link_events(&store).len(), 1);
    }

    #[test]
    fn one_owner_can_have_several_nodes_and_one_node_several_owners() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        for (kind, owner, node) in [
            (OwnerKind::Session, "s1", "1"),
            (OwnerKind::Session, "s1", "2"),
            (OwnerKind::Project, "p1", "1"),
        ] {
            store
                .add_link(&new_link(kind, owner, node), T0, &user())
                .unwrap();
        }

        assert_eq!(store.links_for(OwnerKind::Session, "s1").unwrap().len(), 2);
        assert_eq!(store.links_for(OwnerKind::Project, "p1").unwrap().len(), 1);
        assert_eq!(store.all_links().unwrap().len(), 3);
    }

    #[test]
    fn unlink_removes_only_that_pair_and_records_an_event() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);
        store
            .add_link(&new_link(OwnerKind::Session, "s1", "1"), T0, &user())
            .unwrap();
        store
            .add_link(&new_link(OwnerKind::Session, "s1", "2"), T0, &user())
            .unwrap();

        assert!(store
            .remove_link(OwnerKind::Session, "s1", "1", T1, &user())
            .unwrap());

        let left = store.links_for(OwnerKind::Session, "s1").unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].cortex_node_id, "2");
        let kinds: Vec<_> = link_events(&store).into_iter().map(|e| e.0).collect();
        assert_eq!(kinds, ["link.added", "link.added", "link.removed"]);
    }

    #[test]
    fn unlinking_a_pair_that_is_not_linked_is_a_quiet_no_op() {
        let dir = TempDir::new().unwrap();
        let store = open(&dir);

        assert!(!store
            .remove_link(OwnerKind::Session, "s1", "1", T1, &user())
            .unwrap());

        assert!(link_events(&store).is_empty());
    }
}

struct Fixture {
    state: TempDir,
    fake: FakeEnv,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let state = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("USER", "ali");
        let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        let session = Session::new("s1", "claude", "agent-s1", "/work/s1", T0);
        store.upsert_session(&session).unwrap();
        let fixture = Self { state, fake, store };
        fixture
            .run(Command::Project {
                action: ProjectAction::Add,
                name: "p1".to_owned(),
                value: None,
            })
            .unwrap();
        fixture
    }

    fn run(&self, command: Command) -> Result<Output, Error> {
        run_in(command, &self.fake.env, Some(self.state.path()))
    }

    fn link(&self, owner: &str, node: &str) -> Result<Output, Error> {
        self.run(Command::Link {
            owner: owner.to_owned(),
            cortex: node.to_owned(),
            kind: "task".to_owned(),
            doc: Some("7".to_owned()),
        })
    }

    fn unlink(&self, owner: &str, node: &str) -> Result<Output, Error> {
        self.run(Command::Unlink {
            owner: owner.to_owned(),
            cortex: node.to_owned(),
        })
    }
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&output.render(true)).unwrap()
}

mod commands {
    use super::*;

    #[test]
    fn link_and_unlink_parse_into_commands() {
        let cli = Cli::try_parse_from(["claude-monitor", "link", "s1", "42"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Link {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
                kind: "task".to_owned(),
                doc: None,
            }
        );
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "link",
            "p1",
            "42",
            "--kind",
            "memory",
            "--doc",
            "7",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::Link {
                owner: "p1".to_owned(),
                cortex: "42".to_owned(),
                kind: "memory".to_owned(),
                doc: Some("7".to_owned()),
            }
        );
        let cli = Cli::try_parse_from(["claude-monitor", "unlink", "s1", "42"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Unlink {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
            }
        );
        assert!(
            Cli::try_parse_from(["claude-monitor", "link", "s1", "42", "--kind", "x"]).is_err()
        );
    }

    #[test]
    fn linking_a_session_stores_the_row_with_the_user_as_author() {
        let fix = Fixture::new();

        let output = fix.link("s1", "42").unwrap();

        let value = json(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["created"], true);
        assert_eq!(value["link"]["owner"]["kind"], "session");
        assert_eq!(value["link"]["owner"]["id"], "s1");
        assert_eq!(value["link"]["node_id"], "42");
        let links = fix.store.links_for(OwnerKind::Session, "s1").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].cortex_doc_id.as_deref(), Some("7"));
        assert_eq!(
            link_events(&fix.store),
            vec![(
                "link.added".to_owned(),
                "2026-10-12T00:00:00Z".to_owned(),
                user()
            )]
        );
    }

    #[test]
    fn linking_a_project_by_name_and_linking_twice_is_idempotent() {
        let fix = Fixture::new();
        fix.link("p1", "42").unwrap();

        let again = fix.link("p1", "42").unwrap();

        assert_eq!(json(&again)["created"], false);
        let project = fix.store.project_by_name("p1").unwrap().unwrap();
        assert_eq!(
            fix.store
                .links_for(OwnerKind::Project, &project.id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(link_events(&fix.store).len(), 1);
    }

    #[test]
    fn an_unknown_owner_is_refused_and_nothing_is_written() {
        let fix = Fixture::new();

        let err = fix.link("nobody", "42").unwrap_err();

        assert!(matches!(err, Error::Refused { .. }), "{err:?}");
        assert!(fix.store.all_links().unwrap().is_empty());
    }

    #[test]
    fn a_node_id_that_is_not_a_number_is_a_usage_error() {
        let fix = Fixture::new();

        let err = fix.link("s1", "not-a-number").unwrap_err();

        assert!(matches!(err, Error::Usage(_)), "{err:?}");
        let err = fix
            .run(Command::Link {
                owner: "s1".to_owned(),
                cortex: "42".to_owned(),
                kind: "task".to_owned(),
                doc: Some("x".to_owned()),
            })
            .unwrap_err();
        assert!(matches!(err, Error::Usage(_)), "{err:?}");
        assert!(fix.store.all_links().unwrap().is_empty());
    }

    #[test]
    fn unlink_removes_the_row_and_a_second_unlink_reports_no_change() {
        let fix = Fixture::new();
        fix.link("s1", "42").unwrap();

        let first = fix.unlink("s1", "42").unwrap();
        let second = fix.unlink("s1", "42").unwrap();

        assert_eq!(json(&first)["changed"], true);
        assert_eq!(json(&second)["changed"], false);
        assert!(fix.store.all_links().unwrap().is_empty());
    }
}

/// A `cortexiphan` stand-in on a private PATH. For `--id N` it logs its
/// arguments, then prints `out.N` (exit 0), exits 1 when `fail.N` exists, or
/// sleeps when `slow.N` exists.
struct FakeClient {
    dir: TempDir,
}

impl FakeClient {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let script = format!(
            r#"#!/bin/sh
echo "$*" >> "{dir}/calls.log"
id=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--id" ]; then id="$arg"; fi
  prev="$arg"
done
[ -e "{dir}/slow.$id" ] && exec sleep 5
[ -e "{dir}/fail.$id" ] && exit 1
[ -e "{dir}/out.$id" ] && exec cat "{dir}/out.$id"
exit 3
"#,
            dir = dir.path().display()
        );
        let program = dir.path().join("cortexiphan");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn reply(&self, id: &str, status: &str) {
        let body = format!(r#"{{"node":{{"id":{id},"fields":{{"status":"{status}"}}}}}}"#);
        std::fs::write(self.path().join(format!("out.{id}")), body).unwrap();
    }

    fn make(&self, prefix: &str, id: &str) {
        std::fs::write(self.path().join(format!("{prefix}.{id}")), "").unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path().join("calls.log"))
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

fn configured(fix: &Fixture, client: &FakeClient) {
    fix.fake.vars.set("PATH", client.path().to_str().unwrap());
    fix.fake
        .vars
        .set("CORTEXIPHAN_SERVER", "http://127.0.0.1:1");
    fix.fake.vars.set("CORTEXIPHAN_TOKEN", "secret-token");
}

fn put_session(fix: &Fixture, id: &str, state: State, done: bool) {
    let mut session = Session::new(id, "claude", &format!("agent-{id}"), "/work", T0);
    session.state = state;
    session.done_at = done.then(|| T0.to_owned());
    fix.store.upsert_session(&session).unwrap();
}

fn link_session(fix: &Fixture, id: &str, node: &str) {
    fix.link(id, node).unwrap();
}

fn checked(fix: &Fixture, timeout: Duration) -> Vec<LinkCheck> {
    match check_links(&fix.store, fix.fake.vars.as_ref(), timeout).unwrap() {
        Check::Checked(checks) => checks,
        Check::Skipped { reason } => panic!("skipped: {reason}"),
    }
}

fn outcome_for<'a>(checks: &'a [LinkCheck], owner: &str) -> &'a Outcome {
    &checks
        .iter()
        .find(|c| c.link.owner_id == owner)
        .unwrap()
        .outcome
}

mod check {
    use super::*;

    #[test]
    fn remote_status_is_found_wherever_the_reply_puts_it() {
        assert_eq!(
            remote_status(r#"{"node":{"fields":{"status":"done"}}}"#).as_deref(),
            Some("done")
        );
        assert_eq!(
            remote_status(r#"{"status":{"custom":"review"}}"#).as_deref(),
            Some("review")
        );
        assert_eq!(remote_status(r#"{"node":{"title":"x"}}"#), None);
        assert_eq!(remote_status("not json"), None);
    }

    #[test]
    fn statuses_agree_when_they_mean_the_same_thing() {
        for (local, remote) in [
            ("working", "doing"),
            ("idle", "Doing"),
            ("finished", "done"),
            ("done", "done"),
            ("interrupted", "blocked"),
            ("active", "doing"),
            ("review", "review"),
        ] {
            assert!(statuses_agree(local, remote), "{local} vs {remote}");
        }
        for (local, remote) in [
            ("interrupted", "done"),
            ("working", "todo"),
            ("done", "doing"),
            ("unknown", "todo"),
        ] {
            assert!(!statuses_agree(local, remote), "{local} vs {remote}");
        }
    }

    #[test]
    fn without_server_and_token_the_check_is_skipped_and_runs_nothing() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        put_session(&fix, "s2", State::Working, false);
        link_session(&fix, "s2", "41");
        fix.fake.vars.set("PATH", client.path().to_str().unwrap());

        let result = check_links(&fix.store, fix.fake.vars.as_ref(), LOOKUP_TIMEOUT).unwrap();
        let Check::Skipped { reason } = result else {
            panic!("expected a skip, got {result:?}");
        };
        assert!(reason.contains("CORTEXIPHAN_SERVER"), "{reason}");
        assert!(reason.contains("CORTEXIPHAN_TOKEN"), "{reason}");

        fix.fake
            .vars
            .set("CORTEXIPHAN_SERVER", "http://127.0.0.1:1");
        let Check::Skipped { reason } =
            check_links(&fix.store, fix.fake.vars.as_ref(), LOOKUP_TIMEOUT).unwrap()
        else {
            panic!("a token is still missing");
        };
        assert!(reason.contains("CORTEXIPHAN_TOKEN"), "{reason}");
        assert!(!reason.contains("CORTEXIPHAN_SERVER"), "{reason}");
        assert!(client.calls().is_empty());
    }

    #[test]
    fn matching_statuses_agree() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        put_session(&fix, "s2", State::Working, false);
        put_session(&fix, "s3", State::Finished, true);
        link_session(&fix, "s2", "41");
        link_session(&fix, "s3", "42");
        client.reply("41", "doing");
        client.reply("42", "done");

        let checks = checked(&fix, LOOKUP_TIMEOUT);

        assert_eq!(checks.len(), 2);
        assert_eq!(
            outcome_for(&checks, "s2"),
            &Outcome::Agree {
                status: "working".to_owned()
            }
        );
        assert_eq!(
            outcome_for(&checks, "s3"),
            &Outcome::Agree {
                status: "done".to_owned()
            }
        );
    }

    #[test]
    fn a_project_is_compared_by_its_status() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        fix.link("p1", "50").unwrap();
        client.reply("50", "done");

        let checks = checked(&fix, LOOKUP_TIMEOUT);

        let project = fix.store.project_by_name("p1").unwrap().unwrap();
        assert_eq!(
            outcome_for(&checks, &project.id),
            &Outcome::Disagree {
                local: "active".to_owned(),
                remote: "done".to_owned()
            }
        );
    }

    #[test]
    fn a_disagreement_reports_both_values() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        put_session(&fix, "s2", State::Interrupted, false);
        link_session(&fix, "s2", "41");
        client.reply("41", "done");

        let checks = checked(&fix, LOOKUP_TIMEOUT);

        assert_eq!(
            outcome_for(&checks, "s2"),
            &Outcome::Disagree {
                local: "interrupted".to_owned(),
                remote: "done".to_owned()
            }
        );
        assert_eq!(checks[0].local_status.as_deref(), Some("interrupted"));
    }

    #[test]
    fn a_failing_slow_or_missing_client_is_unreachable_not_an_error() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        for (id, node) in [("s2", "41"), ("s3", "42"), ("s4", "43")] {
            put_session(&fix, id, State::Working, false);
            link_session(&fix, id, node);
        }
        client.make("fail", "41");
        client.make("slow", "42");

        let checks = checked(&fix, Duration::from_millis(300));

        assert!(matches!(
            outcome_for(&checks, "s2"),
            Outcome::Unreachable { .. }
        ));
        let Outcome::Unreachable { reason } = outcome_for(&checks, "s3") else {
            panic!("a slow client is unreachable");
        };
        assert!(reason.contains("timed out"), "{reason}");
        assert!(
            matches!(outcome_for(&checks, "s4"), Outcome::Unreachable { .. }),
            "no out file makes the fake exit 3"
        );

        let empty = TempDir::new().unwrap();
        fix.fake.vars.set("PATH", empty.path().to_str().unwrap());
        let checks = checked(&fix, LOOKUP_TIMEOUT);
        assert!(checks
            .iter()
            .all(|c| matches!(&c.outcome, Outcome::Unreachable { reason } if reason.contains("cortexiphan"))));
    }

    #[test]
    fn a_link_without_a_doc_id_is_unreachable_and_runs_nothing() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        put_session(&fix, "s2", State::Working, false);
        fix.run(Command::Link {
            owner: "s2".to_owned(),
            cortex: "41".to_owned(),
            kind: "task".to_owned(),
            doc: None,
        })
        .unwrap();

        let checks = checked(&fix, LOOKUP_TIMEOUT);

        let Outcome::Unreachable { reason } = outcome_for(&checks, "s2") else {
            panic!("expected unreachable");
        };
        assert!(reason.contains("doc"), "{reason}");
        assert!(client.calls().is_empty());
    }

    #[test]
    fn a_deleted_owner_and_a_reply_without_status_are_reported_as_such() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        link_session(&fix, "s1", "41");
        fix.store
            .add_link(&new_link(OwnerKind::Session, "gone", "42"), T0, &user())
            .unwrap();
        std::fs::write(client.path().join("out.41"), r#"{"node":{"title":"x"}}"#).unwrap();
        client.reply("42", "done");

        let checks = checked(&fix, LOOKUP_TIMEOUT);

        assert_eq!(outcome_for(&checks, "s1"), &Outcome::NoRemoteStatus);
        assert_eq!(outcome_for(&checks, "gone"), &Outcome::OwnerMissing);
    }

    #[test]
    fn a_check_writes_nothing_and_only_runs_read_subcommands() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        put_session(&fix, "s2", State::Interrupted, false);
        link_session(&fix, "s2", "41");
        link_session(&fix, "s1", "42");
        client.reply("41", "done");
        client.make("fail", "42");
        let links_before = fix.store.all_links().unwrap();
        let seq_before = fix.store.last_seq().unwrap();
        let sessions_before = fix.store.list_sessions().unwrap();
        let projects_before = fix.store.list_projects().unwrap();

        checked(&fix, LOOKUP_TIMEOUT);
        checked(&fix, LOOKUP_TIMEOUT);

        assert_eq!(fix.store.all_links().unwrap(), links_before);
        assert_eq!(fix.store.last_seq().unwrap(), seq_before);
        assert_eq!(fix.store.list_sessions().unwrap(), sessions_before);
        assert_eq!(fix.store.list_projects().unwrap(), projects_before);
        let calls = client.calls();
        assert_eq!(calls.len(), 4);
        for call in &calls {
            assert!(
                call.starts_with("--json node get --doc 7 --id "),
                "unexpected client call: {call}"
            );
        }
    }

    #[test]
    fn the_client_is_given_the_server_and_token_from_the_environment() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        let program = client.path().join("cortexiphan");
        let script = std::fs::read_to_string(&program).unwrap();
        let script = script.replacen(
            "id=\"\"",
            &format!(
                "echo \"$CORTEXIPHAN_SERVER $CORTEXIPHAN_TOKEN\" > \"{}/env.log\"\nid=\"\"",
                client.path().display()
            ),
            1,
        );
        std::fs::write(&program, script).unwrap();
        put_session(&fix, "s2", State::Working, false);
        link_session(&fix, "s2", "41");
        client.reply("41", "doing");

        checked(&fix, LOOKUP_TIMEOUT);

        let seen = std::fs::read_to_string(client.path().join("env.log")).unwrap();
        assert_eq!(seen.trim(), "http://127.0.0.1:1 secret-token");
    }
}

mod links_check_command {
    use super::*;

    fn links_check(fix: &Fixture) -> Result<Output, Error> {
        fix.run(Command::LinksCheck)
    }

    #[test]
    fn it_parses_as_links_check() {
        let cli = Cli::try_parse_from(["claude-monitor", "links", "check"]).unwrap();
        assert_eq!(cli.command(), Command::LinksCheck);
        assert!(Cli::try_parse_from(["claude-monitor", "links"]).is_err());
    }

    #[test]
    fn without_settings_it_says_it_was_skipped_and_why_and_still_succeeds() {
        let fix = Fixture::new();
        fix.link("s1", "42").unwrap();

        let output = links_check(&fix).unwrap();

        let text = output.render(false);
        assert!(text.contains("skipped"), "{text}");
        assert!(text.contains("CORTEXIPHAN_SERVER"), "{text}");
        let value = json(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["checked"], false);
        assert!(value["skipped"]
            .as_str()
            .unwrap()
            .contains("CORTEXIPHAN_TOKEN"));
    }

    #[test]
    fn it_reports_agreement_disagreement_and_unreachable_links_with_exit_0() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);
        put_session(&fix, "s2", State::Working, false);
        put_session(&fix, "s3", State::Interrupted, false);
        put_session(&fix, "s4", State::Working, false);
        link_session(&fix, "s2", "41");
        link_session(&fix, "s3", "42");
        link_session(&fix, "s4", "43");
        client.reply("41", "doing");
        client.reply("42", "done");
        client.make("fail", "43");

        let output = links_check(&fix).unwrap();

        let value = json(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["checked"], true);
        assert_eq!(value["summary"]["agree"], 1);
        assert_eq!(value["summary"]["disagree"], 1);
        assert_eq!(value["summary"]["unreachable"], 1);
        let by_owner = |owner: &str| {
            value["links"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["owner"]["id"] == owner)
                .unwrap()
                .clone()
        };
        let disagreeing = by_owner("s3");
        assert_eq!(disagreeing["outcome"], "disagree");
        assert_eq!(disagreeing["local_status"], "interrupted");
        assert_eq!(disagreeing["remote_status"], "done");
        assert_eq!(by_owner("s2")["outcome"], "agree");
        assert_eq!(by_owner("s4")["outcome"], "unreachable");
        assert!(by_owner("s4")["reason"].is_string());
        let text = output.render(false);
        assert!(text.contains("interrupted"), "{text}");
        assert!(text.contains("done"), "{text}");
        assert!(!text.contains("secret-token"), "{text}");
    }

    #[test]
    fn with_no_links_it_says_so() {
        let fix = Fixture::new();
        let client = FakeClient::new();
        configured(&fix, &client);

        let output = links_check(&fix).unwrap();

        assert!(output.render(false).contains("no links"));
        assert_eq!(json(&output)["links"].as_array().unwrap().len(), 0);
    }
}
