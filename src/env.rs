//! Provider traits for everything the app reads from or does to the outside
//! world, so library code is tested with fakes (see `crate::testing`) and no
//! process spawn.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

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
    /// Where one pane is; `None` when tmux does not list it.
    fn pane_location(&self, pane_id: &str) -> Result<Option<PaneLocation>, EnvError>;
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
    /// Split the window holding `window_target_pane` (a pane id) with a
    /// detached pane running `argv` directly (no shell) in `cwd`; returns the
    /// new pane's id.
    fn split_pane(
        &self,
        window_target_pane: &str,
        cwd: &Path,
        argv: &[String],
    ) -> Result<String, EnvError>;
    /// Apply `layout` (a layout string or a name such as `tiled`) to the
    /// window holding `window_target_pane`.
    fn select_layout(&self, window_target_pane: &str, layout: &str) -> Result<(), EnvError>;
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
            tmux: Arc::new(SystemTmux::from_socket_var(
                std::env::var(TMUX_SOCKET_VAR)
                    .ok()
                    .filter(|value| !value.is_empty()),
            )),
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
        // Read through the syscall: the `sysctl` binary is not on PATH in a
        // Nix build sandbox.
        const NAME: &std::ffi::CStr = c"kern.bootsessionuuid";
        let failed = |what: &str| EnvError::Failed {
            operation: "read boot id",
            detail: format!(
                "sysctlbyname kern.bootsessionuuid ({what}): {}",
                std::io::Error::last_os_error()
            ),
        };
        let mut len: libc::size_t = 0;
        // SAFETY: NAME is NUL-terminated and `len` is a valid out-pointer; a
        // null `oldp` asks only for the required size.
        let rc = unsafe {
            libc::sysctlbyname(
                NAME.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return Err(failed("size query"));
        }
        let mut buf = vec![0u8; len];
        // SAFETY: `buf` is writable for `len` bytes and `len` holds its size;
        // the kernel writes at most that many bytes and updates `len`.
        let rc = unsafe {
            libc::sysctlbyname(
                NAME.as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return Err(failed("read"));
        }
        buf.truncate(len);
        sysctl_string("read boot id", &buf)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn current(&self) -> Result<String, EnvError> {
        Err(EnvError::Unsupported {
            operation: "read boot id",
        })
    }
}

/// A string value a `sysctl` call returned: the bytes up to the first NUL.
/// An empty or non-UTF-8 value is an error, not a boot id.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn sysctl_string(operation: &'static str, buf: &[u8]) -> Result<String, EnvError> {
    let bytes = buf.split(|b| *b == 0).next().unwrap_or_default();
    let text = std::str::from_utf8(bytes).map_err(|e| EnvError::Failed {
        operation,
        detail: format!("sysctl value is not UTF-8: {e}"),
    })?;
    if text.is_empty() {
        return Err(EnvError::Failed {
            operation,
            detail: "sysctl value is empty".to_owned(),
        });
    }
    Ok(text.to_owned())
}

