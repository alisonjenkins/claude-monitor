//! One corrupt record never breaks the rest (FR-021).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use claude_monitor::model::{Session, State};
use claude_monitor::reconcile::{reconcile, Scope};
use claude_monitor::store::queries::{Flag, OverviewQuery};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use tempfile::TempDir;

const NOW: &str = "2026-10-12T12:00:00Z";
const PID: u32 = 100;
const PID_START: u64 = 5;

struct Fixture {
    _dir: TempDir,
    store: Store,
    fake: FakeEnv,
}

impl Fixture {
    /// A good and a corrupt session, both recorded as working under a process
    /// that has since died.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
        let fake = FakeEnv::new();
        for (id, last_activity_at) in [
            ("good", "2026-10-12T11:00:00Z"),
            ("bad", "2026-10-12T10:00:00Z"),
        ] {
            let mut session = Session::new(
                id,
                "claude",
                &format!("agent-{id}"),
                &format!("/work/{id}"),
                "2026-10-01T00:00:00Z",
            );
            session.state = State::Working;
            session.last_activity_at = last_activity_at.to_owned();
            session.pid = Some(PID);
            session.pid_start = Some(PID_START);
            session.boot_id = Some("boot-1".to_owned());
            store.upsert_session(&session).unwrap();
        }
        let conn = store.connection();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
            .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
            .unwrap();
        Self {
            _dir: dir,
            store,
            fake,
        }
    }
}

#[test]
fn corrupt_row_is_a_degraded_entry_and_the_rest_of_the_list_works() {
    let fx = Fixture::new();

    let overview = fx.store.overview(&OverviewQuery::default(), NOW).unwrap();

    let entries: Vec<_> = overview.sections.iter().flat_map(|s| &s.sessions).collect();
    assert_eq!(entries.len(), 2);
    let bad = entries.iter().find(|e| e.id == "bad").unwrap();
    assert!(bad.flags.contains(&Flag::Degraded));
    assert!(bad.error.as_deref().unwrap().contains("bogus"));
    let good = entries.iter().find(|e| e.id == "good").unwrap();
    assert!(good.error.is_none());
    assert!(!good.flags.contains(&Flag::Degraded));
}

#[test]
#[ignore = "T052"]
fn reconcile_skips_the_corrupt_row_reports_it_and_still_fixes_the_rest() {
    let fx = Fixture::new();

    let report = reconcile(&fx.store, &fx.fake.env, Scope::All, NOW).unwrap();

    assert_eq!(report.degraded.len(), 1);
    assert_eq!(report.degraded[0].id, "bad");
    assert!(report.degraded[0].error.contains("bogus"));
    assert_eq!((report.checked, report.interrupted), (1, 1));
    assert_eq!(
        fx.store.get_session("good").unwrap().unwrap().state,
        State::Interrupted
    );
    let raw_state: String = fx
        .store
        .connection()
        .query_row("SELECT state FROM sessions WHERE id = 'bad'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(raw_state, "bogus");
}
