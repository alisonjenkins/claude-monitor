//! The in-memory session table: random ids, per-session CSRF values, idle and
//! absolute expiry, bounded size.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;
use std::time::Duration;

use claude_monitor::web::clock::ManualTicks;
use claude_monitor::web::session::{Sessions, ABSOLUTE_EXPIRY, IDLE_EXPIRY, MAX_SESSIONS};

const HOUR: Duration = Duration::from_secs(3600);

fn table() -> (Sessions, Arc<ManualTicks>) {
    let clock = Arc::new(ManualTicks::new());
    (Sessions::new(clock.clone()), clock)
}

#[test]
fn an_id_is_256_bits_and_the_csrf_value_is_a_different_secret() {
    let (sessions, _) = table();

    let session = sessions.create().unwrap();

    assert_eq!(session.id.len(), 64);
    assert!(session.id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(session.csrf.len(), 64);
    assert_ne!(*session.id, *session.csrf);
}

#[test]
fn every_session_has_its_own_id_and_csrf_value() {
    let (sessions, _) = table();

    let a = sessions.create().unwrap();
    let b = sessions.create().unwrap();

    assert_ne!(*a.id, *b.id);
    assert_ne!(*a.csrf, *b.csrf);
}

#[test]
fn a_lookup_returns_the_csrf_value_of_that_session_only() {
    let (sessions, _) = table();
    let a = sessions.create().unwrap();
    let b = sessions.create().unwrap();

    assert_eq!(sessions.lookup(&a.id).as_deref(), Some(a.csrf.as_str()));
    assert_eq!(sessions.lookup(&b.id).as_deref(), Some(b.csrf.as_str()));
    assert_eq!(
        sessions.lookup(&a.csrf),
        None,
        "the csrf value is not an id"
    );
    assert_eq!(sessions.lookup("nope"), None);
    assert_eq!(sessions.lookup(""), None);
}

#[test]
fn idle_sessions_expire_after_8_hours_and_use_resets_the_clock() {
    assert_eq!(IDLE_EXPIRY, 8 * HOUR);
    let (sessions, clock) = table();
    let kept = sessions.create().unwrap();
    let idle = sessions.create().unwrap();

    clock.advance(IDLE_EXPIRY - Duration::from_secs(1));
    assert!(sessions.lookup(&kept.id).is_some(), "just under 8 h idle");
    clock.advance(Duration::from_secs(2));

    assert!(sessions.lookup(&kept.id).is_some(), "touched 2 s ago");
    assert!(sessions.lookup(&idle.id).is_none(), "idle over 8 h");
}

#[test]
fn a_session_dies_24_hours_after_login_however_busy() {
    assert_eq!(ABSOLUTE_EXPIRY, 24 * HOUR);
    let (sessions, clock) = table();
    let session = sessions.create().unwrap();

    for _ in 0..3 {
        clock.advance(7 * HOUR);
        assert!(sessions.lookup(&session.id).is_some());
    }
    clock.advance(4 * HOUR);

    assert!(
        sessions.lookup(&session.id).is_none(),
        "used 4 h ago, but 25 h old"
    );
}

#[test]
fn the_table_is_bounded_and_drops_the_oldest() {
    let (sessions, _) = table();
    let first = sessions.create().unwrap();
    let rest: Vec<_> = (0..MAX_SESSIONS)
        .map(|_| sessions.create().unwrap())
        .collect();

    assert!(sessions.lookup(&first.id).is_none(), "oldest was evicted");
    assert!(rest.iter().all(|s| sessions.lookup(&s.id).is_some()));
}
