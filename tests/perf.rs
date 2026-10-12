//! Scale (SC-007): 500 historical sessions open in under 2 s with the
//! default sections. Measures the overview query, not process start.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::time::{Duration, Instant};

use claude_monitor::model::{Session, State};
use claude_monitor::store::queries::OverviewQuery;
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const SESSIONS: usize = 500;
const BUDGET: Duration = Duration::from_secs(2);

const STATES: [State; 5] = [
    State::Finished,
    State::Interrupted,
    State::Idle,
    State::Working,
    State::Unknown,
];

fn seeded_store(dir: &TempDir) -> Store {
    let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
    for n in 0..SESSIONS {
        let id = format!("s{n:04}");
        let mut session = Session::new(
            &id,
            "claude",
            &format!("agent-{id}"),
            &format!("/work/{id}"),
            "2026-01-01T00:00:00Z",
        );
        session.state = STATES[n % STATES.len()];
        session.last_activity_at = format!("2026-{:02}-{:02}T10:00:00Z", 1 + n % 9, 1 + n % 28);
        session.task_description = Some(format!("historical task {n}"));
        session.dismissed = n % 17 == 0;
        store.upsert_session(&session).unwrap();
    }
    store
}

#[test]
fn five_hundred_sessions_open_the_default_overview_within_two_seconds() {
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir);

    let started = Instant::now();
    let overview = store.overview(&OverviewQuery::default()).unwrap();
    let elapsed = started.elapsed();

    let listed: usize = overview.sections.iter().map(|s| s.sessions.len()).sum();
    let hidden = (0..SESSIONS).filter(|n| n % 17 == 0).count();
    assert_eq!(
        listed,
        SESSIONS - hidden,
        "every undismissed session is listed"
    );
    assert!(
        elapsed < BUDGET,
        "the overview of {SESSIONS} sessions took {elapsed:?}, budget {BUDGET:?}"
    );
}
