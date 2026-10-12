//! Provider traits for everything the app reads from or does to the outside
//! world, so library code is tested with fakes (see `crate::testing`) and no
//! process spawn.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use crate::tmux::PaneLocation;

/// Failure of a provider operation. Each variant names the operation so a log
/// line alone identifies what failed.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error("{operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{operation}: {detail}")]
    Failed {
        operation: &'static str,
        detail: String,
    },
    #[error("{operation}: not supported on this platform")]
    Unsupported { operation: &'static str },
}

pub trait Clock: Send + Sync {
    /// Current UTC time as RFC 3339 with second precision, e.g.
    /// `2026-10-12T14:03:22Z`.
    fn now(&self) -> String;
}

pub trait BootId: Send + Sync {
    /// Identifier that changes on every machine boot.
    fn current(&self) -> Result<String, EnvError>;
}

pub trait Processes: Send + Sync {
    /// True when `pid` exists and its start time is `start_time`; a recycled
    /// pid has a different start time.
    fn is_alive(&self, pid: u32, start_time: u64) -> bool;
    /// Start time of `pid` in the platform's unit, `None` when it is not running.
    fn start_time(&self, pid: u32) -> Result<Option<u64>, EnvError>;
}

pub trait Tmux: Send + Sync {
    /// True when a tmux binary can be run. `list_panes` cannot tell this from
    /// "no server running", which a respawn can fix by starting one.
    fn is_installed(&self) -> bool;
    /// Every pane keyed by pane id; `None` when tmux is unavailable.
    fn list_panes(&self) -> Result<Option<HashMap<String, PaneLocation>>, EnvError>;
    /// Create a detached session running `command` in `cwd`; returns the pane id.
    fn new_session(&self, name: &str, cwd: &Path, command: &str) -> Result<String, EnvError>;
    /// Create a detached window in `session` running `command` in `cwd`; returns the pane id.
    fn new_window(&self, session: &str, cwd: &Path, command: &str) -> Result<String, EnvError>;
    fn display_message(&self, message: &str) -> Result<(), EnvError>;
    /// Make sure the server is running and `session` exists, creating it
    /// detached in `cwd` when absent. `true` when this call created it.
    fn ensure_session(&self, session: &str, cwd: &Path) -> Result<bool, EnvError>;
    /// Open a detached window in `session`, named `label`, running `argv`
    /// directly (no shell) in `cwd`.
    fn spawn_window(
        &self,
        session: &str,
        label: &str,
        cwd: &Path,
        argv: &[String],
    ) -> Result<SpawnedWindow, EnvError>;
}

/// Ids of a window `Tmux::spawn_window` opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedWindow {
    /// Id of the window's only pane, e.g. `%5`.
    pub pane_id: String,
    /// Id of the window itself, e.g. `@3`.
    pub window_id: String,
}

pub trait Notifier: Send + Sync {
    fn send(&self, summary: &str, body: &str) -> Result<(), EnvError>;
}

/// Size and kind of a file, without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub len: u64,
    pub is_dir: bool,
    pub modified: Option<SystemTime>,
}

pub trait Fs: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError>;
    fn exists(&self, path: &Path) -> bool;
    /// Entries of a directory, sorted by path.
    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, EnvError>;
    fn read_link(&self, path: &Path) -> Result<PathBuf, EnvError>;
    fn metadata(&self, path: &Path) -> Result<FileMeta, EnvError>;
    /// Up to `max_len` bytes starting at `offset`; shorter at end of file.
    /// For files that may be too large to read whole.
    fn read_range(&self, path: &Path, offset: u64, max_len: usize) -> Result<Vec<u8>, EnvError>;
}

/// Process environment variables.
pub trait Vars: Send + Sync {
    /// The variable's value; `None` when unset, empty or not UTF-8.
    fn var(&self, name: &str) -> Option<String>;
}

