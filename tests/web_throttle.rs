//! The failure limiter behind 429: five failures a minute, globally.
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
use claude_monitor::web::throttle::{Throttle, DEFAULT_MAX_FAILURES, DEFAULT_WINDOW};

const SECOND: Duration = Duration::from_secs(1);

fn throttle() -> (Throttle, Arc<ManualTicks>) {
    let clock = Arc::new(ManualTicks::new());
    (
        Throttle::new(clock.clone(), DEFAULT_MAX_FAILURES, DEFAULT_WINDOW),
        clock,
    )
}

#[test]
fn the_default_is_five_failures_a_minute() {
    assert_eq!(DEFAULT_MAX_FAILURES, 5);
    assert_eq!(DEFAULT_WINDOW, Duration::from_secs(60));
}

#[test]
fn five_failures_are_tolerated_the_sixth_attempt_is_not() {
    let (throttle, _) = throttle();

    for _ in 0..5 {
        assert!(throttle.check().is_ok());
        throttle.record_failure();
    }

    assert!(throttle.check().is_err());
}

#[test]
fn checking_alone_never_counts() {
    let (throttle, _) = throttle();

    for _ in 0..50 {
        assert!(throttle.check().is_ok());
    }
}

#[test]
fn retry_after_counts_down_to_the_oldest_failure_leaving_the_window() {
    let (throttle, clock) = throttle();
    for _ in 0..5 {
        throttle.record_failure();
    }

    assert_eq!(throttle.check(), Err(Duration::from_secs(60)));
    clock.advance(45 * SECOND);
    assert_eq!(throttle.check(), Err(Duration::from_secs(15)));
}

#[test]
fn it_recovers_when_the_window_passes() {
    let (throttle, clock) = throttle();
    for _ in 0..5 {
        throttle.record_failure();
    }
    assert!(throttle.check().is_err());

    clock.advance(DEFAULT_WINDOW);

    assert!(throttle.check().is_ok());
}

#[test]
fn the_window_slides_one_failure_at_a_time() {
    let (throttle, clock) = throttle();
    throttle.record_failure();
    clock.advance(30 * SECOND);
    for _ in 0..4 {
        throttle.record_failure();
    }
    assert!(throttle.check().is_err());

    clock.advance(30 * SECOND);

    assert!(throttle.check().is_ok(), "the first failure is 60 s old");
    throttle.record_failure();
    assert!(throttle.check().is_err());
}

#[test]
fn a_flood_of_failures_does_not_grow_without_bound() {
    let (throttle, _) = throttle();

    for _ in 0..100_000 {
        throttle.record_failure();
    }

    assert!(throttle.check().is_err());
}
