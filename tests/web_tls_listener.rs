//! What a client sees on the wire: TLS 1.3 with the install's CA, nothing
//! else.
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
use std::path::PathBuf;
use std::time::SystemTime;

use claude_monitor::web::tls;
use tempfile::TempDir;
use web_support::{client_config, tcp, tls_over, Request, Rig, TlsStream};

fn ca_of(rig: &Rig) -> PathBuf {
    rig.dir.path().join("tls").join("ca.pem")
}

fn handshake(stream: &mut TlsStream) -> std::io::Result<()> {
    stream.conn.complete_io(&mut stream.sock).map(|_| ())
}

#[test]
fn a_client_trusting_only_the_generated_ca_completes_the_handshake() {
    let rig = Rig::start();

    let mut stream = tls_over(rig.addr(), client_config(&[&ca_of(&rig)]));
    handshake(&mut stream).unwrap();

    assert_eq!(
        stream.conn.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(Request::get("/healthz").send(rig.addr()).status, 401);
}

#[test]
fn a_client_trusting_an_unrelated_ca_is_refused() {
    let rig = Rig::start();
    let elsewhere = TempDir::new().unwrap();
    let other = tls::ensure(elsewhere.path(), SystemTime::now()).unwrap();

    let mut stream = tls_over(rig.addr(), client_config(&[&other.ca_cert]));
    let error = handshake(&mut stream).unwrap_err();

    assert!(
        error.to_string().to_lowercase().contains("certificate"),
        "{error}"
    );
}

#[test]
fn plain_http_to_the_port_gets_no_http_answer() {
    let rig = Rig::start();
    let mut stream = tcp(rig.addr());
    stream
        .write_all(
            format!(
                "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                rig.host()
            )
            .as_bytes(),
        )
        .unwrap();

    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);

    assert!(
        !answer.starts_with(b"HTTP/"),
        "plain HTTP was answered: {:?}",
        String::from_utf8_lossy(&answer)
    );
}

#[test]
fn a_tls_1_2_client_is_refused() {
    let rig = Rig::start();
    let mut store = rustls::RootCertStore::empty();
    store.add(web_support::ca_der(&ca_of(&rig))).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS12])
    .unwrap()
    .with_root_certificates(store)
    .with_no_client_auth();

    let mut stream = tls_over(rig.addr(), config);
    let error = handshake(&mut stream).unwrap_err();

    assert!(
        error.to_string().to_lowercase().contains("version"),
        "{error}"
    );
}
