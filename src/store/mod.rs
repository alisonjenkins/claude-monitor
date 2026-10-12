use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

pub mod events;
pub mod legacy;
pub mod markers;
mod migrations;
pub mod progress;
pub mod projects;
pub mod queries;
pub mod respawn;
mod schema;
pub mod sessions;
pub mod summaries;

pub use migrations::SCHEMA_VERSION;

pub const DB_FILE_NAME: &str = "state.db";
pub const APP_DIR_NAME: &str = "ai-control-centre";

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const VIEWER_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_BUSY_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot locate a state directory: neither XDG_STATE_HOME nor HOME is usable")]
    NoStateDir,
    #[error("cannot create state directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("cannot create database file {path}: {source}")]
    CreateFile { path: PathBuf, source: io::Error },
    #[error("cannot restrict permissions of {path}: {source}")]
    Permissions { path: PathBuf, source: io::Error },
    #[error("cannot open database {path}: {source}")]
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error("cannot set {pragma} on database {path}: {source}")]
    Configure {
        path: PathBuf,
        pragma: &'static str,
        source: rusqlite::Error,
    },
    #[error("cannot enable WAL journaling on database {path}: SQLite kept journal mode {mode}")]
    JournalMode { path: PathBuf, mode: String },
    #[error("cannot read schema version of database {path}: {source}")]
    ReadVersion {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error(
        "cannot use database {path}: schema version {found} is newer than the supported {supported}"
    )]
    NewerSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("cannot migrate database {path} to schema version {version}: {source}")]
    Migrate {
        path: PathBuf,
        version: u32,
        source: rusqlite::Error,
    },
}

impl StoreError {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::NoStateDir => None,
            Self::CreateDir { path, .. }
            | Self::CreateFile { path, .. }
            | Self::Permissions { path, .. }
            | Self::Open { path, .. }
            | Self::Configure { path, .. }
            | Self::JournalMode { path, .. }
            | Self::ReadVersion { path, .. }
            | Self::NewerSchema { path, .. }
            | Self::Migrate { path, .. } => Some(path),
        }
    }

    pub fn operation(&self) -> &'static str {
        match self {
            Self::NoStateDir => "locate state directory",
            Self::CreateDir { .. } => "create state directory",
            Self::CreateFile { .. } => "create database file",
            Self::Permissions { .. } => "restrict permissions",
            Self::Open { .. } => "open database",
            Self::Configure { .. } => "configure database",
            Self::JournalMode { .. } => "enable WAL journaling",
            Self::ReadVersion { .. } => "read schema version",
            Self::NewerSchema { .. } => "check schema version",
            Self::Migrate { .. } => "migrate database",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoreOptions {
    pub busy_timeout: Duration,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }
}

impl StoreOptions {
    pub fn viewer() -> Self {
        Self {
            busy_timeout: VIEWER_BUSY_TIMEOUT,
        }
    }

    pub fn hook() -> Self {
        Self {
            busy_timeout: HOOK_BUSY_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StateDirEnv {
    pub xdg_state_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl StateDirEnv {
    pub fn from_process() -> Self {
        Self {
            xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

/// `override_dir` is used as given. Otherwise `$XDG_STATE_HOME/ai-control-centre`
/// (ignored unless absolute, per the XDG spec), else the per-OS default under `$HOME`.
pub fn resolve_state_dir(
    override_dir: Option<&Path>,
    env: &StateDirEnv,
) -> Result<PathBuf, StoreError> {
    if let Some(dir) = override_dir {
        return Ok(dir.to_owned());
    }
    if let Some(xdg) = env.xdg_state_home.as_deref().filter(|p| p.is_absolute()) {
        return Ok(xdg.join(APP_DIR_NAME));
    }
    let home = env
        .home
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(StoreError::NoStateDir)?;
    if cfg!(target_os = "macos") {
        Ok(home
            .join("Library")
            .join("Application Support")
            .join(APP_DIR_NAME))
    } else {
        Ok(home.join(".local").join("state").join(APP_DIR_NAME))
    }
}

fn create_private_dir(dir: &Path) -> Result<(), StoreError> {
    if dir.is_dir() {
        return Ok(());
    }
    DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIR_MODE)
        .create(dir)
        .map_err(|source| StoreError::CreateDir {
            path: dir.to_owned(),
            source,
        })
}

fn ensure_private_file(path: &Path) -> Result<(), StoreError> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(|source| StoreError::CreateFile {
            path: path.to_owned(),
            source,
        })?;
    let permissions_error = |source| StoreError::Permissions {
        path: path.to_owned(),
        source,
    };
    let current = fs::metadata(path).map_err(permissions_error)?.permissions();
    if current.mode() & 0o777 != PRIVATE_FILE_MODE {
        fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
            .map_err(permissions_error)?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    conn: Connection,
}

impl Store {
    /// Creates the directory (0700) and file (0600) if missing, enables WAL and
    /// foreign keys, then migrates to the current schema. Refuses a database
    /// written by a newer schema version without touching it.
    pub fn open(path: &Path, opts: &StoreOptions) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            create_private_dir(parent)?;
        }
        ensure_private_file(path)?;

        let mut conn = Connection::open(path).map_err(|source| StoreError::Open {
            path: path.to_owned(),
            source,
        })?;
        let configure = |pragma: &'static str| {
            move |source| StoreError::Configure {
                path: path.to_owned(),
                pragma,
                source,
            }
        };
        conn.busy_timeout(opts.busy_timeout)
            .map_err(configure("busy_timeout"))?;

        let found = migrations::user_version(&conn).map_err(|source| StoreError::ReadVersion {
            path: path.to_owned(),
            source,
        })?;
        if found > SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                path: path.to_owned(),
                found,
                supported: SCHEMA_VERSION,
            });
        }

        let mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .map_err(configure("journal_mode"))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::JournalMode {
                path: path.to_owned(),
                mode,
            });
        }
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(configure("foreign_keys"))?;

        migrations::run(&mut conn, path)?;
        Ok(Self {
            path: path.to_owned(),
            conn,
        })
    }

    pub fn open_in_dir(dir: &Path, opts: &StoreOptions) -> Result<Self, StoreError> {
        Self::open(&dir.join(DB_FILE_NAME), opts)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        migrations::user_version(&self.conn).map_err(|source| StoreError::ReadVersion {
            path: self.path.clone(),
            source,
        })
    }
}
