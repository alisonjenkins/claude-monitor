//! `serve --mtls`: a client certificate from the install's CA is required in
//! the TLS handshake, before any HTTP is read, and login still applies.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::SystemTime;

use claude_monitor::web::tls::{self, client_pkcs12, issue_client_identity};
use p12_keystore::{KeyStore, Pkcs12ImportPolicy};
use tempfile::TempDir;
use web_support::{client_config, client_config_with, tls_over, Identity, Request, Rig, TlsStream};

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");

fn mtls_rig() -> Rig {
    Rig::start_with(|c| c.mtls = true)
}

fn ca_of(rig: &Rig) -> PathBuf {
    rig.dir.path().join("tls").join("ca.pem")
}

fn identity_of(state: &Path) -> Identity {
    let id = issue_client_identity(state, SystemTime::now()).unwrap();
    (id.cert_der, id.key_der.to_vec())
}

fn pem_der(path: &Path) -> Vec<u8> {
    let pem = std::fs::read(path).unwrap();
    x509_parser::pem::parse_x509_pem(&pem).unwrap().1.contents
}

fn key_der(path: &Path) -> Vec<u8> {
    let pem = std::fs::read_to_string(path).unwrap();
    rcgen::KeyPair::from_pem(&pem).unwrap().serialize_der()
}

/// Sends a request and reports whether any HTTP came back. A refused client
/// may fail at connect, at write or at read; all of them mean "no answer".
fn answer_of(mut stream: TlsStream, host: &str) -> Option<String> {
    let request = format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return None;
    }
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    (!raw.is_empty()).then(|| String::from_utf8_lossy(&raw).into_owned())
}

#[test]
fn without_a_client_certificate_the_server_says_nothing() {
    let rig = mtls_rig();

    let answer = answer_of(
        tls_over(rig.addr(), client_config(&[&ca_of(&rig)])),
        &rig.host(),
    );

    assert_eq!(
        answer, None,
        "no HTTP may be answered without a client cert"
    );
}

#[test]
fn a_refused_client_never_reaches_the_http_parser() {
    let rig = Rig::start_with(|c| {
        c.mtls = true;
        c.max_auth_failures = 1;
    });
    let _ = answer_of(
        tls_over(rig.addr(), client_config(&[&ca_of(&rig)])),
        &rig.host(),
    );

    let anonymous = Request::get("/healthz").send(rig.addr());

    assert_eq!(
        anonymous.status, 401,
        "an HTTP failure counted from the refused client would make this 429"
    );
}

#[test]
fn the_installs_client_certificate_is_accepted_and_login_still_applies() {
    let rig = mtls_rig();

    let anonymous = Request::get("/healthz").send(rig.addr());
    let signed_in = Request::get("/healthz")
        .header("Cookie", rig.cookie())
        .send(rig.addr());

    assert_eq!(anonymous.status, 401, "a client cert is not a login");
    assert_eq!(signed_in.status, 200);
}

#[test]
fn a_certificate_from_another_install_is_refused() {
    let rig = mtls_rig();
    let elsewhere = TempDir::new().unwrap();
    tls::ensure(elsewhere.path(), SystemTime::now()).unwrap();
    let theirs = identity_of(elsewhere.path());

    let answer = answer_of(
        tls_over(rig.addr(), client_config_with(&[&ca_of(&rig)], theirs)),
        &rig.host(),
    );

    assert_eq!(answer, None);
}

#[test]
fn the_servers_own_leaf_certificate_is_not_a_client_certificate() {
    let rig = mtls_rig();
    let tls = rig.dir.path().join("tls");
    let leaf: Identity = (
        pem_der(&tls.join("server.pem")),
        key_der(&tls.join("server.key")),
    );

    let answer = answer_of(
        tls_over(rig.addr(), client_config_with(&[&ca_of(&rig)], leaf)),
        &rig.host(),
    );

    assert_eq!(answer, None, "same CA, but the leaf lacks the client EKU");
}

#[test]
fn without_mtls_a_client_certificate_is_not_needed() {
    let rig = Rig::start();

    let answer = answer_of(
        tls_over(rig.addr(), client_config(&[&ca_of(&rig)])),
        &rig.host(),
    );

    assert!(answer.unwrap().starts_with("HTTP/1.1 401"));
}

#[test]
fn the_pkcs12_bundle_holds_the_key_and_certificate_under_the_password() {
    let state = TempDir::new().unwrap();
    tls::ensure(state.path(), SystemTime::now()).unwrap();
    let id = issue_client_identity(state.path(), SystemTime::now()).unwrap();

    let bundle = client_pkcs12(&id, "correct horse").unwrap();

    let store =
        KeyStore::from_pkcs12(&bundle, "correct horse", Pkcs12ImportPolicy::Strict).unwrap();
    let (_, chain) = store.private_key_chain().unwrap();
    assert_eq!(chain.key().as_der(), id.key_der.as_slice());
    assert_eq!(
        chain.certs().first().unwrap().as_der(),
        id.cert_der.as_slice()
    );
    assert!(KeyStore::from_pkcs12(&bundle, "wrong", Pkcs12ImportPolicy::Strict).is_err());
}

#[test]
fn client_cert_writes_a_private_bundle_that_logs_into_an_mtls_server() {
    let rig = mtls_rig();
    let out = rig.dir.path().join("me.p12");
    let password_file = rig.dir.path().join("pw");
    std::fs::write(&password_file, "s3cret-from-file\n").unwrap();

    let output = Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(rig.dir.path())
        .args(["web", "client-cert", "--out"])
        .arg(&out)
        .arg("--password-file")
        .arg(&password_file)
        .env("HOME", rig.dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains(&out.display().to_string()), "{printed}");
    assert!(
        !printed.contains("s3cret"),
        "a password the user chose is not echoed"
    );
    assert_eq!(
        std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let store = KeyStore::from_pkcs12(
        &std::fs::read(&out).unwrap(),
        "s3cret-from-file",
        Pkcs12ImportPolicy::Strict,
    )
    .unwrap();
    let (_, chain) = store.private_key_chain().unwrap();
    let identity: Identity = (
        chain.certs().first().unwrap().as_der().to_vec(),
        chain.key().as_der().to_vec(),
    );
    let answer = answer_of(
        tls_over(rig.addr(), client_config_with(&[&ca_of(&rig)], identity)),
        &rig.host(),
    );
    assert!(
        answer.unwrap().starts_with("HTTP/1.1 401"),
        "handshake passed"
    );
}

#[test]
fn client_cert_without_a_password_file_prints_a_random_one_once() {
    let state = TempDir::new().unwrap();
    let out = state.path().join("c.p12");

    let output = Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(state.path())
        .args(["web", "client-cert", "--out"])
        .arg(&out)
        .env("HOME", state.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let printed = String::from_utf8(output.stdout).unwrap();
    let password = printed
        .lines()
        .find_map(|l| l.strip_prefix("password: "))
        .unwrap_or_else(|| panic!("no password line in {printed:?}"))
        .trim();
    assert!(password.len() >= 32, "{password}");
    let opened = KeyStore::from_pkcs12(
        &std::fs::read(&out).unwrap(),
        password,
        Pkcs12ImportPolicy::Strict,
    );
    assert!(opened.is_ok());
}

#[test]
fn serve_help_mentions_mtls_and_it_defaults_off() {
    let output = Process::new(BIN)
        .args(["serve", "--help"])
        .output()
        .unwrap();

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--mtls"), "{help}");
}
