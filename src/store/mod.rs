use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

pub const DB_FILE_NAME: &str = "state.db";
pub const APP_DIR_NAME: &str = "ai-control-centre";

const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const VIEWER_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_BUSY_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot {operation}: not implemented")]
    NotImplemented { operation: &'static str },
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
            Self::NotImplemented { .. } | Self::NoStateDir => None,
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
            Self::NotImplemented { operation } => operation,
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

pub fn resolve_state_dir(
    _override_dir: Option<&Path>,
    _env: &StateDirEnv,
) -> Result<PathBuf, StoreError> {
    Err(StoreError::NotImplemented {
        operation: "resolve state directory",
    })
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path, _opts: &StoreOptions) -> Result<Self, StoreError> {
        let _ = path;
        Err(StoreError::NotImplemented {
            operation: "open database",
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
        Err(StoreError::NotImplemented {
            operation: "read schema version",
        })
    }
}