/// Field 22 (`starttime`, clock ticks after boot) of a `/proc/<pid>/stat` line.
/// The command name in field 2 may contain spaces and parentheses, so fields
/// are counted from the last `)`.
pub fn parse_proc_stat_start_time(stat: &str) -> Option<u64> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    // `after_comm` begins at field 3, so field 22 is the 20th entry.
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// macOS `pbi_start_tvsec`/`pbi_start_tvusec` as microseconds since the epoch;
/// `None` on overflow.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bsd_start_micros(tvsec: u64, tvusec: u64) -> Option<u64> {
    tvsec.checked_mul(1_000_000)?.checked_add(tvusec)
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

    #[cfg(target_os = "macos")]
    fn is_alive(&self, pid: u32, start_time: u64) -> bool {
        matches!(self.start_time(pid), Ok(Some(t)) if t == start_time)
    }

    /// Microseconds since the Unix epoch, not Linux clock ticks; a recorded
    /// value is only ever compared with one read on the same machine.
    #[cfg(target_os = "macos")]
    fn start_time(&self, pid: u32) -> Result<Option<u64>, EnvError> {
        const OPERATION: &str = "read process start time";
        // A pid above i32::MAX cannot name a process.
        let Ok(pid) = i32::try_from(pid) else {
            return Ok(None);
        };
        // SAFETY: proc_bsdinfo is plain integers and char arrays, for which
        // all-zero bytes are a valid value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size =
            libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).map_err(|e| {
                EnvError::Failed {
                    operation: OPERATION,
                    detail: e.to_string(),
                }
            })?;
        // SAFETY: `info` is writable for `size` bytes, which is the size
        // passed; the kernel writes at most that many.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if written == size {
            return bsd_start_micros(info.pbi_start_tvsec, info.pbi_start_tvusec)
                .map(Some)
                .ok_or_else(|| EnvError::Failed {
                    operation: OPERATION,
                    detail: format!(
                        "start time {}s {}us overflows u64 microseconds",
                        info.pbi_start_tvsec, info.pbi_start_tvusec
                    ),
                });
        }
        let source = std::io::Error::last_os_error();
        if written <= 0 && source.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        Err(EnvError::Failed {
            operation: OPERATION,
            detail: format!("proc_pidinfo({pid}) returned {written}: {source}"),
        })
    }

    /// Without a start-time source the pid check cannot detect recycling; it
    /// only reports whether the pid exists.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn is_alive(&self, pid: u32, _start_time: u64) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn start_time(&self, _pid: u32) -> Result<Option<u64>, EnvError> {
        Err(EnvError::Unsupported {
            operation: "read process start time",
        })
    }
}

/// Names a private tmux server socket (`tmux -S`) for the binary to use
/// instead of the default server, so an end-to-end run never touches the
/// user's own tmux.
pub const TMUX_SOCKET_VAR: &str = "CLAUDE_MONITOR_TMUX_SOCKET";

/// The hook asks tmux where its pane is; a wedged server must not stall the
/// agent, and reconcile fills the location in later.
const PANE_LOCATION_DEADLINE: Duration = Duration::from_millis(500);
const PANE_LOCATION_POLL: Duration = Duration::from_millis(5);

/// Name of the window a new session starts with; `spawn_window` closes it.
const PLACEHOLDER_WINDOW: &str = "claude-monitor-placeholder";

