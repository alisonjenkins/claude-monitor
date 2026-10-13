//! Single-use login codes: valid once, for 5 minutes, in memory only.
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
use claude_monitor::web::login::{LoginCodes, LOGIN_CODE_TTL};

fn store() -> (LoginCodes, Arc<ManualTicks>) {
    let clock = Arc::new(ManualTicks::new());
    (LoginCodes::new(clock.clone()), clock)
}

#[test]
fn a_code_is_256_bits_of_hex() {
    let (codes, _) = store();

    let code = codes.issue().unwrap();

    assert_eq!(code.len(), 64);
    assert!(code.chars().all(|c| c.is_ascii_hexdigit()), "{}", *code);
}

#[test]
fn two_codes_differ() {
    let (codes, _) = store();

    assert_ne!(*codes.issue().unwrap(), *codes.issue().unwrap());
}

#[test]
fn a_code_works_exactly_once() {
    let (codes, _) = store();
    let code = codes.issue().unwrap();

    assert!(codes.redeem(&code), "first use");
    assert!(!codes.redeem(&code), "replay");
}

#[test]
fn a_wrong_code_is_refused_and_leaves_the_real_one_alone() {
    let (codes, _) = store();
    let code = codes.issue().unwrap();
    let mut wrong = code.to_string();
    wrong.replace_range(0..1, if code.starts_with('a') { "b" } else { "a" });

    assert!(!codes.redeem(&wrong));
    assert!(!codes.redeem(""));
    assert!(!codes.redeem(&code[..63]));
    assert!(codes.redeem(&code));
}

#[test]
fn a_code_lives_five_minutes() {
    assert_eq!(LOGIN_CODE_TTL, Duration::from_secs(300));
    let (codes, clock) = store();
    let fresh = codes.issue().unwrap();
    let stale = codes.issue().unwrap();

    clock.advance(LOGIN_CODE_TTL - Duration::from_secs(1));
    assert!(codes.redeem(&fresh), "one second before expiry");
    clock.advance(Duration::from_secs(1));
    assert!(!codes.redeem(&stale), "at expiry");
}

#[test]
fn codes_issued_later_expire_later() {
    let (codes, clock) = store();
    let first = codes.issue().unwrap();
    clock.advance(Duration::from_secs(200));
    let second = codes.issue().unwrap();
    clock.advance(Duration::from_secs(150));

    assert!(!codes.redeem(&first));
    assert!(codes.redeem(&second));
}

#[test]
fn only_a_handful_of_codes_stay_valid_at_once() {
    let (codes, _) = store();
    let issued: Vec<_> = (0..20).map(|_| codes.issue().unwrap()).collect();

    let valid = issued.iter().filter(|c| codes.redeem(c)).count();

    assert!(valid <= 8, "{valid} codes were valid together");
    assert!(
        codes.redeem(&codes.issue().unwrap()),
        "a new code still works"
    );
    assert!(!issued.first().is_some_and(|c| codes.redeem(c)));
}

#[test]
fn racing_redemptions_of_one_code_yield_one_winner() {
    let (codes, _) = store();
    let codes = Arc::new(codes);
    let code = codes.issue().unwrap().to_string();

    let winners: usize = (0..16)
        .map(|_| {
            let (codes, code) = (Arc::clone(&codes), code.clone());
            std::thread::spawn(move || usize::from(codes.redeem(&code)))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .sum();

    assert_eq!(winners, 1);
}
