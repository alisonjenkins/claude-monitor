//! In-memory, controllable, call-recording providers for tests (unit and
//! `tests/`). Compiled always because it is small; hidden from the docs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::env::{
    BootId, Clock, Env, EnvError, FileMeta, Fs, Notifier, Processes, SpawnedWindow, Tmux, Vars,
};
use crate::tmux::PaneLocation;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
pub struct FakeClock(Mutex<String>);

impl FakeClock {
    pub fn new(now: &str) -> Self {
        Self(Mutex::new(now.to_owned()))
    }

    pub fn set(&self, now: &str) {
        *lock(&self.0) = now.to_owned();
    }
}

impl Clock for FakeClock {
    fn now(&self) -> String {
        lock(&self.0).clone()
    }
}

#[derive(Debug)]
pub struct FakeBootId(Mutex<Result<String, String>>);

impl FakeBootId {
    pub fn new(id: &str) -> Self {
        Self(Mutex::new(Ok(id.to_owned())))
    }

    /// Simulate a reboot.
    pub fn set(&self, id: &str) {
        *lock(&self.0) = Ok(id.to_owned());
    }

    pub fn fail(&self, detail: &str) {
        *lock(&self.0) = Err(detail.to_owned());
    }
}

impl BootId for FakeBootId {
    fn current(&self) -> Result<String, EnvError> {
        lock(&self.0).clone().map_err(|detail| EnvError::Failed {
            operation: "read boot id",
            detail,
        })
    }
}

#[derive(Debug, Default)]
pub struct FakeProcesses(Mutex<BTreeMap<u32, u64>>);

impl FakeProcesses {
    /// Register a running process; replacing a pid models pid reuse.
    pub fn spawn(&self, pid: u32, start_time: u64) {
        lock(&self.0).insert(pid, start_time);
    }

    pub fn kill(&self, pid: u32) {
        lock(&self.0).remove(&pid);
    }
}

impl Processes for FakeProcesses {
    fn is_alive(&self, pid: u32, start_time: u64) -> bool {
        lock(&self.0).get(&pid) == Some(&start_time)
    }

