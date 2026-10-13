//! The NSS operations against the real `certutil` and `pk12util`, on
//! throwaway databases in a temp directory.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod nss_support;

use std::time::SystemTime;

use claude_monitor::browser_setup::nss::{DbAccess, Nss, CA_NICKNAME, CLIENT_NICKNAME};
use claude_monitor::browser_setup::runner::SystemRunner;
use claude_monitor::browser_setup::secret_file::SecretFile;
use claude_monitor::web::tls;
use tempfile::TempDir;

struct Fixture {
    state: TempDir,
    scratch: TempDir,
    empty: SecretFile,
}

fn fixture() -> Fixture {
    let state = TempDir::new().unwrap();
    let scratch = TempDir::new().unwrap();
    let empty = SecretFile::create(scratch.path(), "empty", b"\n").unwrap();
    Fixture {
        state,
        scratch,
        empty,
    }
}

fn import(fx: &Fixture, nss: &Nss<'_>, db: &std::path::Path) -> Vec<u8> {
    let identity = tls::ensure_client_identity(fx.state.path(), SystemTime::now()).unwrap();
    let bundle = tls::client_pkcs12(&identity, "bundle-password").unwrap();
    let bundle_file = SecretFile::create(fx.scratch.path(), "bundle", &bundle).unwrap();
    let password_file =
        SecretFile::create(fx.scratch.path(), "bundle-pw", b"bundle-password").unwrap();
    nss.import_client(db, bundle_file.path(), password_file.path())
        .unwrap();
    identity.cert_der
}

#[test]
fn the_ca_is_trusted_and_read_back_byte_for_byte() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let fx = fixture();
    let db = fx.scratch.path().join("db");
    nss_support::new_db(&db);
    let paths = tls::ensure(fx.state.path(), SystemTime::now()).unwrap();
    let nss = Nss::new(&SystemRunner, fx.empty.path());

    assert_eq!(nss.stored_der(&db, CA_NICKNAME).unwrap(), None);
    nss.trust_ca(&db, &paths.ca_cert).unwrap();

    assert_eq!(
        nss.stored_der(&db, CA_NICKNAME).unwrap(),
        tls::read_ca_der(fx.state.path())
    );
    assert!(nss_support::list(&db).contains(CA_NICKNAME));
}

#[test]
fn the_client_certificate_imports_under_its_nickname() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let fx = fixture();
    let db = fx.scratch.path().join("db");
    nss_support::new_db(&db);
    let nss = Nss::new(&SystemRunner, fx.empty.path());

    let cert_der = import(&fx, &nss, &db);

    assert_eq!(
        nss.stored_der(&db, CLIENT_NICKNAME).unwrap(),
        Some(cert_der)
    );
    assert!(nss_support::keys(&db, fx.empty.path()).contains(CLIENT_NICKNAME));
}

#[test]
fn removing_the_client_takes_its_key_too_and_leaves_the_ca() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let fx = fixture();
    let db = fx.scratch.path().join("db");
    nss_support::new_db(&db);
    let paths = tls::ensure(fx.state.path(), SystemTime::now()).unwrap();
    let nss = Nss::new(&SystemRunner, fx.empty.path());
    nss.trust_ca(&db, &paths.ca_cert).unwrap();
    import(&fx, &nss, &db);

    nss.remove(&db, CLIENT_NICKNAME, true).unwrap();

    assert_eq!(nss.stored_der(&db, CLIENT_NICKNAME).unwrap(), None);
    assert!(nss.stored_der(&db, CA_NICKNAME).unwrap().is_some());
    assert!(!nss_support::keys(&db, fx.empty.path()).contains(CLIENT_NICKNAME));
}

#[test]
fn access_tells_an_open_database_from_a_password_protected_one() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let fx = fixture();
    let open = fx.scratch.path().join("open");
    let locked = fx.scratch.path().join("locked");
    nss_support::new_db(&open);
    let secret = SecretFile::create(fx.scratch.path(), "primary", b"primary-password\n").unwrap();
    nss_support::new_db_with_password(&locked, secret.path());
    let nss = Nss::new(&SystemRunner, fx.empty.path());

    assert_eq!(nss.access(&open).unwrap(), DbAccess::Open);
    assert_eq!(nss.access(&locked).unwrap(), DbAccess::PrimaryPassword);
}
