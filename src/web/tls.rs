//! The per-install CA and the loopback leaf certificate.
//!
//! `serve` presents a leaf for `127.0.0.1`, `::1` and `localhost` signed by a
//! CA that lives in `<state-dir>/tls/` and is name-constrained to those names,
//! so trusting it in a browser cannot be used to impersonate any other site.
//! Everything in the directory is private to the user: the directory is 0700,
//! files are 0600 from the moment they exist.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;

use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    NameConstraints, SanType,
};
use time::{Duration, OffsetDateTime};
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroizing;

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// The CA lasts five years (plus the leap day).
const CA_VALIDITY_DAYS: i64 = 5 * 365 + 1;
const LEAF_VALIDITY_DAYS: i64 = 90;
/// A client certificate is imported into a browser by hand, so it lasts a year.
const CLIENT_VALIDITY_DAYS: i64 = 365;
pub const CLIENT_ALIAS: &str = "claude-monitor web client";
/// A persisted client certificate with less than this left is replaced.
const CLIENT_RENEW_WITHIN_DAYS: i64 = 30;
/// A leaf with less than this left is replaced at the next `serve`.
const LEAF_RENEW_WITHIN_DAYS: i64 = 14;
/// A CA this close to expiry is replaced along with its leaf.
const CA_RENEW_WITHIN_DAYS: i64 = 30;
/// Certificates start a day early so a slightly slow clock still validates.
const BACKDATE_DAYS: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("cannot {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("cannot {operation}: {detail}")]
    Generate {
        operation: &'static str,
        detail: String,
    },
    #[error("the system clock is not representable as a certificate date")]
    Clock,
}

