//! Process-exit event source: wakes the moment a registered process dies.
//!
//! Linux uses a `pidfd` per process, polled together with an `eventfd` that
//! `register` writes to so a blocked `wait` picks up new processes without a
//! sleep loop. The macOS `kqueue` `NOTE_EXIT` backend is a stub that reports
//! [`ProcExitError::Unsupported`] until the macOS task fills it in.
//!
//! The wake is edge-triggered: `wait` returns true when an exit was recorded
//! since the last `wait` returned true, and [`ProcExitHandle::drain_exited`]
//! says which pids.

#[cfg(target_os = "linux")]
pub use linux::{ProcExitHandle, ProcExitSource};
#[cfg(not(target_os = "linux"))]
pub use unsupported::{ProcExitHandle, ProcExitSource};

#[derive(Debug, thiserror::Error)]
pub enum ProcExitError {
    #[error("process exit notification is not supported on this platform")]
    Unsupported,
    #[error("cannot create the wake eventfd: {0}")]
    WakeCreate(std::io::Error),
    #[error("pid {pid} is not a valid process id")]
    InvalidPid { pid: u32 },
    #[error("cannot open a pidfd for pid {pid}: {source}")]
    PidfdOpen { pid: u32, source: std::io::Error },
    #[error("cannot read /proc/{pid}/stat: {source}")]
    ReadStat { pid: u32, source: std::io::Error },
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{BTreeSet, HashMap};
    use std::io::ErrorKind;
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags, Timespec};
    use rustix::io::Errno;
    use rustix::process::{pidfd_open, Pid, PidfdFlags};

    use super::ProcExitError;
    use crate::env::parse_proc_stat_start_time;
    use crate::feed::EventSource;

    #[derive(Debug, Default)]
    struct State {
        watched: HashMap<u32, Arc<OwnedFd>>,
        exited: BTreeSet<u32>,
        /// An exit was recorded that no `wait` has reported yet.
        fresh: bool,
    }

    #[derive(Debug)]
    struct Shared {
        state: Mutex<State>,
        wake: OwnedFd,
    }

    /// Start time of `pid` (field 22 of `/proc/<pid>/stat`); `None` when the
    /// process is gone or the line is unreadable as a stat line.
    fn read_start_time(pid: u32) -> Result<Option<u64>, ProcExitError> {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => Ok(parse_proc_stat_start_time(&stat)),
            Err(e)
                if e.kind() == ErrorKind::NotFound
                    || e.raw_os_error() == Some(Errno::SRCH.raw_os_error()) =>
            {
                Ok(None)
            }
            Err(source) => Err(ProcExitError::ReadStat { pid, source }),
        }
    }

    fn timespec(d: Duration) -> Timespec {
        Timespec {
            tv_sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(d.subsec_nanos()),
        }
    }

    impl Shared {
        fn lock(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Interrupts a blocked `wait` so it rebuilds its poll set. A full
        /// counter means a wake is already pending, so a failed write is fine.
        fn notify(&self) {
            let _ = rustix::io::write(&self.wake, &1u64.to_ne_bytes());
        }

        fn register(&self, pid: u32, start_time: u64) -> Result<(), ProcExitError> {
            let raw = i32::try_from(pid)
                .ok()
                .and_then(Pid::from_raw)
                .ok_or(ProcExitError::InvalidPid { pid })?;
            // Open the pidfd before reading the start time: a pid recycled
            // after the open cannot change what the pidfd refers to, and one
            // recycled before it shows up as a start-time mismatch.
            let pidfd = match pidfd_open(raw, PidfdFlags::empty()) {
                Ok(fd) => Some(fd),
                Err(Errno::SRCH) => None,
                Err(e) => {
                    return Err(ProcExitError::PidfdOpen {
                        pid,
                        source: e.into(),
                    })
                }
            };
            let verified = match pidfd {
                Some(fd) if read_start_time(pid)? == Some(start_time) => Some(fd),
                _ => None,
            };
            {
                let mut state = self.lock();
                state.watched.remove(&pid);
                state.exited.remove(&pid);
                match verified {
                    Some(fd) => {
                        state.watched.insert(pid, Arc::new(fd));
                    }
                    None => {
                        state.exited.insert(pid);
                        state.fresh = true;
                    }
                }
            }
            self.notify();
            Ok(())
        }

        fn unregister(&self, pid: u32) {
            let mut state = self.lock();
            state.watched.remove(&pid);
            state.exited.remove(&pid);
        }

        fn drain_exited(&self) -> Vec<u32> {
            std::mem::take(&mut self.lock().exited)
                .into_iter()
                .collect()
        }

        fn wait(&self, timeout: Duration) -> bool {
            let deadline = Instant::now().checked_add(timeout);
            loop {
                let snapshot: Vec<(u32, Arc<OwnedFd>)> = {
                    let mut state = self.lock();
                    if std::mem::take(&mut state.fresh) {
                        return true;
                    }
                    state
                        .watched
                        .iter()
                        .map(|(pid, fd)| (*pid, Arc::clone(fd)))
                        .collect()
                };
                let remaining =
                    deadline.map_or(timeout, |d| d.saturating_duration_since(Instant::now()));
                let mut fds = Vec::with_capacity(snapshot.len().saturating_add(1));
                fds.push(PollFd::new(&self.wake, PollFlags::IN));
                fds.extend(
                    snapshot
                        .iter()
                        .map(|(_, fd)| PollFd::new(&**fd, PollFlags::IN)),
                );
                match poll(&mut fds, Some(&timespec(remaining))) {
                    Ok(_) => {}
                    Err(Errno::INTR) => continue,
                    Err(_) => {
                        // Only resource exhaustion lands here; sleeping out
                        // the timeout keeps a caller's loop from spinning.
                        std::thread::sleep(remaining);
                        return false;
                    }
                }
                let woken = fds.first().is_some_and(|f| !f.revents().is_empty());
                let ready: Vec<(u32, Arc<OwnedFd>)> = snapshot
                    .iter()
                    .zip(fds.iter().skip(1))
                    .filter(|(_, f)| !f.revents().is_empty())
                    .map(|(entry, _)| entry.clone())
                    .collect();
                drop(fds);
                if woken {
                    let _ = rustix::io::read(&self.wake, &mut [0u8; 8]);
                }
                let mut state = self.lock();
                for (pid, fd) in ready {
                    // An unregister or re-register since the snapshot wins.
                    if state
                        .watched
                        .get(&pid)
                        .is_some_and(|cur| Arc::ptr_eq(cur, &fd))
                    {
                        state.watched.remove(&pid);
                        state.exited.insert(pid);
                        state.fresh = true;
                    }
                }
                if std::mem::take(&mut state.fresh) {
                    return true;
                }
                drop(state);
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return false;
                }
            }
        }
    }

    /// Cloneable, thread-safe access to the registrations of a
    /// [`ProcExitSource`], for a thread other than the one blocked in `wait`.
    #[derive(Debug, Clone)]
    pub struct ProcExitHandle(Arc<Shared>);

    impl ProcExitHandle {
        /// Watches `pid`, recorded with `start_time` (field 22 of
        /// `/proc/<pid>/stat`). A process that already exited, or whose start
        /// time differs because the pid was reused, is reported as exited at
        /// once; that is the pid-reuse guard. Re-registering replaces the
        /// earlier registration.
        pub fn register(&self, pid: u32, start_time: u64) -> Result<(), ProcExitError> {
            self.0.register(pid, start_time)
        }

        /// Stops watching `pid` and forgets an exit that was not drained.
        pub fn unregister(&self, pid: u32) {
            self.0.unregister(pid);
        }

        /// Pids that exited since the last drain, ascending.
        pub fn drain_exited(&self) -> Vec<u32> {
            self.0.drain_exited()
        }
    }

    #[derive(Debug)]
    pub struct ProcExitSource(ProcExitHandle);

    impl ProcExitSource {
        pub fn new() -> Result<Self, ProcExitError> {
            let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
                .map_err(|e| ProcExitError::WakeCreate(e.into()))?;
            Ok(Self(ProcExitHandle(Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake,
            }))))
        }

        pub fn handle(&self) -> ProcExitHandle {
            self.0.clone()
        }

        pub fn register(&self, pid: u32, start_time: u64) -> Result<(), ProcExitError> {
            self.0.register(pid, start_time)
        }

        pub fn unregister(&self, pid: u32) {
            self.0.unregister(pid);
        }

        pub fn drain_exited(&self) -> Vec<u32> {
            self.0.drain_exited()
        }
    }

    impl EventSource for ProcExitSource {
        /// Returns true when a registered process exited (or was found already
        /// gone at registration) since the last true return.
        fn wait(&mut self, timeout: Duration) -> bool {
            self.0 .0.wait(timeout)
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod unsupported {
    use std::time::Duration;

    use super::ProcExitError;
    use crate::feed::EventSource;

    #[derive(Debug, Clone)]
    pub struct ProcExitHandle(());

    impl ProcExitHandle {
        pub fn register(&self, _pid: u32, _start_time: u64) -> Result<(), ProcExitError> {
            Err(ProcExitError::Unsupported)
        }

        pub fn unregister(&self, _pid: u32) {}

        pub fn drain_exited(&self) -> Vec<u32> {
            Vec::new()
        }
    }

    #[derive(Debug)]
    pub struct ProcExitSource(ProcExitHandle);

    impl ProcExitSource {
        pub fn new() -> Result<Self, ProcExitError> {
            Err(ProcExitError::Unsupported)
        }

        pub fn handle(&self) -> ProcExitHandle {
            self.0.clone()
        }

        pub fn register(&self, pid: u32, start_time: u64) -> Result<(), ProcExitError> {
            self.0.register(pid, start_time)
        }

        pub fn unregister(&self, pid: u32) {
            self.0.unregister(pid);
        }

        pub fn drain_exited(&self) -> Vec<u32> {
            self.0.drain_exited()
        }
    }

    impl EventSource for ProcExitSource {
        fn wait(&mut self, _timeout: Duration) -> bool {
            false
        }
    }
}
