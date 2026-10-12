//! Process-exit event source (stub; the pidfd implementation follows).

use std::time::Duration;

use super::EventSource;

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