fn generate(operation: &'static str) -> impl FnOnce(rcgen::Error) -> TlsError {
    move |e| TlsError::Generate {
        operation,
        detail: e.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPaths {
    pub dir: PathBuf,
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

impl TlsPaths {
    pub fn in_state_dir(state_dir: &Path) -> Self {
        let dir = state_dir.join("tls");
        Self {
            ca_cert: dir.join("ca.pem"),
            ca_key: dir.join("ca.key"),
            server_cert: dir.join("server.pem"),
            server_key: dir.join("server.key"),
            client_cert: dir.join("client.pem"),
            client_key: dir.join("client.key"),
            dir,
        }
    }
}

/// The persisted client identity, created or replaced as needed. Unlike
/// [`issue_client_identity`] it returns the same certificate on every call
/// until that one is close to expiry or the CA changes, so importing it into a
/// browser a second time is a no-op.
pub fn ensure_client_identity(
    state_dir: &Path,
    now: SystemTime,
) -> Result<ClientIdentity, TlsError> {
    let paths = ensure(state_dir, now)?;
    if let Some(identity) = load_client_identity(&paths, to_date(now)?) {
        return Ok(identity);
    }
    let issued = issue(state_dir, now)?;
    write_private(&paths.client_key, issued.key_pem.as_bytes())?;
    write_private(&paths.client_cert, issued.cert_pem.as_bytes())?;
    Ok(issued.identity)
}

/// The persisted client identity if it is still usable; writes nothing.
pub fn peek_client_identity(state_dir: &Path, now: SystemTime) -> Option<ClientIdentity> {
    let paths = TlsPaths::in_state_dir(state_dir);
    load_client_identity(&paths, to_date(now).ok()?)
}

/// The CA certificate as DER, if the install has one.
pub fn read_ca_der(state_dir: &Path) -> Option<Vec<u8>> {
    read_der(&TlsPaths::in_state_dir(state_dir).ca_cert)
}

/// Lowercase hex SHA-256 of a DER certificate: the fingerprint browsers show.
pub fn fingerprint_sha256(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn load_client_identity(paths: &TlsPaths, now: OffsetDateTime) -> Option<ClientIdentity> {
    let cert_der = read_der(&paths.client_cert)?;
    let key = load_key(&paths.client_key)?;
    let ca_der = read_der(&paths.ca_cert)?;
    let (_, cert) = X509Certificate::from_der(&cert_der).ok()?;
    let (_, ca) = X509Certificate::from_der(&ca_der).ok()?;
    let days = cert
        .validity()
        .not_after
        .timestamp()
        .saturating_sub(now.unix_timestamp())
        / 86_400;
    let usable = days >= CLIENT_RENEW_WITHIN_DAYS
        && cert.verify_signature(Some(ca.public_key())).is_ok()
        && cert.public_key().subject_public_key.data.as_ref() == key.public_key_raw();
    usable.then(|| ClientIdentity {
        cert_der,
        key_der: Zeroizing::new(key.serialize_der()),
    })
}

/// Creates whatever is missing or about to expire and returns the paths.
/// `now` is injected so renewal can be tested without waiting 76 days.
pub fn ensure(state_dir: &Path, now: SystemTime) -> Result<TlsPaths, TlsError> {
    let paths = TlsPaths::in_state_dir(state_dir);
    let now = to_date(now)?;
    make_private_dir(&paths.dir)?;
    let ca_fresh = if ca_usable(&paths, now) {
        false
    } else {
        write_ca(&paths, now)?;
        true
    };
    if ca_fresh || !leaf_usable(&paths, now) {
        write_leaf(&paths, now)?;
    }
    Ok(paths)
}

/// A client certificate and its PKCS#8 key, both DER.
pub struct ClientIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Zeroizing<Vec<u8>>,
}

/// A new client certificate signed by the install's CA, for `serve --mtls`.
/// The CA signs both server and client certificates; what keeps one from
/// standing in for the other is the extended key usage (serverAuth on the
/// leaf, clientAuth here), which the verifier on each side enforces.
pub fn issue_client_identity(
    state_dir: &Path,
    now: SystemTime,
) -> Result<ClientIdentity, TlsError> {
    issue(state_dir, now).map(|issued| issued.identity)
}

struct Issued {
    identity: ClientIdentity,
    cert_pem: String,
    key_pem: Zeroizing<String>,
}

fn issue(state_dir: &Path, now: SystemTime) -> Result<Issued, TlsError> {
    let paths = TlsPaths::in_state_dir(state_dir);
    let now = to_date(now)?;
    let issuer = load_issuer(&paths)?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "claude-monitor web client");
    params.distinguished_name = name;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = shifted(now, -BACKDATE_DAYS)?;
    params.not_after = shifted(now, CLIENT_VALIDITY_DAYS)?;
    let key = KeyPair::generate().map_err(generate("generate the client key"))?;
    let cert = params
        .signed_by(&key, &issuer)
        .map_err(generate("sign the client certificate"))?;
    Ok(Issued {
        identity: ClientIdentity {
            cert_der: cert.der().to_vec(),
            key_der: Zeroizing::new(key.serialize_der()),
        },
        cert_pem: cert.pem(),
        key_pem: Zeroizing::new(key.serialize_pem()),
    })
}

/// The identity as a password-protected PKCS#12 file a browser can import.
/// Only the client certificate is bundled; the CA is trusted separately
/// (`web ca`).
pub fn client_pkcs12(identity: &ClientIdentity, password: &str) -> Result<Vec<u8>, TlsError> {
    let bundle = |detail: p12_keystore::error::Error| TlsError::Generate {
        operation: "build the PKCS#12 bundle",
        detail: detail.to_string(),
    };
    let cert = p12_keystore::Certificate::from_der(&identity.cert_der).map_err(bundle)?;
    let key = p12_keystore::PrivateKey::from_der(&identity.key_der).map_err(bundle)?;
    let chain = p12_keystore::PrivateKeyChain::new(CLIENT_ALIAS, key, [cert]);
    let mut store = p12_keystore::KeyStore::new();
    store.add_entry(
        CLIENT_ALIAS,
        p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
    );
    store.writer(password).write().map_err(bundle)
}

/// The server side of every connection: TLS 1.3 only, HTTP/1.1 over ALPN, and
/// with `require_client_cert` a client certificate from this install's CA.
pub fn server_config(
    paths: &TlsPaths,
    require_client_cert: bool,
) -> Result<Arc<ServerConfig>, TlsError> {
    let leaf = read_der(&paths.server_cert).ok_or_else(|| TlsError::Generate {
        operation: "read the leaf certificate",
        detail: paths.server_cert.display().to_string(),
    })?;
    let key = load_key(&paths.server_key).ok_or_else(|| TlsError::Generate {
        operation: "read the leaf key",
        detail: paths.server_key.display().to_string(),
    })?;
    let key = PrivatePkcs8KeyDer::from(key.serialize_der());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TlsError::Generate {
            operation: "choose the TLS version",
            detail: e.to_string(),
        })?;
    let builder = if require_client_cert {
        let ca = read_der(&paths.ca_cert).ok_or_else(|| TlsError::Generate {
            operation: "read the CA certificate",
            detail: paths.ca_cert.display().to_string(),
        })?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(ca))
            .map_err(|e| TlsError::Generate {
                operation: "trust the CA for client certificates",
                detail: e.to_string(),
            })?;
        let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|e| TlsError::Generate {
                operation: "build the client certificate verifier",
                detail: e.to_string(),
            })?;
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let mut config = builder
        .with_single_cert(vec![CertificateDer::from(leaf)], key.into())
        .map_err(|e| TlsError::Generate {
            operation: "load the leaf certificate",
            detail: e.to_string(),
        })?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn to_date(at: SystemTime) -> Result<OffsetDateTime, TlsError> {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TlsError::Clock)?
        .as_secs();
    let secs = i64::try_from(secs).map_err(|_| TlsError::Clock)?;
    OffsetDateTime::from_unix_timestamp(secs).map_err(|_| TlsError::Clock)
}