    fn start_time(&self, pid: u32) -> Result<Option<u64>, EnvError> {
        Ok(lock(&self.0).get(&pid).copied())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxCall {
    ListPanes,
    NewSession {
        name: String,
        cwd: PathBuf,
        command: String,
    },
    NewWindow {
        session: String,
        cwd: PathBuf,
        command: String,
    },
    DisplayMessage(String),
    EnsureSession {
        session: String,
        cwd: PathBuf,
    },
    SpawnWindow {
        session: String,
        label: String,
        cwd: PathBuf,
        argv: Vec<String>,
    },
}

#[derive(Debug, Default)]
struct TmuxState {
    available: bool,
    panes: HashMap<String, PaneLocation>,
    calls: Vec<TmuxCall>,
    next_pane: u32,
    next_window: u32,
    fail_creates: Option<String>,
}

#[derive(Debug)]
pub struct FakeTmux(Mutex<TmuxState>);

impl Default for FakeTmux {
    fn default() -> Self {
        Self(Mutex::new(TmuxState {
            available: true,
            ..TmuxState::default()
        }))
    }
}

impl FakeTmux {
    pub fn set_available(&self, available: bool) {
        lock(&self.0).available = available;
    }

    pub fn add_pane(&self, pane_id: &str, location: PaneLocation) {
        lock(&self.0).panes.insert(pane_id.to_owned(), location);
    }

    pub fn remove_pane(&self, pane_id: &str) {
        lock(&self.0).panes.remove(pane_id);
    }

    /// Make `new_session` and `new_window` fail with `detail`.
    pub fn fail_creates(&self, detail: &str) {
        lock(&self.0).fail_creates = Some(detail.to_owned());
    }

    pub fn calls(&self) -> Vec<TmuxCall> {
        lock(&self.0).calls.clone()
    }

    fn create(
        &self,
        operation: &'static str,
        call: TmuxCall,
        location: PaneLocation,
    ) -> Result<String, EnvError> {
        let mut state = lock(&self.0);
        state.calls.push(call);
        if let Some(detail) = state.fail_creates.clone() {
            return Err(EnvError::Failed { operation, detail });
        }
        let pane_id = format!("%{}", state.next_pane);
        state.next_pane = state.next_pane.saturating_add(1);
        state.panes.insert(pane_id.clone(), location);
        Ok(pane_id)
    }
}

impl Tmux for FakeTmux {
    fn is_installed(&self) -> bool {
        lock(&self.0).available
    }

    fn list_panes(&self) -> Result<Option<HashMap<String, PaneLocation>>, EnvError> {
        let mut state = lock(&self.0);
        state.calls.push(TmuxCall::ListPanes);
        Ok(state.available.then(|| state.panes.clone()))
    }

    fn new_session(&self, name: &str, cwd: &Path, command: &str) -> Result<String, EnvError> {
        self.create(
            "create tmux session",
            TmuxCall::NewSession {
                name: name.to_owned(),
                cwd: cwd.to_owned(),
                command: command.to_owned(),
            },
            PaneLocation {
                session_name: name.to_owned(),
                window_name: "claude".to_owned(),
                window_index: "0".to_owned(),
                pane_index: "0".to_owned(),
            },
        )
    }

    fn new_window(&self, session: &str, cwd: &Path, command: &str) -> Result<String, EnvError> {
        self.create(
            "create tmux window",
            TmuxCall::NewWindow {
                session: session.to_owned(),
                cwd: cwd.to_owned(),
                command: command.to_owned(),
            },
            PaneLocation {
                session_name: session.to_owned(),
                window_name: "claude".to_owned(),
                window_index: "1".to_owned(),
                pane_index: "0".to_owned(),
            },
        )
    }

    fn display_message(&self, message: &str) -> Result<(), EnvError> {
        lock(&self.0)
            .calls
            .push(TmuxCall::DisplayMessage(message.to_owned()));
        Ok(())
    }

    fn ensure_session(&self, session: &str, cwd: &Path) -> Result<bool, EnvError> {
        let mut state = lock(&self.0);
        state.calls.push(TmuxCall::EnsureSession {
            session: session.to_owned(),
            cwd: cwd.to_owned(),
        });
        if state.panes.values().any(|p| p.session_name == session) {
            return Ok(false);
        }
        if let Some(detail) = state.fail_creates.clone() {
            return Err(EnvError::Failed {
                operation: "ensure tmux session",
                detail,
            });
        }
        let pane_id = format!("%{}", state.next_pane);
        state.next_pane = state.next_pane.saturating_add(1);
        state.next_window = state.next_window.saturating_add(1);
        state.panes.insert(
            pane_id,
            PaneLocation {
                session_name: session.to_owned(),
                window_name: "claude-monitor-placeholder".to_owned(),
                window_index: "0".to_owned(),
                pane_index: "0".to_owned(),
            },
        );
        Ok(true)
    }

    fn spawn_window(
        &self,
        session: &str,
        label: &str,
        cwd: &Path,
        argv: &[String],
    ) -> Result<SpawnedWindow, EnvError> {
        let mut state = lock(&self.0);
        state.calls.push(TmuxCall::SpawnWindow {
            session: session.to_owned(),
            label: label.to_owned(),
            cwd: cwd.to_owned(),
            argv: argv.to_vec(),
        });
        if let Some(detail) = state.fail_creates.clone() {
            return Err(EnvError::Failed {
                operation: "spawn tmux window",
                detail,
            });
        }
        let pane_id = format!("%{}", state.next_pane);
        let window_index = state.next_window;
        state.next_pane = state.next_pane.saturating_add(1);
        state.next_window = state.next_window.saturating_add(1);
        state.panes.insert(
            pane_id.clone(),
            PaneLocation {
                session_name: session.to_owned(),
                window_name: label.to_owned(),
                window_index: window_index.to_string(),
                pane_index: "0".to_owned(),
            },
        );
        Ok(SpawnedWindow {
            pane_id,
            window_id: format!("@{window_index}"),
        })
    }
}

#[derive(Debug, Default)]
pub struct FakeNotifier(Mutex<Vec<(String, String)>>);

impl FakeNotifier {
    /// `(summary, body)` of every notification sent, in order.
    pub fn sent(&self) -> Vec<(String, String)> {
        lock(&self.0).clone()
    }
}

impl Notifier for FakeNotifier {
    fn send(&self, summary: &str, body: &str) -> Result<(), EnvError> {
        lock(&self.0).push((summary.to_owned(), body.to_owned()));
        Ok(())
    }
}

#[derive(Debug, Default)]
struct FsState {
    files: BTreeMap<PathBuf, String>,
    dirs: BTreeSet<PathBuf>,
    links: BTreeMap<PathBuf, PathBuf>,
}

#[derive(Debug, Default)]
pub struct FakeFs(Mutex<FsState>);

impl FakeFs {
    /// Add a file; its parent directories exist from then on.
    pub fn add_file(&self, path: impl Into<PathBuf>, contents: &str) {
        let path = path.into();
        let mut state = lock(&self.0);
        Self::add_parents(&mut state, &path);
        state.files.insert(path, contents.to_owned());
    }

    pub fn add_dir(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut state = lock(&self.0);
        Self::add_parents(&mut state, &path);
        state.dirs.insert(path);
    }

    pub fn add_link(&self, path: impl Into<PathBuf>, target: impl Into<PathBuf>) {
        let path = path.into();
        let mut state = lock(&self.0);
        Self::add_parents(&mut state, &path);
        state.links.insert(path, target.into());
    }

    fn add_parents(state: &mut FsState, path: &Path) {
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.as_os_str().is_empty() {
                state.dirs.insert(ancestor.to_owned());
            }
        }
    }

    fn not_found(operation: &'static str, path: &Path) -> EnvError {
        EnvError::Io {
            operation,
            path: path.to_owned(),
            source: io::Error::from(io::ErrorKind::NotFound),
        }
    }
}

impl Fs for FakeFs {
    fn read_to_string(&self, path: &Path) -> Result<String, EnvError> {
        lock(&self.0)
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| Self::not_found("read file", path))
    }

