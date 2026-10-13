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

    use crate::cli::command::WebAction;
    use crate::cli::migrate::store_error;
    use crate::cli::Output;
    use crate::error::Error;
    use crate::store::{resolve_state_dir, StateDirEnv};
    use crate::web::{control, tls};

    const TRUST_HINT: &str = "Import it once as a certificate authority: Firefox: Settings > \
        Privacy & Security > Certificates > View Certificates > Authorities > Import; \
        Chromium: Settings > Privacy and security > Security > Manage certificates > \
        Authorities > Import. It is limited to localhost, 127.0.0.0/8 and ::1.";

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
        }
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