/// The providers a command runs against.
#[derive(Clone)]
pub struct Env {
    pub clock: Arc<dyn Clock>,
    pub boot_id: Arc<dyn BootId>,
    pub processes: Arc<dyn Processes>,
    pub tmux: Arc<dyn Tmux>,
    pub notifier: Arc<dyn Notifier>,
    pub fs: Arc<dyn Fs>,
    pub vars: Arc<dyn Vars>,
}

impl Env {
    pub fn real() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            boot_id: Arc::new(SystemBootId),
            processes: Arc::new(SystemProcesses),
            tmux: Arc::new(SystemTmux::default()),
            notifier: Arc::new(SystemNotifier),
            fs: Arc::new(SystemFs),
            vars: Arc::new(SystemVars),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemVars;

impl Vars for SystemVars {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemBootId;

impl BootId for SystemBootId {
    #[cfg(target_os = "linux")]
    fn current(&self) -> Result<String, EnvError> {
        let path = Path::new("/proc/sys/kernel/random/boot_id");
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_owned())
            .map_err(|source| EnvError::Io {
                operation: "read boot id",
                path: path.to_owned(),
                source,
            })
    }

    #[cfg(target_os = "macos")]
    fn current(&self) -> Result<String, EnvError> {
        let out = Command::new("sysctl")
            .args(["-n", "kern.bootsessionuuid"])
            .output()
            .map_err(|e| EnvError::Failed {
                operation: "read boot id",
                detail: e.to_string(),
            })?;
        if !out.status.success() {
            return Err(EnvError::Failed {
                operation: "read boot id",
                detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn current(&self) -> Result<String, EnvError> {
        Err(EnvError::Unsupported {
            operation: "read boot id",
        })
    }
}

/// Field 22 (`starttime`, clock ticks after boot) of a `/proc/<pid>/stat` line.
/// The command name in field 2 may contain spaces and parentheses, so fields
/// are counted from the last `)`.
pub fn parse_proc_stat_start_time(stat: &str) -> Option<u64> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    // `after_comm` begins at field 3, so field 22 is the 20th entry.
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[derive(Debug, Clone, Copy)]
pub struct SystemProcesses;

impl Processes for SystemProcesses {
    #[cfg(target_os = "linux")]
    fn is_alive(&self, pid: u32, start_time: u64) -> bool {
        matches!(self.start_time(pid), Ok(Some(t)) if t == start_time)
    }

    #[cfg(target_os = "linux")]
    fn start_time(&self, pid: u32) -> Result<Option<u64>, EnvError> {
        let path = PathBuf::from(format!("/proc/{pid}/stat"));
        match std::fs::read_to_string(&path) {
            Ok(stat) => Ok(parse_proc_stat_start_time(&stat)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(EnvError::Io {
                operation: "read process start time",
                path,
                source,
            }),
        }
    }

    /// Without a start-time source the pid check cannot detect recycling; it
    /// only reports whether the pid exists.
    #[cfg(not(target_os = "linux"))]
    fn is_alive(&self, pid: u32, _start_time: u64) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(not(target_os = "linux"))]
    fn start_time(&self, _pid: u32) -> Result<Option<u64>, EnvError> {
        Err(EnvError::Unsupported {
            operation: "read process start time",
        })
    }
}

/// The real tmux. With a socket it talks to that private server (`tmux -S`),
/// otherwise to the default one.
#[derive(Debug, Clone, Default)]
pub struct SystemTmux {
    socket: Option<PathBuf>,
}

impl SystemTmux {
    pub fn with_socket(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: Some(socket.into()),
        }
    }

    fn output(&self, operation: &'static str, args: &[&str]) -> Result<String, EnvError> {
        tmux_output(operation, self.socket.as_deref(), args)
    }

    /// Exact-name match (`=`), so `agents` never matches `agents-old`.
    fn has_session(&self, operation: &'static str, session: &str) -> Result<bool, EnvError> {
        let target = format!("={session}");
        let mut command = Command::new("tmux");
        if let Some(socket) = &self.socket {
            command.arg("-S").arg(socket);
        }
        command
            .args(["has-session", "-t", &target])
            .output()
            .map(|out| out.status.success())
            .map_err(|e| EnvError::Failed {
                operation,
                detail: format!("tmux has-session -t {target}: {e}"),
            })
    }
}

fn tmux_output(
    operation: &'static str,
    socket: Option<&Path>,
    args: &[&str],
) -> Result<String, EnvError> {
    let mut command = Command::new("tmux");
    if let Some(socket) = socket {
        command.arg("-S").arg(socket);
    }
    let out = command.args(args).output().map_err(|e| EnvError::Failed {
        operation,
        detail: format!("tmux {}: {e}", args.join(" ")),
    })?;
    if !out.status.success() {
        return Err(EnvError::Failed {
            operation,
            detail: format!(
                "tmux {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

impl Tmux for SystemTmux {
    fn is_installed(&self) -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn list_panes(&self) -> Result<Option<HashMap<String, PaneLocation>>, EnvError> {
        crate::tmux::list_panes_at(self.socket.as_deref()).map_err(|e| EnvError::Failed {
            operation: "list tmux panes",
            detail: format!("{e:#}"),
        })
    }

    fn new_session(&self, name: &str, cwd: &Path, command: &str) -> Result<String, EnvError> {
        let cwd = cwd.to_string_lossy();
        self.output(
            "create tmux session",
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                name,
                "-c",
                &cwd,
                command,
            ],
        )
    }

    fn new_window(&self, session: &str, cwd: &Path, command: &str) -> Result<String, EnvError> {
        let cwd = cwd.to_string_lossy();
        self.output(
            "create tmux window",
            &[
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                session,
                "-c",
                &cwd,
                command,
            ],
        )
    }

    fn display_message(&self, message: &str) -> Result<(), EnvError> {
        self.output("tmux display-message", &["display-message", message])
            .map(|_| ())
    }

    fn ensure_session(&self, session: &str, cwd: &Path) -> Result<bool, EnvError> {
        const OPERATION: &str = "ensure tmux session";
        if self.has_session(OPERATION, session)? {
            return Ok(false);
        }
        let cwd = cwd.to_string_lossy();
        match self.output(OPERATION, &["new-session", "-d", "-s", session, "-c", &cwd]) {
            Ok(_) => Ok(true),
            // Another client may have created it between the check and now.
            Err(e) => {
                if self.has_session(OPERATION, session)? {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn spawn_window(
        &self,
        session: &str,
        label: &str,
        cwd: &Path,
        argv: &[String],
    ) -> Result<SpawnedWindow, EnvError> {
        const OPERATION: &str = "spawn tmux window";
        let target = format!("={session}:");
        let cwd = cwd.to_string_lossy();
        let mut args = vec![
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}\t#{window_id}",
            "-t",
            &target,
            "-n",
            label,
            "-c",
            &cwd,
            "--",
        ];
        args.extend(argv.iter().map(String::as_str));
        let out = self.output(OPERATION, &args)?;
        match out.split_once('\t') {
            Some((pane_id, window_id)) => Ok(SpawnedWindow {
                pane_id: pane_id.to_owned(),
                window_id: window_id.to_owned(),
            }),
            None => Err(EnvError::Failed {
                operation: OPERATION,
                detail: format!("unexpected new-window output {out:?}"),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn send(&self, summary: &str, body: &str) -> Result<(), EnvError> {
        #[cfg(target_os = "linux")]
        {
            notify_rust::Notification::new()
                .summary(summary)
                .body(body)
                .urgency(notify_rust::Urgency::Normal)
                .show()
                .map(|_| ())
                .map_err(|e| EnvError::Failed {
                    operation: "send notification",
                    detail: e.to_string(),
                })
        }
        #[cfg(target_os = "macos")]
        {
            mac_notification_sys::send_notification(summary, None, body, None)
                .map(|_| ())
                .map_err(|e| EnvError::Failed {
                    operation: "send notification",
                    detail: e.to_string(),
                })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (summary, body);
            Err(EnvError::Unsupported {
                operation: "send notification",
            })
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemFs;

impl Fs for SystemFs {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError> {
        std::fs::read_to_string(path).map_err(|source| EnvError::Io {
            operation: "read file",
            path: path.to_owned(),
            source,
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, EnvError> {
        let io = |source| EnvError::Io {
            operation: "list directory",
            path: path.to_owned(),
            source,
        };
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path).map_err(io)? {
            entries.push(entry.map_err(io)?.path());
        }
        entries.sort();
        Ok(entries)
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf, EnvError> {
        std::fs::read_link(path).map_err(|source| EnvError::Io {
            operation: "read link",
            path: path.to_owned(),
            source,
        })
    }

    fn metadata(&self, path: &Path) -> Result<FileMeta, EnvError> {
        let meta = std::fs::metadata(path).map_err(|source| EnvError::Io {
            operation: "stat file",
            path: path.to_owned(),
            source,
        })?;
        Ok(FileMeta {
            len: meta.len(),
            is_dir: meta.is_dir(),
            modified: meta.modified().ok(),
        })
    }

    fn read_range(&self, path: &Path, offset: u64, max_len: usize) -> Result<Vec<u8>, EnvError> {
        let io = |operation| {
            move |source| EnvError::Io {
                operation,
                path: path.to_owned(),
                source,
            }
        };
        let mut file = std::fs::File::open(path).map_err(io("open file"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(io("seek in file"))?;
        let limit = u64::try_from(max_len).unwrap_or(u64::MAX);
        let mut buf = Vec::new();
        file.take(limit)
            .read_to_end(&mut buf)
            .map_err(io("read file range"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parses_start_time_with_awkward_command_name() {
        let stat = "42 (a b) c) S 1 42 42 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
        assert_eq!(parse_proc_stat_start_time(stat), Some(987_654));
    }

    #[test]
    fn rejects_truncated_stat() {
        assert_eq!(parse_proc_stat_start_time("42 (x) S 1"), None);
        assert_eq!(parse_proc_stat_start_time("garbage"), None);
    }

    #[test]
    fn system_clock_is_rfc3339_utc() {
        let now = SystemClock.now();
        assert!(chrono::DateTime::parse_from_rfc3339(&now).is_ok(), "{now}");
        assert!(now.ends_with('Z'));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_boot_id_is_stable_and_nonempty() {
        let a = SystemBootId.current().unwrap();
        assert!(!a.is_empty());
        assert_eq!(a, SystemBootId.current().unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_own_process_is_alive_and_recycled_pid_is_not() {
        let pid = std::process::id();
        let started = SystemProcesses.start_time(pid).unwrap().unwrap();
        assert!(SystemProcesses.is_alive(pid, started));
        assert!(!SystemProcesses.is_alive(pid, started.wrapping_add(1)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_missing_pid_has_no_start_time() {
        // pid_max is at most 2^22, so this pid cannot exist.
        assert_eq!(SystemProcesses.start_time(u32::MAX).unwrap(), None);
    }

    #[test]
    fn system_fs_lists_sorted_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b"), "1").unwrap();
        std::fs::write(dir.path().join("a"), "2").unwrap();
        let fs = SystemFs;
        let names: Vec<_> = fs
            .list_dir(dir.path())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(fs.read_to_string(&dir.path().join("a")).unwrap(), "2");
        assert!(!fs.exists(&dir.path().join("missing")));
        let err = fs.read_to_string(&dir.path().join("missing")).unwrap_err();
        assert!(err.to_string().starts_with("read file "), "{err}");
    }
}
