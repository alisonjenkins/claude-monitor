//! Looking after a profile directory before it is written to: is the
//! browser using it, and a copy of its certificate database to fall back on.

use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// The NSS database files a write can damage.
const DB_FILES: [&str; 2] = ["cert9.db", "key4.db"];
pub const BACKUP_PREFIX: &str = "claude-monitor-backup-";

pub fn has_database(dir: &Path) -> bool {
    dir.join("cert9.db").is_file()
}

/// Why the browser appears to be running on `dir`, if it does.
///
/// Firefox on Linux points `lock` at `<host>:+<pid>` and keeps an fcntl lock
/// on `.parentlock`. A `lock` naming a dead pid is a crash leftover, not a
/// running browser; one that cannot be read is treated as running.
pub fn in_use(dir: &Path) -> Option<String> {
    if let Ok(target) = fs::read_link(dir.join("lock")) {
        let target = target.to_string_lossy();
        match target
            .rsplit_once(":+")
            .and_then(|(_, pid)| pid.parse::<i32>().ok())
        {
            Some(pid) if pid_alive(pid) => return Some(format!("lock held by process {pid}")),
            Some(_) => {}
            None => return Some(format!("lock points at {target}")),
        }
    }
    if parentlock_held(&dir.join(".parentlock")) {
        return Some(".parentlock is locked".to_owned());
    }
    None
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// True when another process holds a lock on the file. A lock this process
/// holds itself is not reported, which is how POSIX locks behave.
fn parentlock_held(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    // SAFETY: `flock` is plain data; all-zero is a valid value for it.
    let mut probe: libc::flock = unsafe { std::mem::zeroed() };
    probe.l_type = libc::F_WRLCK as _;
    probe.l_whence = libc::SEEK_SET as _;
    // SAFETY: the fd is open for the duration of the call and `probe` is a
    // valid `flock` the kernel fills in.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut probe) };
    let unlocked: libc::c_short = libc::F_UNLCK as _;
    rc == 0 && probe.l_type != unlocked
}

/// Copies the certificate database into
/// `<dir>/claude-monitor-backup-<UTC ISO 8601>/` and returns that directory.
/// Never overwrites an earlier backup.
pub fn backup_database(dir: &Path, now: DateTime<Utc>) -> io::Result<PathBuf> {
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let mut target = dir.join(format!("{BACKUP_PREFIX}{stamp}"));
    let mut attempt = 1u32;
    loop {
        match fs::create_dir(&target) {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                target = dir.join(format!("{BACKUP_PREFIX}{stamp}-{attempt}"));
                attempt = attempt.saturating_add(1);
            }
            Err(e) => return Err(e),
        }
    }
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
    for name in DB_FILES {
        let source = dir.join(name);
        if source.is_file() {
            fs::copy(&source, target.join(name))?;
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::TimeZone;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn dead_pid() -> i32 {
        // SAFETY: the child calls only `_exit`, which is async-signal-safe.
        let pid = unsafe {
            let pid = libc::fork();
            if pid == 0 {
                libc::_exit(0);
            }
            pid
        };
        assert!(pid > 0);
        // SAFETY: reaps the child forked above.
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        pid
    }

    #[test]
    fn a_quiet_directory_is_not_in_use() {
        let dir = TempDir::new().unwrap();
        assert_eq!(in_use(dir.path()), None);
        fs::write(dir.path().join(".parentlock"), "").unwrap();
        assert_eq!(
            in_use(dir.path()),
            None,
            "an unlocked .parentlock is normal"
        );
    }

    #[test]
    fn a_lock_naming_a_live_process_means_in_use() {
        let dir = TempDir::new().unwrap();
        symlink(
            format!("127.0.1.1:+{}", std::process::id()),
            dir.path().join("lock"),
        )
        .unwrap();
        let reason = in_use(dir.path()).unwrap();
        assert!(reason.contains(&std::process::id().to_string()), "{reason}");
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_stale() {
        let dir = TempDir::new().unwrap();
        symlink(
            format!("127.0.1.1:+{}", dead_pid()),
            dir.path().join("lock"),
        )
        .unwrap();
        assert_eq!(in_use(dir.path()), None);
    }

    #[test]
    fn a_lock_that_cannot_be_understood_counts_as_in_use() {
        let dir = TempDir::new().unwrap();
        symlink("garbage", dir.path().join("lock")).unwrap();
        assert!(in_use(dir.path()).is_some());
    }

    #[test]
    fn a_parentlock_another_process_holds_means_in_use() {
        let dir = TempDir::new().unwrap();
        let lock = dir.path().join(".parentlock");
        fs::write(&lock, "").unwrap();
        let path = std::ffi::CString::new(lock.to_str().unwrap()).unwrap();
        let mut pipe = [0i32; 2];
        // SAFETY: plain pipe(2) into a two-element array.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: the child only calls async-signal-safe functions (open,
        // fcntl, write, pause, _exit) before it is killed.
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            // SAFETY: as above; `flock` is plain data.
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_RDWR);
                let mut want: libc::flock = std::mem::zeroed();
                want.l_type = libc::F_WRLCK as _;
                want.l_whence = libc::SEEK_SET as _;
                let rc = libc::fcntl(fd, libc::F_SETLK, &mut want);
                let byte = u8::from(rc == 0);
                libc::write(pipe[1], std::ptr::addr_of!(byte).cast(), 1);
                libc::pause();
                libc::_exit(0);
            }
        }
        let mut byte = 0u8;
        // SAFETY: reads one byte into a local the pipe's write end filled.
        let n = unsafe { libc::read(pipe[0], std::ptr::addr_of_mut!(byte).cast(), 1) };
        let held = n == 1 && byte == 1;
        let seen = in_use(dir.path());
        // SAFETY: reaps the child this test forked.
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
        assert!(held, "the child could not take the lock");
        assert_eq!(seen.as_deref(), Some(".parentlock is locked"));
        assert_eq!(in_use(dir.path()), None, "released when the holder exits");
    }

    #[test]
    fn the_backup_copies_both_databases_under_an_iso_8601_utc_name() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("cert9.db"), "cert-bytes").unwrap();
        fs::write(dir.path().join("key4.db"), "key-bytes").unwrap();
        fs::write(dir.path().join("prefs.js"), "not copied").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 13, 9, 5, 7).unwrap();

        let backup = backup_database(dir.path(), now).unwrap();

        assert_eq!(
            backup.file_name().unwrap(),
            "claude-monitor-backup-20261013T090507Z"
        );
        assert_eq!(fs::read(backup.join("cert9.db")).unwrap(), b"cert-bytes");
        assert_eq!(fs::read(backup.join("key4.db")).unwrap(), b"key-bytes");
        assert!(!backup.join("prefs.js").exists());
        assert_eq!(
            fs::read(dir.path().join("cert9.db")).unwrap(),
            b"cert-bytes"
        );
    }

    #[test]
    fn a_second_backup_in_the_same_second_gets_its_own_directory() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("cert9.db"), "1").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 13, 9, 5, 7).unwrap();

        let first = backup_database(dir.path(), now).unwrap();
        fs::write(dir.path().join("cert9.db"), "2").unwrap();
        let second = backup_database(dir.path(), now).unwrap();

        assert_ne!(first, second);
        assert_eq!(fs::read(first.join("cert9.db")).unwrap(), b"1");
        assert_eq!(fs::read(second.join("cert9.db")).unwrap(), b"2");
    }
}
