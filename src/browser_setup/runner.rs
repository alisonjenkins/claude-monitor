//! The one place external commands are spawned, behind a trait so the rest of
//! `setup-browser` is tested with a fake that records argument vectors.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// What NSS tools may print is a few KiB; a runaway child must not fill memory.
const OUTPUT_CAP_BYTES: u64 = 1 << 20;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<OsString>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// `None` when a signal ended the process.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("cannot start {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} did not finish within {after:?} and was killed")]
    Timeout { program: String, after: Duration },
    #[error("cannot wait for {program}: {source}")]
    Wait {
        program: String,
        source: std::io::Error,
    },
}

pub trait CommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunError>;
}

/// Spawns the real process with stdin closed, so a tool that would prompt
/// (a primary password) reads end-of-file instead of hanging.
pub struct SystemRunner;

fn drain(stream: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(stream) = stream {
            let _ = stream.take(OUTPUT_CAP_BYTES).read_to_end(&mut buf);
        }
        buf
    })
}

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunError> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| RunError::Spawn {
                program: spec.program.clone(),
                source,
            })?;
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        let deadline = Instant::now().checked_add(spec.timeout);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if deadline.is_some_and(|d| Instant::now() >= d) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::Timeout {
                        program: spec.program.clone(),
                        after: spec.timeout,
                    });
                }
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(source) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::Wait {
                        program: spec.program.clone(),
                        source,
                    });
                }
            }
        };
        Ok(CommandOutput {
            code: status.code(),
            stdout: out.join().unwrap_or_default(),
            stderr: String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned(),
        })
    }
}

/// The NSS tools `setup-browser` drives.
pub const NSS_TOOLS: [&str; 2] = ["certutil", "pk12util"];

pub const NSS_INSTALL_HINT: &str = "nix-shell -p nss.tools";

/// The tools in `NSS_TOOLS` that `path` does not provide as an executable.
pub fn missing_tools(path: &OsStr) -> Vec<&'static str> {
    NSS_TOOLS
        .into_iter()
        .filter(|tool| find_in_path(tool, path).is_none())
        .collect()
}

fn find_in_path(tool: &str, path: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|dir| dir.join(tool))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// A scripted runner for tests: every spec is recorded, the handler answers.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::RefCell;

    type Handler = Box<dyn Fn(&CommandSpec) -> Result<CommandOutput, RunError>>;

    pub struct FakeRunner {
        pub calls: RefCell<Vec<CommandSpec>>,
        handler: Handler,
    }

    pub fn ok(stdout: &[u8]) -> Result<CommandOutput, RunError> {
        Ok(CommandOutput {
            code: Some(0),
            stdout: stdout.to_vec(),
            stderr: String::new(),
        })
    }

    pub fn failed(code: i32, stderr: &str) -> Result<CommandOutput, RunError> {
        Ok(CommandOutput {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.to_owned(),
        })
    }

    impl FakeRunner {
        pub fn new(
            handler: impl Fn(&CommandSpec) -> Result<CommandOutput, RunError> + 'static,
        ) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                handler: Box::new(handler),
            }
        }

        pub fn argv(&self) -> Vec<Vec<String>> {
            self.calls
                .borrow()
                .iter()
                .map(|c| {
                    std::iter::once(c.program.clone())
                        .chain(c.args.iter().map(|a| a.to_string_lossy().into_owned()))
                        .collect()
                })
                .collect()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunError> {
            self.calls.borrow_mut().push(spec.clone());
            (self.handler)(spec)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn spec(program: &str, args: &[&str], timeout_ms: u64) -> CommandSpec {
        CommandSpec {
            program: program.to_owned(),
            args: args.iter().map(OsString::from).collect(),
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    #[test]
    fn captures_stdout_stderr_and_exit_code() {
        let out = SystemRunner
            .run(&spec("sh", &["-c", "echo out; echo err >&2; exit 3"], 5000))
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, "err\n");
        assert!(!out.success());
    }

    #[test]
    fn stdin_is_closed_so_a_prompting_tool_sees_end_of_file() {
        let out = SystemRunner
            .run(&spec("sh", &["-c", "read line; echo got=[$line]"], 5000))
            .unwrap();
        assert_eq!(out.stdout, b"got=[]\n");
    }

    #[test]
    fn a_child_that_outlives_the_timeout_is_killed() {
        let started = Instant::now();
        let err = SystemRunner.run(&spec("sleep", &["30"], 100)).unwrap_err();
        assert!(matches!(err, RunError::Timeout { .. }), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_program_is_a_spawn_error_naming_it() {
        let err = SystemRunner
            .run(&spec("claude-monitor-no-such-tool", &[], 1000))
            .unwrap_err();
        assert!(err.to_string().contains("claude-monitor-no-such-tool"));
    }

    #[test]
    fn missing_tools_lists_what_the_path_lacks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let certutil = dir.path().join("certutil");
        std::fs::write(&certutil, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&certutil, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("pk12util"), "not executable").unwrap();

        assert_eq!(missing_tools(dir.path().as_os_str()), vec!["pk12util"]);
        assert_eq!(missing_tools(OsStr::new("")), vec!["certutil", "pk12util"]);
    }
}
