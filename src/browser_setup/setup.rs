//! `setup-browser` end to end: which targets, what each needs, doing it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use super::discover::{discover, Browser, Roots, Target};
use super::nss::{DbAccess, Nss, CA_NICKNAME, CLIENT_NICKNAME};
use super::report::{Outcome, Report, TargetReport};
use super::runner::CommandRunner;
use super::secret_file::SecretFile;
use super::{chromium_policy, profile, userjs};
use crate::web::secret;
use crate::web::tls::{self, ClientIdentity, TlsError, TlsPaths};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub browsers: Vec<Browser>,
    pub profiles: Vec<PathBuf>,
    pub auto_select: bool,
    pub force: bool,
    pub dry_run: bool,
}

pub struct Env<'a> {
    pub runner: &'a dyn CommandRunner,
    pub roots: &'a Roots,
    pub state_dir: &'a Path,
    pub now: SystemTime,
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("no browser profile or certificate database was found; name one with --profile PATH")]
    NoTargets,
    #[error("cannot prepare the TLS material: {0}")]
    Tls(#[from] TlsError),
}

pub fn run(env: &Env<'_>, opts: &Options) -> Result<Report, SetupError> {
    let targets = discover(env.roots, &opts.browsers, &opts.profiles);
    if targets.is_empty() {
        return Err(SetupError::NoTargets);
    }
    let material = Material::load(env, opts)?;
    let mut report = Report {
        dry_run: opts.dry_run,
        ..Report::default()
    };
    if material.is_none() {
        report
            .notes
            .push("dry run: the CA and client certificate do not exist yet".to_owned());
    }
    let mut job = Job {
        opts,
        nss: Nss::new(env.runner, Path::new(NO_PASSWORD_FILE)),
        material,
        tls_dir: TlsPaths::in_state_dir(env.state_dir).dir,
        bundle: None,
        now: DateTime::<Utc>::from(env.now),
    };
    for target in targets {
        let outcome = job.process(&target);
        report.targets.push(TargetReport { target, outcome });
    }
    if opts.auto_select && opts.browsers.contains(&Browser::Chromium) {
        report.chromium_policy = Some(chromium_policy::instructions());
    }
    Ok(report)
}

/// `/dev/null` reads as an empty password file, which is the right answer
/// for a database without a primary password and never opens a prompt.
const NO_PASSWORD_FILE: &str = "/dev/null";

struct Material {
    ca_pem: PathBuf,
    ca_fingerprint: String,
    client_fingerprint: String,
    /// Only a real run needs the key, to build the bundle.
    identity: Option<ClientIdentity>,
}

impl Material {
    /// A real run creates whatever is missing. A dry run only looks, and
    /// reports `None` when there is nothing to compare with yet.
    fn load(env: &Env<'_>, opts: &Options) -> Result<Option<Self>, SetupError> {
        let paths = TlsPaths::in_state_dir(env.state_dir);
        let identity = if opts.dry_run {
            match tls::peek_client_identity(env.state_dir, env.now) {
                Some(identity) => identity,
                None => return Ok(None),
            }
        } else {
            tls::ensure_client_identity(env.state_dir, env.now)?
        };
        let Some(ca_der) = tls::read_ca_der(env.state_dir) else {
            return Ok(None);
        };
        Ok(Some(Self {
            ca_pem: paths.ca_cert,
            ca_fingerprint: tls::fingerprint_sha256(&ca_der),
            client_fingerprint: tls::fingerprint_sha256(&identity.cert_der),
            identity: (!opts.dry_run).then_some(identity),
        }))
    }
}

/// The PKCS#12 bundle and its password, as files only this user can read.
/// Dropping them removes both, on every exit path.
struct Bundle {
    file: SecretFile,
    password: SecretFile,
}

struct Job<'a> {
    opts: &'a Options,
    nss: Nss<'a>,
    material: Option<Material>,
    tls_dir: PathBuf,
    bundle: Option<Bundle>,
    now: DateTime<Utc>,
}

