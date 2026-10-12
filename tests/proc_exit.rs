// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::env::parse_proc_stat_start_time;
use claude_monitor::feed::{EventSource, ProcExitSource};

const REAL_OS_WAKE_TIMEOUT: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(300);

fn spawn_sleeper() -> Child {
    Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

fn start_time(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    parse_proc_stat_start_time(&stat).unwrap()
}

fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    child.wait().unwrap();
}

#[test]
fn killing_a_registered_process_wakes_and_reports_its_pid() {
    let mut source = ProcExitSource::new().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();
    source.register(pid, start_time(pid)).unwrap();
    assert!(source.drain_exited().is_empty(), "alive at registration");

    child.kill().unwrap();

    assert!(source.wait(REAL_OS_WAKE_TIMEOUT));
    assert_eq!(source.drain_exited(), vec![pid]);
    assert!(source.drain_exited().is_empty(), "drained once");
    child.wait().unwrap();
}

#[test]
fn registering_an_already_dead_pid_reports_it_at_once() {
    let mut source = ProcExitSource::new().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();
    let started = start_time(pid);
    child.kill().unwrap();
    child.wait().unwrap();

    source.register(pid, started).unwrap();

    assert!(source.wait(Duration::ZERO), "the exit is already a wake");
    assert_eq!(source.drain_exited(), vec![pid]);
}

#[test]
fn a_start_time_mismatch_is_reported_as_exited() {
    // The pid now belongs to a different process than the one recorded (pid
    // reuse), so the recorded process is gone; the live one is not watched.
    let mut source = ProcExitSource::new().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();

    source.register(pid, start_time(pid) + 1).unwrap();

    assert!(source.wait(Duration::ZERO));
    assert_eq!(source.drain_exited(), vec![pid]);
    assert!(
        child.try_wait().unwrap().is_none(),
        "the live process is untouched"
    );
    kill_and_reap(child);
}

#[test]
fn unregistered_processes_are_not_reported() {
    let mut source = ProcExitSource::new().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();
    source.register(pid, start_time(pid)).unwrap();
    source.unregister(pid);

    child.kill().unwrap();
    child.wait().unwrap();

    assert!(!source.wait(QUIET));
    assert!(source.drain_exited().is_empty());
}

#[test]
fn unregister_forgets_an_exit_that_was_not_drained() {
    let mut source = ProcExitSource::new().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();
    source.register(pid, start_time(pid)).unwrap();
    child.kill().unwrap();
    assert!(source.wait(REAL_OS_WAKE_TIMEOUT));

    source.unregister(pid);

    assert!(source.drain_exited().is_empty());
    child.wait().unwrap();
}

#[test]
fn registering_from_another_thread_wakes_a_blocked_wait() {
    let mut source = ProcExitSource::new().unwrap();
    let handle = source.handle();
    let (tx, rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let woke = source.wait(Duration::from_secs(10));
        tx.send(woke).unwrap();
    });

    let mut child = spawn_sleeper();
    let pid = child.id();
    handle.register(pid, start_time(pid)).unwrap();
    child.kill().unwrap();

    assert!(rx.recv_timeout(REAL_OS_WAKE_TIMEOUT).unwrap());
    assert_eq!(handle.drain_exited(), vec![pid]);
    waiter.join().unwrap();
    child.wait().unwrap();
}

#[test]
fn wait_times_out_without_an_exit() {
    let mut source = ProcExitSource::new().unwrap();
    let child = spawn_sleeper();
    let pid = child.id();
    source.register(pid, start_time(pid)).unwrap();

    let started = Instant::now();
    let woke = source.wait(Duration::from_millis(100));

    assert!(!woke);
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(source.drain_exited().is_empty());
    kill_and_reap(child);
}
