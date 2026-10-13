//! `claude-monitor web ...`: the CA, login and client-certificate helpers for
//! the web view.

use std::path::Path;

use super::command::WebAction;
use super::Output;
use crate::error::Error;

#[cfg(feature = "web")]
pub fn run(state_dir: Option<&Path>, action: WebAction) -> Result<Output, Error> {
    enabled::run(state_dir, action)
}

#[cfg(not(feature = "web"))]
pub fn run(_state_dir: Option<&Path>, _action: WebAction) -> Result<Output, Error> {
    Err(Error::Usage(
        "web needs the `web` feature; rebuild with `--features web`".to_owned(),
    ))
}

#[cfg(feature = "web")]
mod enabled {
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use serde_json::json;

    use crate::browser_setup::discover::{Browser, Roots};
    use crate::browser_setup::runner::{self, SystemRunner};
    use crate::browser_setup::setup::{self, Env, Options, SetupError};
    use crate::cli::command::{BrowserChoice, WebAction};
    use crate::cli::migrate::store_error;
    use crate::cli::Output;
    use crate::error::Error;
    use crate::store::{resolve_state_dir, StateDirEnv};
    use crate::web::{control, secret, tls};

    const TRUST_HINT: &str = "Import it once as a certificate authority: Firefox: Settings > \
        Privacy & Security > Certificates > View Certificates > Authorities > Import; \
        Chromium: Settings > Privacy and security > Security > Manage certificates > \
        Authorities > Import. It is limited to localhost, 127.0.0.0/8 and ::1.";

    const CLIENT_HINT: &str = "Import it in the browser, then start the server with --mtls. \
        Firefox: Settings > Privacy & Security > Certificates > View Certificates > \
        Your Certificates > Import. Chromium: Settings > Privacy and security > Security > \
        Manage certificates > Your certificates > Import.";

    fn failed(operation: &str, source: impl std::fmt::Display) -> Error {
        Error::Io {
            operation: operation.to_owned(),
            path: None,
            source: std::io::Error::other(source.to_string()),
        }
    }

    fn state_dir_of(override_dir: Option<&Path>) -> Result<PathBuf, Error> {
        resolve_state_dir(override_dir, &StateDirEnv::from_process()).map_err(|e| store_error(&e))
    }

    pub fn run(state_dir: Option<&Path>, action: WebAction) -> Result<Output, Error> {
        let dir = state_dir_of(state_dir)?;
        match action {
            WebAction::Ca { print } => ca(&dir, print),
            WebAction::Login => login(&dir),
            WebAction::ClientCert { out, password_file } => {
                client_cert(&dir, out, password_file.as_deref())
            }
            WebAction::SetupBrowser {
                browser,
                profiles,
                auto_select,
                force,
                dry_run,
            } => setup_browser(
                &dir,
                browsers_of(browser),
                profiles,
                auto_select,
                force,
                dry_run,
            ),
        }
    }

    fn browsers_of(choice: BrowserChoice) -> Vec<Browser> {
        match choice {
            BrowserChoice::Firefox => vec![Browser::Firefox],
            BrowserChoice::Zen => vec![Browser::Zen],
            BrowserChoice::Chromium => vec![Browser::Chromium],
            BrowserChoice::All => vec![Browser::Firefox, Browser::Zen, Browser::Chromium],
        }
    }

    fn setup_browser(
        dir: &Path,
        browsers: Vec<Browser>,
        profiles: Vec<PathBuf>,
        auto_select: bool,
        force: bool,
        dry_run: bool,
    ) -> Result<Output, Error> {
        let missing = runner::missing_tools(&std::env::var_os("PATH").unwrap_or_default());
        if !missing.is_empty() {
            return Err(Error::Refused {
                code: "nss_tools_missing",
                message: format!(
                    "{} not found on PATH; install the NSS tools, for example with `{}`",
                    missing.join(" and "),
                    runner::NSS_INSTALL_HINT
                ),
            });
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|h| h.is_absolute())
            .ok_or_else(|| Error::Config("HOME is not set to an absolute path".to_owned()))?;
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let roots = Roots::new(&home, xdg.as_deref());
        let env = Env {
            runner: &SystemRunner,
            roots: &roots,
            state_dir: dir,
            now: SystemTime::now(),
        };
        let opts = Options {
            browsers,
            profiles,
            auto_select,
            force,
            dry_run,
        };
        let report = setup::run(&env, &opts).map_err(|e| match e {
            SetupError::NoTargets => Error::Refused {
                code: "no_targets",
                message: e.to_string(),
            },
            SetupError::Tls(_) => failed("prepare the TLS material", e),
        })?;
        if !report.all_fine() {
            return Err(Error::Partial {
                operation: "setup-browser".to_owned(),
                detail: format!("some targets were not set up\n{}", report.text()),
            });
        }
        Ok(Output::Dual {
            text: report.text(),
            json: report.json(),
        })
    }

    fn client_cert(
        dir: &Path,
        out: Option<PathBuf>,
        password_file: Option<&Path>,
    ) -> Result<Output, Error> {
        let now = SystemTime::now();
        let paths = tls::ensure(dir, now).map_err(|e| failed("prepare the CA", e))?;
        let (password, generated) = match password_file {
            Some(file) => (read_password(file)?, false),
            None => (
                secret::random_hex().map_err(|e| failed("make a password", e))?,
                true,
            ),
        };
        let identity = tls::issue_client_identity(dir, now)
            .map_err(|e| failed("issue a client certificate", e))?;
        let bundle = tls::client_pkcs12(&identity, &password)
            .map_err(|e| failed("build the client bundle", e))?;
        let out = out.unwrap_or_else(|| paths.dir.join("client.p12"));
        tls::write_private(&out, &bundle).map_err(|e| failed("write the client bundle", e))?;
        let path = out.display().to_string();
        let mut text = format!("{path}\n");
        if generated {
            text.push_str(&format!("password: {}\n", password.as_str()));
        }
        text.push_str(CLIENT_HINT);
        let json = if generated {
            json!({ "path": path, "password": password.as_str() })
        } else {
            json!({ "path": path })
        };
        Ok(Output::Dual { text, json })
    }

    fn read_password(file: &Path) -> Result<zeroize::Zeroizing<String>, Error> {
        let raw =
            zeroize::Zeroizing::new(std::fs::read_to_string(file).map_err(|source| Error::Io {
                operation: "read the password file".to_owned(),
                path: Some(file.to_owned()),
                source,
            })?);
        let password = raw.trim_end_matches(['\n', '\r']);
        if password.is_empty() {
            return Err(Error::Usage(format!(
                "the password file {} is empty",
                file.display()
            )));
        }
        Ok(zeroize::Zeroizing::new(password.to_owned()))
    }

    fn login(dir: &Path) -> Result<Output, Error> {
        let url =
            control::request_login_url(dir).map_err(|e| failed("ask serve for a login URL", e))?;
        Ok(Output::Dual {
            text: url.as_str().to_owned(),
            json: json!({ "url": url.as_str() }),
        })
    }

    fn ca(dir: &Path, print: bool) -> Result<Output, Error> {
        let paths = tls::ensure(dir, SystemTime::now()).map_err(|e| failed("prepare the CA", e))?;
        if print {
            let pem = std::fs::read_to_string(&paths.ca_cert)
                .map_err(|e| failed("read the CA certificate", e))?;
            return Ok(Output::Raw(pem));
        }
        let path = paths.ca_cert.display().to_string();
        Ok(Output::Dual {
            text: format!("{path}\n{TRUST_HINT}"),
            json: json!({ "path": path, "hint": TRUST_HINT }),
        })
    }
}