fn make_private_dir(dir: &Path) -> Result<(), TlsError> {
    let fail = |source| TlsError::Io {
        operation: "create the TLS directory",
        path: dir.to_owned(),
        source,
    };
    fs::create_dir_all(dir).map_err(fail)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(DIR_MODE)).map_err(fail)
}

/// Writes through a 0600 staging file and renames it into place, so the file
/// is never readable by anyone else and never half written.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<(), TlsError> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".new");
    let staging = PathBuf::from(staging);
    let fail = |source| TlsError::Io {
        operation: "write",
        path: path.to_owned(),
        source,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(&staging)
        .map_err(fail)?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(FILE_MODE)).map_err(fail)?;
    file.write_all(contents).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);
    fs::rename(&staging, path).map_err(fail)
}

fn read_der(path: &Path) -> Option<Vec<u8>> {
    let pem = fs::read(path).ok()?;
    let (_, parsed) = x509_parser::pem::parse_x509_pem(&pem).ok()?;
    Some(parsed.contents)
}

fn not_after(der: &[u8]) -> Option<i64> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    Some(cert.validity().not_after.timestamp())
}

fn days_left(path: &Path, now: OffsetDateTime) -> Option<i64> {
    let end = not_after(&read_der(path)?)?;
    Some(end.saturating_sub(now.unix_timestamp()) / 86_400)
}

fn load_key(path: &Path) -> Option<KeyPair> {
    let pem = Zeroizing::new(fs::read_to_string(path).ok()?);
    KeyPair::from_pem(&pem).ok()
}

fn ca_usable(paths: &TlsPaths, now: OffsetDateTime) -> bool {
    days_left(&paths.ca_cert, now).is_some_and(|d| d >= CA_RENEW_WITHIN_DAYS)
        && load_key(&paths.ca_key).is_some()
}

fn leaf_usable(paths: &TlsPaths, now: OffsetDateTime) -> bool {
    days_left(&paths.server_cert, now).is_some_and(|d| d >= LEAF_RENEW_WITHIN_DAYS)
        && load_key(&paths.server_key).is_some()
}

fn shifted(from: OffsetDateTime, days: i64) -> Result<OffsetDateTime, TlsError> {
    from.checked_add(Duration::days(days))
        .ok_or(TlsError::Clock)
}

fn ca_params(now: OffsetDateTime) -> Result<CertificateParams, TlsError> {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "claude-monitor local CA");
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = shifted(now, -BACKDATE_DAYS)?;
    params.not_after = shifted(now, CA_VALIDITY_DAYS)?;
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("localhost".to_owned()),
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                8,
            )),
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                128,
            )),
        ],
        excluded_subtrees: Vec::new(),
    });
    Ok(params)
}

fn write_ca(paths: &TlsPaths, now: OffsetDateTime) -> Result<(), TlsError> {
    let key = KeyPair::generate().map_err(generate("generate the CA key"))?;
    let cert = ca_params(now)?
        .self_signed(&key)
        .map_err(generate("sign the CA certificate"))?;
    let key_pem = Zeroizing::new(key.serialize_pem());
    write_private(&paths.ca_key, key_pem.as_bytes())?;
    write_private(&paths.ca_cert, cert.pem().as_bytes())
}

fn load_issuer(paths: &TlsPaths) -> Result<Issuer<'static, KeyPair>, TlsError> {
    let ca_key = load_key(&paths.ca_key).ok_or_else(|| TlsError::Generate {
        operation: "read the CA key",
        detail: paths.ca_key.display().to_string(),
    })?;
    let ca_pem = fs::read_to_string(&paths.ca_cert).map_err(|source| TlsError::Io {
        operation: "read",
        path: paths.ca_cert.clone(),
        source,
    })?;
    Issuer::from_ca_cert_pem(&ca_pem, ca_key).map_err(generate("load the CA certificate"))
}

fn write_leaf(paths: &TlsPaths, now: OffsetDateTime) -> Result<(), TlsError> {
    let issuer = load_issuer(paths)?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "claude-monitor web view");
    params.distinguished_name = name;
    params.subject_alt_names = vec![
        SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(generate("name the leaf certificate"))?,
        ),
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    ];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = shifted(now, -BACKDATE_DAYS)?;
    params.not_after = shifted(now, LEAF_VALIDITY_DAYS)?;
    let key = KeyPair::generate().map_err(generate("generate the leaf key"))?;
    let cert = params
        .signed_by(&key, &issuer)
        .map_err(generate("sign the leaf certificate"))?;
    let key_pem = Zeroizing::new(key.serialize_pem());
    write_private(&paths.server_key, key_pem.as_bytes())?;
    write_private(&paths.server_cert, cert.pem().as_bytes())
}