    fn exists(&self, path: &Path) -> bool {
        let state = lock(&self.0);
        state.files.contains_key(path)
            || state.dirs.contains(path)
            || state.links.contains_key(path)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, EnvError> {
        let state = lock(&self.0);
        if !state.dirs.contains(path) {
            return Err(Self::not_found("list directory", path));
        }
        let children: BTreeSet<PathBuf> = state
            .files
            .keys()
            .chain(state.dirs.iter())
            .chain(state.links.keys())
            .filter(|p| p.parent() == Some(path))
            .cloned()
            .collect();
        Ok(children.into_iter().collect())
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf, EnvError> {
        lock(&self.0)
            .links
            .get(path)
            .cloned()
            .ok_or_else(|| Self::not_found("read link", path))
    }

    fn metadata(&self, path: &Path) -> Result<FileMeta, EnvError> {
        let state = lock(&self.0);
        if let Some(contents) = state.files.get(path) {
            return Ok(FileMeta {
                len: u64::try_from(contents.len()).unwrap_or(u64::MAX),
                is_dir: false,
                modified: None,
            });
        }
        if state.dirs.contains(path) {
            return Ok(FileMeta {
                len: 0,
                is_dir: true,
                modified: None,
            });
        }
        Err(Self::not_found("stat file", path))
    }

    fn read_range(&self, path: &Path, offset: u64, max_len: usize) -> Result<Vec<u8>, EnvError> {
        let state = lock(&self.0);
        let contents = state
            .files
            .get(path)
            .ok_or_else(|| Self::not_found("read file range", path))?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        Ok(contents
            .as_bytes()
            .iter()
            .skip(start)
            .take(max_len)
            .copied()
            .collect())
    }
}

#[derive(Debug, Default)]
pub struct FakeVars(Mutex<BTreeMap<String, String>>);

impl FakeVars {
    pub fn set(&self, name: &str, value: &str) {
        lock(&self.0).insert(name.to_owned(), value.to_owned());
    }
}

impl Vars for FakeVars {
    fn var(&self, name: &str) -> Option<String> {
        lock(&self.0).get(name).filter(|v| !v.is_empty()).cloned()
    }
}

/// An [`Env`] built from fakes, with the typed handles kept for control and
/// assertions.
pub struct FakeEnv {
    pub env: Env,
    pub clock: Arc<FakeClock>,
    pub boot_id: Arc<FakeBootId>,
    pub processes: Arc<FakeProcesses>,
    pub tmux: Arc<FakeTmux>,
    pub notifier: Arc<FakeNotifier>,
    pub fs: Arc<FakeFs>,
    pub vars: Arc<FakeVars>,
}

impl FakeEnv {
    pub fn new() -> Self {
        let clock = Arc::new(FakeClock::new("2026-10-12T00:00:00Z"));
        let boot_id = Arc::new(FakeBootId::new("boot-1"));
        let processes = Arc::new(FakeProcesses::default());
        let tmux = Arc::new(FakeTmux::default());
        let notifier = Arc::new(FakeNotifier::default());
        let fs = Arc::new(FakeFs::default());
        let vars = Arc::new(FakeVars::default());
        let env = Env {
            clock: clock.clone(),
            boot_id: boot_id.clone(),
            processes: processes.clone(),
            tmux: tmux.clone(),
            notifier: notifier.clone(),
            fs: fs.clone(),
            vars: vars.clone(),
        };
        Self {
            env,
            clock,
            boot_id,
            processes,
            tmux,
            notifier,
            fs,
            vars,
        }
    }
}

impl Default for FakeEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn clock_and_boot_id_are_controllable() {
        let f = FakeEnv::new();
        assert_eq!(f.env.clock.now(), "2026-10-12T00:00:00Z");
        f.clock.set("2026-10-12T01:00:00Z");
        assert_eq!(f.env.clock.now(), "2026-10-12T01:00:00Z");
        assert_eq!(f.env.boot_id.current().unwrap(), "boot-1");
        f.boot_id.set("boot-2");
        assert_eq!(f.env.boot_id.current().unwrap(), "boot-2");
        f.boot_id.fail("no /proc");
        assert_eq!(
            f.env.boot_id.current().unwrap_err().to_string(),
            "read boot id: no /proc"
        );
    }

    #[test]
    fn processes_detect_kill_and_pid_reuse() {
        let f = FakeEnv::new();
        f.processes.spawn(10, 100);
        assert!(f.env.processes.is_alive(10, 100));
        assert_eq!(f.env.processes.start_time(10).unwrap(), Some(100));
        f.processes.spawn(10, 200);
        assert!(!f.env.processes.is_alive(10, 100));
        f.processes.kill(10);
        assert!(!f.env.processes.is_alive(10, 200));
        assert_eq!(f.env.processes.start_time(10).unwrap(), None);
    }

    #[test]
    fn tmux_records_calls_and_tracks_created_panes() {
        let f = FakeEnv::new();
        let a = f
            .env
            .tmux
            .new_session("agents", Path::new("/w/a"), "claude")
            .unwrap();
        let b = f
            .env
            .tmux
            .new_window("agents", Path::new("/w/b"), "claude --resume x")
            .unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("%0", "%1"));
        f.env.tmux.display_message("hi").unwrap();
        let panes = f.env.tmux.list_panes().unwrap().unwrap();
        assert_eq!(panes.len(), 2);
        assert_eq!(
            panes
                .get("%1")
                .map(PaneLocation::format)
                .unwrap_or_default(),
            "agents:1.0"
        );
        assert_eq!(
            f.tmux.calls(),
            vec![
                TmuxCall::NewSession {
                    name: "agents".into(),
                    cwd: "/w/a".into(),
                    command: "claude".into()
                },
                TmuxCall::NewWindow {
                    session: "agents".into(),
                    cwd: "/w/b".into(),
                    command: "claude --resume x".into()
                },
                TmuxCall::DisplayMessage("hi".into()),
                TmuxCall::ListPanes,
            ]
        );
    }

