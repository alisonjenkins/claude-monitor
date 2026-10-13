//! The NSS database operations `setup-browser` needs, as `certutil` and
//! `pk12util` invocations. Passwords and the bundle travel in 0600 files named
//! with `-w`/`-k`/`-f`, never in an argument.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::runner::{CommandOutput, CommandRunner, CommandSpec, RunError};

pub const CA_NICKNAME: &str = "claude-monitor local CA";
/// The bundle carries no friendly name NSS honours, so `pk12util` names the
/// imported certificate after its subject.
pub const CLIENT_NICKNAME: &str = "CN=claude-monitor web client";

/// `certutil -N` plus a few `-A`/`-F` calls take well under a second; this only
/// bounds a tool that is stuck on a lock.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// Importing the same nickname repeatedly can leave `nick`, `nick #2`, ...;
/// this bounds how many a removal sweeps.
const MAX_DUPLICATES: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum NssError {
    #[error("{operation} in {dir}: {source}")]
    Run {
        operation: &'static str,
        dir: PathBuf,
        source: RunError,
    },
    #[error("{operation} in {dir} failed (exit {code}): {stderr}")]
    Tool {
        operation: &'static str,
        dir: PathBuf,
        code: String,
        stderr: String,
    },
    #[error("the certificate stored as {nickname:?} in {dir} is not readable")]
    UnreadableCertificate {
        nickname: &'static str,
        dir: PathBuf,
    },
    #[error("{nickname:?} in {dir} cannot be removed after {MAX_DUPLICATES} attempts")]
    Stuck {
        nickname: &'static str,
        dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAccess {
    Open,
    /// A primary password protects the keys; `certutil` would prompt for it.
    PrimaryPassword,
}

pub struct Nss<'a> {
    runner: &'a dyn CommandRunner,
    /// A file holding an empty line: the answer for a database with no
    /// primary password, so a tool never falls back to prompting.
    empty_password: &'a Path,
}

fn db_arg(dir: &Path) -> OsString {
    let mut arg = OsString::from("sql:");
    arg.push(dir);
    arg
}

fn args(parts: &[&std::ffi::OsStr]) -> Vec<OsString> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

impl<'a> Nss<'a> {
    pub fn new(runner: &'a dyn CommandRunner, empty_password: &'a Path) -> Self {
        Self {
            runner,
            empty_password,
        }
    }

    fn run(
        &self,
        operation: &'static str,
        dir: &Path,
        program: &str,
        args: Vec<OsString>,
    ) -> Result<CommandOutput, NssError> {
        self.runner
            .run(&CommandSpec {
                program: program.to_owned(),
                args,
                timeout: TOOL_TIMEOUT,
            })
            .map_err(|source| NssError::Run {
                operation,
                dir: dir.to_owned(),
                source,
            })
    }

    fn require_success(
        operation: &'static str,
        dir: &Path,
        out: CommandOutput,
    ) -> Result<CommandOutput, NssError> {
        if out.success() {
            return Ok(out);
        }
        Err(NssError::Tool {
            operation,
            dir: dir.to_owned(),
            code: out
                .code
                .map_or_else(|| "signal".to_owned(), |c| c.to_string()),
            stderr: out.stderr.trim().to_owned(),
        })
    }

    /// Whether the keys can be reached without a password. Listing keys with
    /// an empty password is the cheapest call that fails differently for a
    /// protected database.
    pub fn access(&self, dir: &Path) -> Result<DbAccess, NssError> {
        let out = self.run(
            "check the primary password",
            dir,
            "certutil",
            args(&[
                "-K".as_ref(),
                "-d".as_ref(),
                &db_arg(dir),
                "-f".as_ref(),
                self.empty_password.as_os_str(),
            ]),
        )?;
        let said = |needle: &str| {
            out.stderr.contains(needle) || String::from_utf8_lossy(&out.stdout).contains(needle)
        };
        if said("SEC_ERROR_BAD_PASSWORD") {
            return Ok(DbAccess::PrimaryPassword);
        }
        // "no keys found" also exits non-zero on a healthy empty database.
        if out.success() || said("no keys found") {
            return Ok(DbAccess::Open);
        }
        Self::require_success("check the primary password", dir, out).map(|_| DbAccess::Open)
    }

    /// The DER of the certificate stored as `nickname`, if there is one.
    pub fn stored_der(
        &self,
        dir: &Path,
        nickname: &'static str,
    ) -> Result<Option<Vec<u8>>, NssError> {
        let out = self.run(
            "read a stored certificate",
            dir,
            "certutil",
            args(&[
                "-L".as_ref(),
                "-d".as_ref(),
                &db_arg(dir),
                "-n".as_ref(),
                nickname.as_ref(),
                "-a".as_ref(),
            ]),
        )?;
        if !out.success() && out.stderr.contains("Could not find cert") {
            return Ok(None);
        }
        let out = Self::require_success("read a stored certificate", dir, out)?;
        let (_, pem) = x509_parser::pem::parse_x509_pem(&out.stdout).map_err(|_| {
            NssError::UnreadableCertificate {
                nickname,
                dir: dir.to_owned(),
            }
        })?;
        Ok(Some(pem.contents))
    }

    /// Trusts the CA at `ca_pem` to identify web sites.
    pub fn trust_ca(&self, dir: &Path, ca_pem: &Path) -> Result<(), NssError> {
        let out = self.run(
            "trust the CA",
            dir,
            "certutil",
            args(&[
                "-A".as_ref(),
                "-d".as_ref(),
                &db_arg(dir),
                "-n".as_ref(),
                CA_NICKNAME.as_ref(),
                "-t".as_ref(),
                "C,,".as_ref(),
                "-a".as_ref(),
                "-i".as_ref(),
                ca_pem.as_os_str(),
                "-f".as_ref(),
                self.empty_password.as_os_str(),
            ]),
        )?;
        Self::require_success("trust the CA", dir, out).map(|_| ())
    }

    /// Removes every certificate (and, for the client, its key) stored under
    /// `nickname`, so a changed one is replaced rather than added beside it.
    pub fn remove(
        &self,
        dir: &Path,
        nickname: &'static str,
        with_key: bool,
    ) -> Result<(), NssError> {
        for _ in 0..MAX_DUPLICATES {
            if self.stored_der(dir, nickname)?.is_none() {
                return Ok(());
            }
            let flag = if with_key { "-F" } else { "-D" };
            let out = self.run(
                "remove a stored certificate",
                dir,
                "certutil",
                args(&[
                    flag.as_ref(),
                    "-d".as_ref(),
                    &db_arg(dir),
                    "-n".as_ref(),
                    nickname.as_ref(),
                    "-f".as_ref(),
                    self.empty_password.as_os_str(),
                ]),
            )?;
            Self::require_success("remove a stored certificate", dir, out)?;
        }
        match self.stored_der(dir, nickname)? {
            None => Ok(()),
            Some(_) => Err(NssError::Stuck {
                nickname,
                dir: dir.to_owned(),
            }),
        }
    }

    /// Imports the PKCS#12 bundle; `bundle_password` names the file that holds
    /// its password.
    pub fn import_client(
        &self,
        dir: &Path,
        bundle: &Path,
        bundle_password: &Path,
    ) -> Result<(), NssError> {
        let out = self.run(
            "import the client certificate",
            dir,
            "pk12util",
            args(&[
                "-i".as_ref(),
                bundle.as_os_str(),
                "-d".as_ref(),
                &db_arg(dir),
                "-w".as_ref(),
                bundle_password.as_os_str(),
                "-k".as_ref(),
                self.empty_password.as_os_str(),
            ]),
        )?;
        Self::require_success("import the client certificate", dir, out).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::runner::fake::{failed, ok, FakeRunner};
    use super::*;

    const DIR: &str = "/profiles/abc.default";
    const EMPTY: &str = "/run/empty-pw";

    fn dir() -> &'static Path {
        Path::new(DIR)
    }

    fn nss(runner: &FakeRunner) -> Nss<'_> {
        Nss::new(runner, Path::new(EMPTY))
    }

    #[test]
    fn access_is_open_for_an_empty_database_and_a_populated_one() {
        let populated = FakeRunner::new(|_| ok(b""));
        let empty = FakeRunner::new(|_| failed(255, "certutil: no keys found\n"));
        assert_eq!(nss(&populated).access(dir()).unwrap(), DbAccess::Open);
        assert_eq!(nss(&empty).access(dir()).unwrap(), DbAccess::Open);
    }

    #[test]
    fn access_detects_a_primary_password_without_prompting() {
        let runner = FakeRunner::new(|_| {
            failed(
                255,
                "Incorrect password/PIN entered.\ncertutil: could not authenticate: SEC_ERROR_BAD_PASSWORD",
            )
        });

        assert_eq!(
            nss(&runner).access(dir()).unwrap(),
            DbAccess::PrimaryPassword
        );
        assert_eq!(
            runner.argv(),
            vec![vec![
                "certutil",
                "-K",
                "-d",
                "sql:/profiles/abc.default",
                "-f",
                EMPTY
            ]]
        );
    }

    #[test]
    fn access_reports_other_failures_instead_of_guessing() {
        let runner = FakeRunner::new(|_| failed(255, "SEC_ERROR_BAD_DATABASE"));
        let err = nss(&runner).access(dir()).unwrap_err();
        assert!(err.to_string().contains("SEC_ERROR_BAD_DATABASE"), "{err}");
    }

    #[test]
    fn stored_der_is_none_when_certutil_cannot_find_the_nickname() {
        let runner = FakeRunner::new(|_| failed(255, "certutil: Could not find cert: nope\n"));
        assert_eq!(nss(&runner).stored_der(dir(), CA_NICKNAME).unwrap(), None);
        assert_eq!(
            runner.argv(),
            vec![vec![
                "certutil",
                "-L",
                "-d",
                "sql:/profiles/abc.default",
                "-n",
                "claude-monitor local CA",
                "-a"
            ]]
        );
    }

    #[test]
    fn stored_der_decodes_the_pem_certutil_prints() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
        let runner = FakeRunner::new(move |_| ok(pem.as_bytes()));
        let got = nss(&runner).stored_der(dir(), CA_NICKNAME).unwrap();
        assert_eq!(got, Some(vec![1, 2, 3]), "AQID is base64 for 01 02 03");
    }

    #[test]
    fn trust_ca_marks_it_for_tls_and_passes_no_secret() {
        let runner = FakeRunner::new(|_| ok(b""));

        nss(&runner)
            .trust_ca(dir(), Path::new("/state/tls/ca.pem"))
            .unwrap();

        assert_eq!(
            runner.argv(),
            vec![vec![
                "certutil",
                "-A",
                "-d",
                "sql:/profiles/abc.default",
                "-n",
                "claude-monitor local CA",
                "-t",
                "C,,",
                "-a",
                "-i",
                "/state/tls/ca.pem",
                "-f",
                EMPTY
            ]]
        );
    }

    #[test]
    fn import_client_names_the_password_file_never_the_password() {
        let runner = FakeRunner::new(|_| ok(b""));

        nss(&runner)
            .import_client(dir(), Path::new("/t/bundle.p12"), Path::new("/t/pw"))
            .unwrap();

        assert_eq!(
            runner.argv(),
            vec![vec![
                "pk12util",
                "-i",
                "/t/bundle.p12",
                "-d",
                "sql:/profiles/abc.default",
                "-w",
                "/t/pw",
                "-k",
                EMPTY
            ]]
        );
    }

    #[test]
    fn a_failing_tool_reports_its_exit_code_and_stderr() {
        let runner = FakeRunner::new(|_| failed(17, "pk12util: PKCS12 decode failed\n"));
        let err = nss(&runner)
            .import_client(dir(), Path::new("/t/b.p12"), Path::new("/t/pw"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("exit 17") && err.contains("decode failed"),
            "{err}"
        );
    }

    #[test]
    fn every_call_has_a_timeout() {
        let runner = FakeRunner::new(|_| ok(b""));
        nss(&runner)
            .trust_ca(dir(), Path::new("/state/tls/ca.pem"))
            .unwrap();
        assert!(runner
            .calls
            .borrow()
            .iter()
            .all(|c| c.timeout > Duration::ZERO && c.timeout <= Duration::from_secs(60)));
    }

    #[test]
    fn remove_deletes_until_the_nickname_is_gone_and_takes_the_key_for_the_client() {
        use std::cell::Cell;
        let present = Cell::new(2u8);
        let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
        let runner = FakeRunner::new(move |spec| {
            let first = spec.args.first().map(|a| a.to_string_lossy().into_owned());
            match first.as_deref() {
                Some("-L") if present.get() == 0 => {
                    failed(255, "certutil: Could not find cert: x\n")
                }
                Some("-L") => ok(pem.as_bytes()),
                Some("-F") => {
                    present.set(present.get().saturating_sub(1));
                    ok(b"")
                }
                _ => failed(1, "unexpected"),
            }
        });

        nss(&runner).remove(dir(), CLIENT_NICKNAME, true).unwrap();

        let deletes = runner
            .argv()
            .into_iter()
            .filter(|a| a.get(1).map(String::as_str) == Some("-F"))
            .count();
        assert_eq!(deletes, 2);
    }

    #[test]
    fn remove_gives_up_when_a_certificate_keeps_coming_back() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
        let runner = FakeRunner::new(move |spec| {
            if spec.args.first().is_some_and(|a| a == "-L") {
                ok(pem.as_bytes())
            } else {
                ok(b"")
            }
        });

        let err = nss(&runner).remove(dir(), CA_NICKNAME, false).unwrap_err();

        assert!(matches!(err, NssError::Stuck { .. }), "{err}");
    }
}
