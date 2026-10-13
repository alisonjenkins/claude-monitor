//! The per-install CA and the loopback leaf `serve` presents.
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

use claude_monitor::web::tls::{ensure, TlsPaths};
use tempfile::TempDir;
use x509_parser::prelude::*;

const DAY: Duration = Duration::from_secs(86_400);

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn der_of(pem_path: &std::path::Path) -> Vec<u8> {
    let pem = fs::read_to_string(pem_path).unwrap();
    let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
    parsed.contents
}

fn not_after(pem_path: &std::path::Path) -> i64 {
    let der = der_of(pem_path);
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    cert.validity().not_after.timestamp()
}

fn epoch(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn first_run_writes_all_four_files_private() {
    let state = TempDir::new().unwrap();

    let paths = ensure(state.path(), SystemTime::now()).unwrap();

    assert_eq!(mode(&paths.dir), 0o700, "tls dir");
    for file in [
        &paths.ca_cert,
        &paths.ca_key,
        &paths.server_cert,
        &paths.server_key,
    ] {
        assert_eq!(mode(file), 0o600, "{}", file.display());
    }
    assert_eq!(paths, TlsPaths::in_state_dir(state.path()));
    assert_eq!(paths.dir, state.path().join("tls"));
}

#[test]
fn an_existing_loose_dir_is_tightened() {
    let state = TempDir::new().unwrap();
    let dir = state.path().join("tls");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    ensure(state.path(), SystemTime::now()).unwrap();

    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn the_leaf_names_loopback_and_localhost() {
    let state = TempDir::new().unwrap();
    let paths = ensure(state.path(), SystemTime::now()).unwrap();

    let der = der_of(&paths.server_cert);
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    let san = cert.subject_alternative_name().unwrap().unwrap();
    let mut seen_dns = false;
    let mut ips = Vec::new();
    for name in &san.value.general_names {
        match name {
            GeneralName::DNSName(n) if *n == "localhost" => seen_dns = true,
            GeneralName::IPAddress(bytes) => ips.push(bytes.to_vec()),
            other => panic!("unexpected SAN {other:?}"),
        }
    }
    assert!(seen_dns, "localhost missing");
    assert!(ips.contains(&vec![127, 0, 0, 1]), "127.0.0.1 missing");
    let mut v6 = vec![0u8; 15];
    v6.push(1);
    assert!(ips.contains(&v6), "::1 missing");
}

#[test]
fn validity_is_90_days_for_the_leaf_and_5_years_for_the_ca() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    let paths = ensure(state.path(), now).unwrap();

    let leaf_days = (not_after(&paths.server_cert) - epoch(now)) / 86_400;
    let ca_days = (not_after(&paths.ca_cert) - epoch(now)) / 86_400;
    assert!((89..=91).contains(&leaf_days), "leaf {leaf_days} days");
    assert!((1824..=1827).contains(&ca_days), "ca {ca_days} days");
}

#[test]
fn the_ca_is_name_constrained_to_loopback() {
    let state = TempDir::new().unwrap();
    let paths = ensure(state.path(), SystemTime::now()).unwrap();

    let der = der_of(&paths.ca_cert);
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    assert!(cert.is_ca());
    let nc = cert
        .name_constraints()
        .unwrap()
        .expect("name constraints")
        .value;
    let permitted = nc.permitted_subtrees.as_ref().expect("permitted subtrees");
    assert!(
        permitted
            .iter()
            .any(|s| matches!(s.base, GeneralName::DNSName("localhost"))),
        "{permitted:?}"
    );
    assert!(
        permitted
            .iter()
            .any(|s| matches!(s.base, GeneralName::IPAddress(_))),
        "{permitted:?}"
    );
}

#[test]
fn a_second_run_reuses_everything() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    let paths = ensure(state.path(), now).unwrap();
    let before: Vec<Vec<u8>> = [
        &paths.ca_cert,
        &paths.ca_key,
        &paths.server_cert,
        &paths.server_key,
    ]
    .iter()
    .map(|p| fs::read(p).unwrap())
    .collect();

    ensure(state.path(), now + 70 * DAY).unwrap();

    let after: Vec<Vec<u8>> = [
        &paths.ca_cert,
        &paths.ca_key,
        &paths.server_cert,
        &paths.server_key,
    ]
    .iter()
    .map(|p| fs::read(p).unwrap())
    .collect();
    assert_eq!(before, after, "70 days in, 20 remain: nothing is renewed");
}

#[test]
fn the_leaf_is_regenerated_inside_the_last_14_days_but_the_ca_is_kept() {
    let state = TempDir::new().unwrap();
    let now = SystemTime::now();
    let paths = ensure(state.path(), now).unwrap();
    let ca_before = fs::read(&paths.ca_cert).unwrap();
    let leaf_before = fs::read(&paths.server_cert).unwrap();
    let later = now + 80 * DAY;

    ensure(state.path(), later).unwrap();

    assert_eq!(fs::read(&paths.ca_cert).unwrap(), ca_before, "CA unchanged");
    assert_ne!(fs::read(&paths.server_cert).unwrap(), leaf_before);
    let renewed_days = (not_after(&paths.server_cert) - epoch(later)) / 86_400;
    assert!((89..=91).contains(&renewed_days), "{renewed_days}");
    assert_eq!(mode(&paths.server_cert), 0o600);
    assert_eq!(mode(&paths.server_key), 0o600);
}

#[test]
fn a_missing_leaf_is_rebuilt_under_the_same_ca() {
    let state = TempDir::new().unwrap();
    let paths = ensure(state.path(), SystemTime::now()).unwrap();
    let ca_before = fs::read(&paths.ca_cert).unwrap();
    fs::remove_file(&paths.server_key).unwrap();

    ensure(state.path(), SystemTime::now()).unwrap();

    assert_eq!(fs::read(&paths.ca_cert).unwrap(), ca_before);
    assert!(paths.server_key.exists());
}

#[test]
fn no_pem_on_disk_is_a_private_key_in_the_certificate_files() {
    let state = TempDir::new().unwrap();
    let paths = ensure(state.path(), SystemTime::now()).unwrap();

    for cert in [&paths.ca_cert, &paths.server_cert] {
        assert!(!fs::read_to_string(cert).unwrap().contains("PRIVATE KEY"));
    }
    for key in [&paths.ca_key, &paths.server_key] {
        assert!(fs::read_to_string(key).unwrap().contains("PRIVATE KEY"));
    }
}