/// `name` as tmux stores it: tmux rewrites `.` and `:` in a session name to
/// `_`, so looking a session up by the unrewritten name never finds it.
fn tmux_session_name(name: &str) -> String {
    name.replace(['.', ':'], "_")
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

    /// The tmux named by `socket` (the value of [`TMUX_SOCKET_VAR`]), or the
    /// default one when it is `None`.
    pub fn from_socket_var(socket: Option<String>) -> Self {
        socket.map_or_else(Self::default, Self::with_socket)
    }

    fn output(&self, operation: &'static str, args: &[&str]) -> Result<String, EnvError> {
        tmux_output(operation, self.socket.as_deref(), args)
    }

    /// Close the empty shell window `ensure_session` left when it created the
    /// session; the agent's own window now keeps the session alive. Best
    /// effort: a leftover shell is harmless, a failed respawn is not.
    fn discard_placeholder(&self, target: &str) {
        let Ok(listing) = self.output(
            "list tmux windows",
            &[
                "list-windows",
                "-t",
                target,
                "-F",
                "#{window_id} #{window_name}",
            ],
        ) else {
            return;
        };
        for line in listing.lines() {
            if let Some((window_id, PLACEHOLDER_WINDOW)) = line.split_once(' ') {
                let _ = self.output(
                    "close placeholder window",
                    &["kill-window", "-t", window_id],
                );
            }
        }
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

    fn pane_location(&self, pane_id: &str) -> Result<Option<PaneLocation>, EnvError> {
        const OPERATION: &str = "locate tmux pane";
        let args = [
            "display-message",
            "-p",
            "-t",
            pane_id,
            crate::tmux::LIST_PANES_FORMAT,
        ];
        let mut command = Command::new("tmux");
        if let Some(socket) = &self.socket {
            command.arg("-S").arg(socket);
        }
        let failed = |detail: String| EnvError::Failed {
            operation: OPERATION,
            detail,
        };
        let mut child = command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| failed(format!("tmux {}: {e}", args.join(" "))))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < PANE_LOCATION_DEADLINE => {
                    std::thread::sleep(PANE_LOCATION_POLL);
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(failed(format!(
                        "tmux {} did not answer within {} ms",
                        args.join(" "),
                        PANE_LOCATION_DEADLINE.as_millis()
                    )));
                }
                Err(e) => return Err(failed(format!("tmux {}: {e}", args.join(" ")))),
            }
        };
        let mut stdout = String::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        if !status.success() {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Err(failed(format!(
                "tmux {} failed: {}",
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(crate::tmux::parse_list_panes(stdout.trim()).remove(pane_id))
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
        let session = &tmux_session_name(session);
        if self.has_session(OPERATION, session)? {
            return Ok(false);
        }
        let cwd = cwd.to_string_lossy();
        match self.output(
            OPERATION,
            &[
                "new-session",
                "-d",
                "-s",
                session,
                "-n",
                PLACEHOLDER_WINDOW,
                "-c",
                &cwd,
            ],
        ) {
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
        let session = &tmux_session_name(session);
        let target = format!("={session}:");
        // After the last window: a bare session target reuses the lowest free
        // index, which puts a window below the ones opened before it once the
        // placeholder is gone.
        let after_last = format!("={session}:{{end}}");
        let cwd = cwd.to_string_lossy();
        let mut args = vec![
            "new-window",
            "-a",
            "-d",
            "-P",
            "-F",
            "#{pane_id} #{window_id}",
            "-t",
            &after_last,
            "-n",
            label,
            "-c",
            &cwd,
            "--",
        ];
        args.extend(argv.iter().map(String::as_str));
        let out = self.output(OPERATION, &args)?;
        self.discard_placeholder(&target);
        // Not a tab: tmux prints tabs as `_` outside a UTF-8 locale.
        match out.split_once(' ') {
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

    fn split_pane(
        &self,
        window_target_pane: &str,
        cwd: &Path,
        argv: &[String],
    ) -> Result<String, EnvError> {
        let cwd = cwd.to_string_lossy();
        let mut args = vec![
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            window_target_pane,
            "-c",
            &cwd,
            "--",
        ];
        args.extend(argv.iter().map(String::as_str));
        self.output("split tmux pane", &args)
    }

    fn select_layout(&self, window_target_pane: &str, layout: &str) -> Result<(), EnvError> {
        self.output(
            "select tmux layout",
            &["select-layout", "-t", window_target_pane, layout],
        )
        .map(|_| ())
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

/// Largest file `Fs::read_to_string` returns; its callers read plan files, and
/// a hostile or runaway one must not be loaded whole.
const MAX_READ_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct SystemFs;

/// Reads a regular file of at most `max_bytes` without ever blocking: a FIFO,
/// device or socket (including one a hostile repo planted) is refused.
pub(crate) fn read_regular_file(path: &Path, max_bytes: u64) -> Result<String, EnvError> {
    let io = |source| EnvError::Io {
        operation: "read file",
        path: path.to_owned(),
        source,
    };
    use std::os::unix::fs::OpenOptionsExt as _;
    // O_NONBLOCK so opening a FIFO with no writer returns instead of hanging.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err(EnvError::Failed {
            operation: "read file",
            detail: format!("{} is not a regular file", path.display()),
        });
    }
    let mut text = String::new();
    let read = file
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)
        .map_err(io)?;
    if read as u64 > max_bytes {
        return Err(EnvError::Failed {
            operation: "read file",
            detail: format!("{} is larger than {max_bytes} bytes", path.display()),
        });
    }
    Ok(text)
}

impl Fs for SystemFs {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError> {
        read_regular_file(path, MAX_READ_BYTES)
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

    #[test]
    fn bsd_start_time_is_whole_microseconds_since_the_epoch() {
        assert_eq!(bsd_start_micros(0, 0), Some(0));
        assert_eq!(
            bsd_start_micros(1_760_000_000, 123_456),
            Some(1_760_000_000_123_456)
        );
        assert_ne!(bsd_start_micros(5, 1), bsd_start_micros(5, 2));
    }

    #[test]
    fn bsd_start_time_overflow_is_none() {
        assert_eq!(bsd_start_micros(u64::MAX, 0), None);
        assert_eq!(bsd_start_micros(u64::MAX / 1_000_000, u64::MAX), None);
    }

    #[test]
    fn sysctl_string_stops_at_the_trailing_nul() {
        let uuid = "6F1D2C3A-0B4E-4C5D-8E9F-A1B2C3D4E5F6";
        let mut buf = uuid.as_bytes().to_vec();
        buf.push(0);
        assert_eq!(sysctl_string("op", &buf).unwrap(), uuid);
        assert_eq!(sysctl_string("op", uuid.as_bytes()).unwrap(), uuid);
    }

    #[test]
    fn sysctl_string_rejects_empty_and_non_utf8_values() {
        assert!(sysctl_string("op", b"").is_err());
        assert!(sysctl_string("op", b"\0").is_err());
        let err = sysctl_string("op", &[0xff, 0xfe, 0]).unwrap_err();
        assert!(err.to_string().contains("not UTF-8"), "{err}");
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_boot_id_is_stable_and_nonempty() {
        let a = SystemBootId.current().unwrap();
        assert!(!a.is_empty());
        assert_eq!(a, SystemBootId.current().unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_own_process_is_alive_and_recycled_pid_is_not() {
        let pid = std::process::id();
        let started = SystemProcesses.start_time(pid).unwrap().unwrap();
        assert!(SystemProcesses.is_alive(pid, started));
        assert!(!SystemProcesses.is_alive(pid, started.wrapping_add(1)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_missing_pid_has_no_start_time() {
        assert_eq!(SystemProcesses.start_time(i32::MAX as u32).unwrap(), None);
    }

    #[test]
    fn tmux_socket_var_selects_a_private_server() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let tmux = SystemTmux::from_socket_var(Some(socket.to_string_lossy().into_owned()));
        if !tmux.is_installed() {
            return;
        }
        assert!(tmux.ensure_session("t", dir.path()).unwrap());
        assert!(socket.exists(), "private socket was not used");
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .arg("kill-server")
            .output();
    }

    #[test]
    fn a_session_name_with_a_dot_or_colon_is_found_again_under_the_name_tmux_gave_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let tmux = SystemTmux::from_socket_var(Some(socket.to_string_lossy().into_owned()));
        if !tmux.is_installed() {
            return;
        }
        let created = tmux.ensure_session("my.work:1", dir.path());
        let again = tmux.ensure_session("my.work:1", dir.path());
        let window = tmux.spawn_window(
            "my.work:1",
            "w",
            dir.path(),
            &["sleep".to_owned(), "30".to_owned()],
        );
        let sessions = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .arg("kill-server")
            .output();
        assert!(created.unwrap());
        assert!(!again.unwrap());
        window.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&sessions.unwrap().stdout).trim(),
            "my_work_1"
        );
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

    #[test]
    fn system_fs_refuses_a_file_over_the_read_cap() {
        let dir = tempfile::tempdir().unwrap();
        let fits = dir.path().join("fits");
        std::fs::write(&fits, vec![b'a'; MAX_READ_BYTES as usize]).unwrap();
        assert_eq!(
            SystemFs.read_to_string(&fits).unwrap().len(),
            MAX_READ_BYTES as usize
        );
        let big = dir.path().join("big");
        std::fs::write(&big, vec![b'a'; MAX_READ_BYTES as usize + 1]).unwrap();
        let err = SystemFs.read_to_string(&big).unwrap_err();
        assert!(err.to_string().contains("larger than"), "{err}");
    }

    #[test]
    fn system_fs_refuses_a_fifo_instead_of_blocking() {
        use std::os::unix::ffi::OsStrExt as _;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("tasks.md");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(SystemFs.read_to_string(&fifo));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("read_to_string blocked on a FIFO");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }
}
