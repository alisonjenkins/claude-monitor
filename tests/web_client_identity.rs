//! The persisted client identity `setup-browser` imports, so a re-run presents
//! the same certificate instead of minting a new one each time.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use claude_monitor::web::tls::{
    ensure, ensure_client_identity, fingerprint_sha256, peek_client_identity, TlsPaths,
};
use tempfile::TempDir;
use x509_parser::prelude::*;

const DAY: Duration = Duration::from_secs(86_400);

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_first_call_persists_a_private_cert_and_key() {
    let state = TempDir::new().unwrap();

    let identity = ensure_client_identity(state.path(), SystemTime::now()).unwrap();

    let paths = TlsPaths::in_state_dir(state.path());
    assert_eq!(mode(&paths.client_cert), 0o600);
    assert_eq!(mode(&paths.client_key), 0o600);
    let pem = fs::read_to_string(&paths.client_cert).unwrap();
    assert!(!pem.contains("PRIVATE KEY"));
    assert!(!identity.cert_der.is_empty());
}

#[test]
fn a_second_call_returns_the_same_certificate() {
    let state = TempDir::new().unwrap();
    let first = ensure_client_identity(state.path(), SystemTime::now()).unwrap();

    let second = ensure_client_identity(state.path(), SystemTime::now()).unwrap();

    assert_eq!(first.cert_der, second.cert_der);
    assert_eq!(*first.key_der, *second.key_der);
}

#[test]
fn a_certificate_inside_its_last_30_days_is_replaced() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    let first = ensure_client_identity(state.path(), now).unwrap();

    let later = ensure_client_identity(state.path(), now + DAY * 340).unwrap();

    assert_ne!(first.cert_der, later.cert_der);
}

#[test]
fn a_new_ca_invalidates_the_old_client_certificate() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    let first = ensure_client_identity(state.path(), now).unwrap();
    let paths = TlsPaths::in_state_dir(state.path());
    fs::remove_file(&paths.ca_cert).unwrap();
    fs::remove_file(&paths.ca_key).unwrap();
    ensure(state.path(), now).unwrap();

    let second = ensure_client_identity(state.path(), now).unwrap();

    assert_ne!(first.cert_der, second.cert_der);
    let ca_pem = fs::read(&paths.ca_cert).unwrap();
    let (_, ca_der) = x509_parser::pem::parse_x509_pem(&ca_pem).unwrap();
    let ca = ca_der.parse_x509().unwrap();
    let (_, client) = X509Certificate::from_der(&second.cert_der).unwrap();
    client.verify_signature(Some(ca.public_key())).unwrap();
}

#[test]
fn peek_reads_without_creating_anything() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    assert!(peek_client_identity(state.path(), now).is_none());
    assert!(!state.path().join("tls").exists(), "peek must not write");

    let made = ensure_client_identity(state.path(), now).unwrap();
    let peeked = peek_client_identity(state.path(), now).unwrap();

    assert_eq!(made.cert_der, peeked.cert_der);
    assert!(peek_client_identity(state.path(), now + DAY * 340).is_none());
}

#[test]
fn the_fingerprint_is_the_lowercase_hex_sha256() {
    assert_eq!(
        fingerprint_sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
