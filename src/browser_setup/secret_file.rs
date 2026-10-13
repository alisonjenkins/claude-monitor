//! Short-lived 0600 files for the passwords and bundle NSS tools read, so
//! none of them appears on a command line another user can see.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// A private file removed when the value is dropped, on every exit path.
#[derive(Debug)]
pub struct SecretFile {
    path: PathBuf,
}

impl SecretFile {
    /// Creates `dir/<prefix>-<random>` readable by the owner only, from the
    /// moment it exists. `dir` must already be private.
    pub fn create(dir: &Path, prefix: &str, contents: &[u8]) -> io::Result<Self> {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{prefix}-{suffix}"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let guard = SecretFile { path };
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(guard)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn the_file_is_private_holds_the_contents_and_goes_on_drop() {
        let dir = TempDir::new().unwrap();
        let file = SecretFile::create(dir.path(), "pw", b"hunter2").unwrap();
        let path = file.path().to_owned();

        assert_eq!(fs::read(&path).unwrap(), b"hunter2");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(path.starts_with(dir.path()));
        drop(file);
        assert!(!path.exists());
    }

    #[test]
    fn the_file_is_removed_when_the_scope_unwinds_through_an_error() {
        let dir = TempDir::new().unwrap();
        let mut seen = None;
        let attempt = |seen: &mut Option<PathBuf>| -> Result<(), &'static str> {
            let file = SecretFile::create(dir.path(), "pw", b"x").unwrap();
            *seen = Some(file.path().to_owned());
            Err("a later step failed")
        };

        assert!(attempt(&mut seen).is_err());

        assert!(!seen.unwrap().exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn two_files_with_the_same_prefix_do_not_collide() {
        let dir = TempDir::new().unwrap();
        let a = SecretFile::create(dir.path(), "pw", b"a").unwrap();
        let b = SecretFile::create(dir.path(), "pw", b"b").unwrap();
        assert_ne!(a.path(), b.path());
    }
}