    #[test]
    fn fake_ensure_session_creates_once_then_reuses() {
        let f = FakeEnv::new();
        let cwd = Path::new("/w");
        assert!(f.env.tmux.ensure_session("agents", cwd).unwrap());
        assert!(!f.env.tmux.ensure_session("agents", cwd).unwrap());
        assert_eq!(
            f.tmux.calls(),
            vec![
                TmuxCall::EnsureSession {
                    session: "agents".into(),
                    cwd: "/w".into()
                };
                2
            ]
        );
        let panes = f.env.tmux.list_panes().unwrap().unwrap();
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn fake_spawn_window_records_the_call_and_tracks_a_pane() {
        let f = FakeEnv::new();
        let argv = vec!["claude".to_owned(), "--resume".to_owned(), "x".to_owned()];
        let a = f
            .env
            .tmux
            .spawn_window("agents", "proj", Path::new("/w/a"), &argv)
            .unwrap();
        let b = f
            .env
            .tmux
            .spawn_window("agents", "other", Path::new("/w/b"), &argv)
            .unwrap();
        assert_ne!(a.pane_id, b.pane_id);
        assert_ne!(a.window_id, b.window_id);
        let panes = f.env.tmux.list_panes().unwrap().unwrap();
        assert!(panes.contains_key(&a.pane_id) && panes.contains_key(&b.pane_id));
        assert_eq!(
            f.tmux.calls().first(),
            Some(&TmuxCall::SpawnWindow {
                session: "agents".into(),
                label: "proj".into(),
                cwd: "/w/a".into(),
                argv,
            })
        );
    }

    #[test]
    fn fake_spawn_window_failure_uses_the_create_failure() {
        let f = FakeEnv::new();
        f.tmux.fail_creates("no server");
        let err = f
            .env
            .tmux
            .spawn_window("agents", "p", Path::new("/"), &["x".to_owned()])
            .unwrap_err();
        assert_eq!(err.to_string(), "spawn tmux window: no server");
    }

    #[test]
    fn tmux_unavailable_and_failing_creates() {
        let f = FakeEnv::new();
        f.tmux.set_available(false);
        assert!(f.env.tmux.list_panes().unwrap().is_none());
        f.tmux.fail_creates("no server");
        let err = f
            .env
            .tmux
            .new_session("s", Path::new("/"), "x")
            .unwrap_err();
        assert_eq!(err.to_string(), "create tmux session: no server");
        f.tmux.add_pane(
            "%9",
            PaneLocation {
                session_name: "s".into(),
                window_name: "w".into(),
                window_index: "0".into(),
                pane_index: "0".into(),
            },
        );
        f.tmux.set_available(true);
        assert!(f.env.tmux.list_panes().unwrap().unwrap().contains_key("%9"));
        f.tmux.remove_pane("%9");
        assert!(f.env.tmux.list_panes().unwrap().unwrap().is_empty());
    }

    #[test]
    fn notifier_records_in_order() {
        let f = FakeEnv::new();
        f.env.notifier.send("a", "1").unwrap();
        f.env.notifier.send("b", "2").unwrap();
        assert_eq!(
            f.notifier.sent(),
            vec![("a".into(), "1".into()), ("b".into(), "2".into())]
        );
    }

    #[test]
    fn fs_files_dirs_and_links() {
        let f = FakeEnv::new();
        f.fs.add_file("/s/b.json", "{}");
        f.fs.add_file("/s/a.json", "[]");
        f.fs.add_link("/s/cur", "/s/a.json");
        f.fs.add_dir("/s/sub");
        assert!(f.env.fs.exists(Path::new("/s")));
        assert!(!f.env.fs.exists(Path::new("/nope")));
        assert_eq!(
            f.env.fs.read_to_string(Path::new("/s/a.json")).unwrap(),
            "[]"
        );
        assert_eq!(
            f.env.fs.list_dir(Path::new("/s")).unwrap(),
            [
                PathBuf::from("/s/a.json"),
                PathBuf::from("/s/b.json"),
                PathBuf::from("/s/cur"),
                PathBuf::from("/s/sub"),
            ]
        );
        assert_eq!(
            f.env.fs.read_link(Path::new("/s/cur")).unwrap(),
            Path::new("/s/a.json")
        );
        assert!(f.env.fs.read_to_string(Path::new("/s/none")).is_err());
        assert!(f.env.fs.list_dir(Path::new("/none")).is_err());
        assert!(f.env.fs.read_link(Path::new("/s/a.json")).is_err());
    }
}