fn differs(stored: Option<&Vec<u8>>, fingerprint: &str) -> bool {
    stored.is_none_or(|der| tls::fingerprint_sha256(der) != fingerprint)
}

fn user_js_path(target: &Target) -> Option<PathBuf> {
    let browser_with_prefs = match target.browser {
        Browser::Firefox | Browser::Zen => true,
        Browser::Custom => target.dir.join("prefs.js").is_file(),
        Browser::Chromium => false,
    };
    browser_with_prefs.then(|| target.dir.join("user.js"))
}

impl Job<'_> {
    fn process(&mut self, target: &Target) -> Outcome {
        let dir = target.dir.as_path();
        if target.browser.is_gecko() && !self.opts.force {
            if let Some(reason) = profile::in_use(dir) {
                return Outcome::InUse(reason);
            }
        }
        if !profile::has_database(dir) {
            return Outcome::NoDatabase;
        }
        match self.nss.access(dir) {
            Ok(DbAccess::Open) => {}
            Ok(DbAccess::PrimaryPassword) => {
                return Outcome::Skipped(
                    "a primary password protects this profile and certutil would prompt for it; \
                     import by hand or remove the password"
                        .to_owned(),
                )
            }
            Err(e) => return Outcome::Failed(e.to_string()),
        }
        let (ca_stored, client_stored) = match (
            self.nss.stored_der(dir, CA_NICKNAME),
            self.nss.stored_der(dir, CLIENT_NICKNAME),
        ) {
            (Ok(ca), Ok(client)) => (ca, client),
            (Err(e), _) | (_, Err(e)) => return Outcome::Failed(e.to_string()),
        };
        let (need_ca, need_client) = match &self.material {
            None => (true, true),
            Some(m) => (
                differs(ca_stored.as_ref(), &m.ca_fingerprint),
                differs(client_stored.as_ref(), &m.client_fingerprint),
            ),
        };
        let js = user_js_path(target).filter(|_| self.opts.auto_select);
        let need_js = match js.as_deref().map(userjs::needs_update) {
            Some(Ok(needed)) => needed,
            Some(Err(e)) => return Outcome::Failed(e.to_string()),
            None => false,
        };
        if !(need_ca || need_client || need_js) {
            return Outcome::UpToDate;
        }
        if self.opts.dry_run {
            return Outcome::WouldImport {
                ca: need_ca,
                client: need_client,
                auto_select: need_js,
            };
        }
        match self.write(
            dir,
            Needs {
                ca: need_ca.then_some(ca_stored.is_some()),
                client: need_client.then_some(client_stored.is_some()),
                js: js.filter(|_| need_js),
            },
        ) {
            Ok(backup) => Outcome::Imported {
                ca: need_ca,
                client: need_client,
                auto_select: need_js,
                backup,
            },
            Err(detail) => Outcome::Failed(detail),
        }
    }

    /// Backs up first, then replaces what changed. `Some(existing)` in
    /// `needs` means "write this, removing the old one if `existing`".
    fn write(&mut self, dir: &Path, needs: Needs) -> Result<Option<PathBuf>, String> {
        let backup =
            if needs.ca.is_some() || needs.client.is_some() {
                Some(profile::backup_database(dir, self.now).map_err(|e| {
                    format!("cannot back up the database in {}: {e}", dir.display())
                })?)
            } else {
                None
            };
        if let Some(existing) = needs.ca {
            let ca_pem = self.material.as_ref().map(|m| m.ca_pem.clone());
            let ca_pem = ca_pem.ok_or("the CA certificate is not available")?;
            if existing {
                self.nss
                    .remove(dir, CA_NICKNAME, false)
                    .map_err(|e| e.to_string())?;
            }
            self.nss.trust_ca(dir, &ca_pem).map_err(|e| e.to_string())?;
        }
        if let Some(existing) = needs.client {
            if existing {
                self.nss
                    .remove(dir, CLIENT_NICKNAME, true)
                    .map_err(|e| e.to_string())?;
            }
            self.ensure_bundle()?;
            if let Some(bundle) = &self.bundle {
                self.nss
                    .import_client(dir, bundle.file.path(), bundle.password.path())
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some(path) = needs.js {
            userjs::apply(&path).map_err(|e| e.to_string())?;
        }
        Ok(backup)
    }

    fn ensure_bundle(&mut self) -> Result<(), String> {
        if self.bundle.is_some() {
            return Ok(());
        }
        let identity = self
            .material
            .as_ref()
            .and_then(|m| m.identity.as_ref())
            .ok_or("the client certificate is not available")?;
        let password = secret::random_hex().map_err(|e| e.to_string())?;
        let p12 = tls::client_pkcs12(identity, &password).map_err(|e| e.to_string())?;
        let private = |what: &str, prefix, bytes: &[u8]| {
            SecretFile::create(&self.tls_dir, prefix, bytes)
                .map_err(|e| format!("cannot write the {what} file: {e}"))
        };
        let file = private("bundle", "browser-setup-bundle", &p12)?;
        let password = private("bundle password", "browser-setup-pw", password.as_bytes())?;
        self.bundle = Some(Bundle { file, password });
        Ok(())
    }
}

/// What to write to one target. `ca` and `client` are `Some(existing)` when
/// they need writing, `existing` saying whether an old one must go first.
struct Needs {
    ca: Option<bool>,
    client: Option<bool>,
    js: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::unwrap_in_result,
        clippy::panic,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::rc::Rc;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::super::nss::{CA_NICKNAME, CLIENT_NICKNAME};
    use super::super::report::Outcome;
    use super::super::runner::fake::{failed, ok, FakeRunner};
    use super::super::runner::{CommandOutput, CommandSpec, RunError};
    use super::super::userjs;
    use super::*;
    use crate::web::tls;

    /// An in-memory stand-in for one NSS database.
    #[derive(Default)]
    struct Db {
        ca: Option<Vec<u8>>,
        client: Option<Vec<u8>>,
        locked: bool,
    }

    #[derive(Default)]
    struct Seen {
        /// Contents of every file named by a `-w`/`-i` argument at call time.
        files: BTreeMap<PathBuf, (Vec<u8>, u32)>,
        backup_existed_at_first_write: Option<bool>,
        pk12_error: bool,
        hang: bool,
    }

    struct Rig {
        home: TempDir,
        state: TempDir,
        db: Rc<RefCell<Db>>,
        seen: Rc<RefCell<Seen>>,
        runner: FakeRunner,
        profile: PathBuf,
        client_der: Vec<u8>,
    }

    fn base64(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn pem(der: &[u8]) -> Vec<u8> {
        let body: Vec<String> = base64(der)
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            body.join("\n")
        )
        .into_bytes()
    }

    fn arg(spec: &CommandSpec, flag: &str) -> Option<String> {
        let at = spec.args.iter().position(|a| a == flag)?;
        spec.args
            .get(at + 1)
            .map(|a| a.to_string_lossy().into_owned())
    }

    fn rig() -> Rig {
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let ff = home.path().join(".mozilla/firefox");
        let profile = ff.join("abc.default");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[Profile0]\nName=default\nIsRelative=1\nPath=abc.default\n",
        )
        .unwrap();
        fs::write(profile.join("cert9.db"), "cert-db").unwrap();
        fs::write(profile.join("key4.db"), "key-db").unwrap();
        let client_der = tls::ensure_client_identity(state.path(), SystemTime::now())
            .unwrap()
            .cert_der;
        let db = Rc::new(RefCell::new(Db::default()));
        let seen = Rc::new(RefCell::new(Seen::default()));
        let runner = {
            let (db, seen, client_der, profile) = (
                db.clone(),
                seen.clone(),
                client_der.clone(),
                profile.clone(),
            );
            FakeRunner::new(move |spec| handle(spec, &db, &seen, &client_der, &profile))
        };
        Rig {
            home,
            state,
            db,
            seen,
            runner,
            profile,
            client_der,
        }
    }

    fn handle(
        spec: &CommandSpec,
        db: &Rc<RefCell<Db>>,
        seen: &Rc<RefCell<Seen>>,
        client_der: &[u8],
        profile: &Path,
    ) -> Result<CommandOutput, RunError> {
        let first = spec.args.first().map(|a| a.to_string_lossy().into_owned());
        let first = first.as_deref().unwrap_or("");
        for flag in ["-w", "-i", "-k", "-f"] {
            if let Some(path) = arg(spec, flag) {
                if let Ok(content) = fs::read(&path) {
                    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                    seen.borrow_mut()
                        .files
                        .insert(PathBuf::from(path), (content, mode));
                }
            }
        }
        let is_write = spec.program == "pk12util" || matches!(first, "-A" | "-D" | "-F");
        if is_write && seen.borrow().backup_existed_at_first_write.is_none() {
            let has_backup = fs::read_dir(profile).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("claude-monitor-backup-")
            });
            seen.borrow_mut().backup_existed_at_first_write = Some(has_backup);
        }
        if seen.borrow().hang {
            return Err(RunError::Timeout {
                program: spec.program.clone(),
                after: Duration::from_secs(30),
            });
        }
        let mut db = db.borrow_mut();
        if spec.program == "pk12util" {
            if seen.borrow().pk12_error {
                return failed(17, "pk12util: PKCS12 decode failed\n");
            }
            db.client = Some(client_der.to_vec());
            return ok(b"");
        }
        match first {
            "-K" if db.locked => failed(255, "SEC_ERROR_BAD_PASSWORD\n"),
            "-K" => failed(255, "certutil: no keys found\n"),
            "-L" => {
                let nick = arg(spec, "-n").unwrap();
                let stored = if nick == CA_NICKNAME {
                    &db.ca
                } else {
                    assert_eq!(nick, CLIENT_NICKNAME);
                    &db.client
                };
                match stored {
                    Some(der) => ok(&pem(der)),
                    None => failed(255, "certutil: Could not find cert: x\n"),
                }
            }
            "-A" => {
                let file = fs::read(arg(spec, "-i").unwrap()).unwrap();
                let (_, parsed) = x509_parser::pem::parse_x509_pem(&file).unwrap();
                db.ca = Some(parsed.contents);
                ok(b"")
            }
            "-D" => {
                db.ca = None;
                ok(b"")
            }
            "-F" => {
                db.client = None;
                ok(b"")
            }
            other => panic!("unexpected certutil {other}"),
        }
    }

    fn opts() -> Options {
        Options {
            browsers: vec![Browser::Firefox],
            ..Options::default()
        }
    }

    fn go(rig: &Rig, opts: &Options) -> Result<Report, SetupError> {
        let roots = Roots::new(rig.home.path(), None);
        run(
            &Env {
                runner: &rig.runner,
                roots: &roots,
                state_dir: rig.state.path(),
                now: SystemTime::now(),
            },
            opts,
        )
    }

    fn outcome(report: &Report) -> &Outcome {
        assert_eq!(report.targets.len(), 1, "{report:?}");
        &report.targets[0].outcome
    }

    /// `program flag` for each call, which is the order of operations.
    fn ops(rig: &Rig) -> Vec<String> {
        rig.runner
            .argv()
            .iter()
            .map(|a| format!("{} {}", a[0], a[1]))
            .collect()
    }

    fn backups(rig: &Rig) -> Vec<String> {
        fs::read_dir(&rig.profile)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("claude-monitor-backup-"))
            .collect()
    }

    #[test]
    fn a_first_run_trusts_the_ca_then_imports_the_client_in_that_order() {
        let rig = rig();

        let report = go(&rig, &opts()).unwrap();

        assert!(matches!(
            outcome(&report),
            Outcome::Imported {
                ca: true,
                client: true,
                auto_select: false,
                backup: Some(_)
            }
        ));
        assert_eq!(
            ops(&rig),
            vec![
                "certutil -K",
                "certutil -L",
                "certutil -L",
                "certutil -A",
                "pk12util -i"
            ]
        );
        assert!(report.all_fine());
    }

    #[test]
    fn a_second_run_does_nothing_and_says_so() {
        let rig = rig();
        go(&rig, &opts()).unwrap();
        let calls_after_first = rig.runner.calls.borrow().len();

        let report = go(&rig, &opts()).unwrap();

        assert_eq!(outcome(&report), &Outcome::UpToDate);
        let second: Vec<String> = ops(&rig).split_off(calls_after_first);
        assert_eq!(second, vec!["certutil -K", "certutil -L", "certutil -L"]);
        assert_eq!(backups(&rig).len(), 1, "no second backup for a no-op");
        assert!(report.text().contains("already up to date"));
        assert!(report.text().contains("nothing to do"));
    }

    #[test]
    fn a_changed_client_certificate_is_replaced_not_added_beside_the_old_one() {
        let rig = rig();
        go(&rig, &opts()).unwrap();
        rig.db.borrow_mut().client = Some(vec![1, 2, 3]);
        let before = rig.runner.calls.borrow().len();

        let report = go(&rig, &opts()).unwrap();

        assert!(matches!(
            outcome(&report),
            Outcome::Imported {
                ca: false,
                client: true,
                ..
            }
        ));
        let second: Vec<String> = ops(&rig).split_off(before);
        assert_eq!(
            second,
            vec![
                "certutil -K",
                "certutil -L",
                "certutil -L",
                "certutil -L",
                "certutil -F",
                "certutil -L",
                "pk12util -i"
            ]
        );
        assert_eq!(rig.db.borrow().client.as_deref(), Some(&rig.client_der[..]));
    }

    #[test]
    fn a_changed_ca_is_replaced_and_the_client_left_alone() {
        let rig = rig();
        go(&rig, &opts()).unwrap();
        rig.db.borrow_mut().ca = Some(vec![9, 9, 9]);
        let before = rig.runner.calls.borrow().len();

        go(&rig, &opts()).unwrap();

        let second: Vec<String> = ops(&rig).split_off(before);
        assert!(second.contains(&"certutil -D".to_owned()), "{second:?}");
        assert!(second.contains(&"certutil -A".to_owned()));
        assert!(!second.iter().any(|o| o.starts_with("pk12util")));
    }

    #[test]
    fn no_password_is_on_a_command_line_and_the_secret_files_are_private_and_then_gone() {
        let rig = rig();
        let tls_dir = rig.state.path().join("tls");
        let before: Vec<_> = fs::read_dir(&tls_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();

        go(&rig, &opts()).unwrap();

        let seen = rig.seen.borrow();
        let password_files: Vec<_> = seen
            .files
            .iter()
            .filter(|(p, _)| {
                p.starts_with(&tls_dir)
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("browser-setup-"))
            })
            .collect();
        assert_eq!(
            password_files.len(),
            2,
            "bundle and its password: {seen:?}",
            seen = password_files
        );
        for (path, (content, mode)) in &password_files {
            assert_eq!(*mode, 0o600, "{}", path.display());
            let text = String::from_utf8_lossy(content).into_owned();
            for call in rig.runner.argv() {
                assert!(
                    !call
                        .iter()
                        .any(|a| !text.is_empty() && text.len() > 8 && a.contains(&text)),
                    "a secret appears on a command line: {call:?}"
                );
            }
        }
        let after: Vec<_> = fs::read_dir(&tls_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before, after, "temp files left behind");
    }

    #[test]
    fn the_secret_files_are_removed_when_the_import_fails() {
        let rig = rig();
        rig.seen.borrow_mut().pk12_error = true;
        let tls_dir = rig.state.path().join("tls");
        let before = fs::read_dir(&tls_dir).unwrap().count();

        let report = go(&rig, &opts()).unwrap();

        let Outcome::Failed(why) = outcome(&report) else {
            panic!("{report:?}")
        };
        assert!(why.contains("decode failed"), "{why}");
        assert!(!report.all_fine());
        assert_eq!(fs::read_dir(&tls_dir).unwrap().count(), before);
    }

    #[test]
    fn a_tool_that_times_out_fails_the_target_instead_of_hanging_the_run() {
        let rig = rig();
        rig.seen.borrow_mut().hang = true;

        let report = go(&rig, &opts()).unwrap();

        let Outcome::Failed(why) = outcome(&report) else {
            panic!("{report:?}")
        };
        assert!(why.contains("did not finish"), "{why}");
    }

    #[test]
    fn a_profile_in_use_is_refused_untouched_unless_forced() {
        let rig = rig();
        std::os::unix::fs::symlink(
            format!("127.0.1.1:+{}", std::process::id()),
            rig.profile.join("lock"),
        )
        .unwrap();

        let report = go(&rig, &opts()).unwrap();

        assert!(matches!(outcome(&report), Outcome::InUse(_)));
        assert!(rig.runner.calls.borrow().is_empty(), "no tool ran");
        assert!(backups(&rig).is_empty());
        assert!(report.text().contains("refused: in use"));

        let forced = go(
            &rig,
            &Options {
                force: true,
                ..opts()
            },
        )
        .unwrap();
        assert!(matches!(outcome(&forced), Outcome::Imported { .. }));
    }

    #[test]
    fn a_primary_password_skips_the_profile_before_any_write() {
        let rig = rig();
        rig.db.borrow_mut().locked = true;

        let report = go(&rig, &opts()).unwrap();

        let Outcome::Skipped(why) = outcome(&report) else {
            panic!("{report:?}")
        };
        assert!(why.contains("primary password"), "{why}");
        assert_eq!(ops(&rig), vec!["certutil -K"]);
        assert!(backups(&rig).is_empty());
        assert!(!report.all_fine());
    }

    #[test]
    fn the_backup_exists_before_the_first_write_and_is_made_once() {
        let rig = rig();

        go(&rig, &opts()).unwrap();

        assert_eq!(rig.seen.borrow().backup_existed_at_first_write, Some(true));
        let names = backups(&rig);
        assert_eq!(names.len(), 1);
        let stamp = names[0].strip_prefix("claude-monitor-backup-").unwrap();
        assert_eq!(stamp.len(), "20261013T090507Z".len(), "{stamp}");
        assert!(stamp.ends_with('Z') && stamp.contains('T'));
        let dir = rig.profile.join(&names[0]);
        assert_eq!(fs::read(dir.join("cert9.db")).unwrap(), b"cert-db");
        assert_eq!(fs::read(dir.join("key4.db")).unwrap(), b"key-db");
    }

    #[test]
    fn a_dry_run_reads_but_writes_nothing_anywhere() {
        let rig = rig();
        let snapshot = |root: &Path| -> Vec<(PathBuf, u64)> {
            let mut out = Vec::new();
            let mut stack = vec![root.to_owned()];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        stack.push(entry.path());
                    }
                    out.push((entry.path(), meta.len()));
                }
            }
            out.sort();
            out
        };
        let (home_before, state_before) = (snapshot(rig.home.path()), snapshot(rig.state.path()));

        let report = go(
            &rig,
            &Options {
                dry_run: true,
                auto_select: true,
                ..opts()
            },
        )
        .unwrap();

        assert!(matches!(
            outcome(&report),
            Outcome::WouldImport {
                ca: true,
                client: true,
                auto_select: true
            }
        ));
        assert!(report.text().contains("would import"));
        assert_eq!(snapshot(rig.home.path()), home_before);
        assert_eq!(snapshot(rig.state.path()), state_before);
        assert!(!ops(&rig).iter().any(|o| matches!(
            o.as_str(),
            "certutil -A" | "certutil -D" | "certutil -F" | "pk12util -i"
        )));
    }

    #[test]
    fn a_dry_run_with_no_tls_material_yet_creates_none() {
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let ff = home.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("p")).unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[Profile0]\nName=p\nIsRelative=1\nPath=p\n",
        )
        .unwrap();
        fs::write(ff.join("p/cert9.db"), "x").unwrap();
        let runner = FakeRunner::new(|spec| {
            if spec.args.first().is_some_and(|a| a == "-L") {
                failed(255, "certutil: Could not find cert: x\n")
            } else {
                failed(255, "certutil: no keys found\n")
            }
        });
        let roots = Roots::new(home.path(), None);

        let report = run(
            &Env {
                runner: &runner,
                roots: &roots,
                state_dir: state.path(),
                now: SystemTime::now(),
            },
            &Options {
                dry_run: true,
                ..opts()
            },
        )
        .unwrap();

        assert!(matches!(
            report.targets[0].outcome,
            Outcome::WouldImport {
                ca: true,
                client: true,
                ..
            }
        ));
        assert_eq!(fs::read_dir(state.path()).unwrap().count(), 0);
    }

    #[test]
    fn auto_select_writes_the_block_only_when_asked_and_only_once() {
        let rig = rig();
        let user_js = rig.profile.join("user.js");

        go(&rig, &opts()).unwrap();
        assert!(!user_js.exists(), "off unless --auto-select");

        let report = go(
            &rig,
            &Options {
                auto_select: true,
                ..opts()
            },
        )
        .unwrap();
        assert!(matches!(
            outcome(&report),
            Outcome::Imported {
                ca: false,
                client: false,
                auto_select: true,
                backup: None
            }
        ));
        let written = fs::read_to_string(&user_js).unwrap();
        assert!(written.contains(userjs::BEGIN) && written.contains("Select Automatically"));
        assert!(
            !rig.profile.join("prefs.js").exists(),
            "prefs.js is never created"
        );

        let again = go(
            &rig,
            &Options {
                auto_select: true,
                ..opts()
            },
        )
        .unwrap();
        assert_eq!(outcome(&again), &Outcome::UpToDate);
        assert_eq!(fs::read_to_string(&user_js).unwrap(), written);
    }

    #[test]
    fn auto_select_keeps_the_users_own_lines_and_never_edits_prefs_js() {
        let rig = rig();
        let user_js = rig.profile.join("user.js");
        fs::write(&user_js, "user_pref(\"mine\", 1);\n").unwrap();
        fs::write(rig.profile.join("prefs.js"), "// prefs\n").unwrap();

        go(
            &rig,
            &Options {
                auto_select: true,
                ..opts()
            },
        )
        .unwrap();

        let written = fs::read_to_string(&user_js).unwrap();
        assert!(written.starts_with("user_pref(\"mine\", 1);\n"));
        assert_eq!(
            fs::read_to_string(rig.profile.join("prefs.js")).unwrap(),
            "// prefs\n"
        );
    }

    #[test]
    fn chromium_gets_the_policy_text_and_no_user_js() {
        let rig = rig();
        let nssdb = rig.home.path().join(".pki/nssdb");
        fs::create_dir_all(&nssdb).unwrap();
        fs::write(nssdb.join("cert9.db"), "x").unwrap();

        let report = go(
            &rig,
            &Options {
                browsers: vec![Browser::Chromium],
                auto_select: true,
                ..Options::default()
            },
        )
        .unwrap();

        assert!(!nssdb.join("user.js").exists());
        assert!(report
            .chromium_policy
            .as_deref()
            .unwrap()
            .contains("AutoSelectCertificateForUrls"));
        let without = go(
            &rig,
            &Options {
                browsers: vec![Browser::Chromium],
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(without.chromium_policy, None);
    }

    #[test]
    fn no_profiles_at_all_is_a_precondition_failure() {
        let rig = rig();
        let err = go(
            &rig,
            &Options {
                browsers: vec![Browser::Zen],
                ..Options::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, SetupError::NoTargets), "{err}");
    }

    #[test]
    fn a_profile_that_has_not_made_its_database_yet_is_left_alone() {
        let rig = rig();
        fs::remove_file(rig.profile.join("cert9.db")).unwrap();

        let report = go(&rig, &opts()).unwrap();

        assert_eq!(outcome(&report), &Outcome::NoDatabase);
        assert!(report.all_fine());
        assert!(rig.runner.calls.borrow().is_empty());
    }
}
