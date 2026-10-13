//! `claude-monitor web setup-browser` as the real binary against real
//! `certutil`/`pk12util` and a temporary HOME. Nothing here touches the
//! user's own profiles.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod nss_support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");
const CLIENT: &str = "CN=claude-monitor web client";
const CA: &str = "claude-monitor local CA";

struct World {
    home: TempDir,
    state: TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
            state: TempDir::new().unwrap(),
        }
    }

    fn h(&self) -> &Path {
        self.home.path()
    }

    /// A Firefox profile listed in profiles.ini with an empty NSS database.
    fn firefox_profile(&self, name: &str) -> PathBuf {
        let root = self.h().join(".mozilla/firefox");
        let dir = root.join(format!("{name}.default"));
        nss_support::new_db(&dir);
        let ini = root.join("profiles.ini");
        let mut text = fs::read_to_string(&ini).unwrap_or_default();
        let n = text.matches("[Profile").count();
        text.push_str(&format!(
            "[Profile{n}]\nName={name}\nIsRelative=1\nPath={name}.default\n\n"
        ));
        fs::write(&ini, text).unwrap();
        dir
    }

    fn cm(&self, args: &[&str]) -> Output {
        self.cm_with(&[], args)
    }

    fn cm_json(&self, args: &[&str]) -> Output {
        self.cm_with(&["--json"], args)
    }

    fn cm_with(&self, global: &[&str], args: &[&str]) -> Output {
        Command::new(BIN)
            .args(global)
            .args(["--no-refresh", "--state-dir"])
            .arg(self.state.path())
            .args(["web", "setup-browser"])
            .args(args)
            .env_clear()
            .env("HOME", self.h())
            .env("PATH", std::env::var_os("PATH").unwrap())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "stdout: {}\nstderr: {}", out(o), err(o));
    out(o)
}

fn backups(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("claude-monitor-backup-"))
        .collect();
    names.sort();
    names
}

fn empty_pw(world: &World) -> PathBuf {
    let path = world.h().join("empty-pw");
    fs::write(&path, "\n").unwrap();
    path
}

fn count(listing: &str, needle: &str) -> usize {
    listing.lines().filter(|l| l.contains(needle)).count()
}

fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            }
            found.push((entry.path(), meta.len()));
        }
    }
    found.sort();
    found
}

#[test]
fn the_first_run_imports_and_the_second_does_nothing_and_says_so() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");

    let first = ok(&w.cm(&["--browser", "firefox"]));
    let listing = nss_support::list(&profile);
    let second = ok(&w.cm(&["--browser", "firefox"]));

    assert!(
        first.contains("imported (CA, client certificate)"),
        "{first}"
    );
    assert_eq!(count(&listing, CA), 1, "{listing}");
    assert_eq!(count(&listing, CLIENT), 1, "{listing}");
    assert!(second.contains("already up to date"), "{second}");
    assert!(second.contains("nothing to do"), "{second}");
    assert_eq!(nss_support::list(&profile), listing);
    assert_eq!(backups(&profile).len(), 1, "the no-op made no backup");
}

#[test]
fn a_new_client_certificate_replaces_the_old_one_instead_of_sitting_beside_it() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    ok(&w.cm(&["--browser", "firefox"]));
    let tls = w.state.path().join("tls");
    fs::remove_file(tls.join("client.pem")).unwrap();
    fs::remove_file(tls.join("client.key")).unwrap();

    let again = ok(&w.cm(&["--browser", "firefox"]));

    assert!(again.contains("imported (client certificate)"), "{again}");
    let listing = nss_support::list(&profile);
    assert_eq!(count(&listing, "claude-monitor web client"), 1, "{listing}");
    let keys = nss_support::keys(&profile, &empty_pw(&w));
    assert_eq!(count(&keys, "claude-monitor web client"), 1, "{keys}");
    assert_eq!(backups(&profile).len(), 2, "one backup per run that wrote");
}

#[test]
fn a_changed_ca_is_replaced_in_place() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    ok(&w.cm(&["--browser", "firefox"]));
    let tls = w.state.path().join("tls");
    for f in ["ca.pem", "ca.key"] {
        fs::remove_file(tls.join(f)).unwrap();
    }

    let again = ok(&w.cm(&["--browser", "firefox"]));

    assert!(
        again.contains("imported (CA, client certificate)"),
        "{again}"
    );
    let listing = nss_support::list(&profile);
    assert_eq!(count(&listing, CA), 1, "{listing}");
    assert_eq!(count(&listing, "claude-monitor web client"), 1, "{listing}");
}

#[test]
fn relative_absolute_and_several_profiles_and_zen_are_all_handled() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let first = w.firefox_profile("one");
    let second = w.firefox_profile("two");
    let elsewhere = TempDir::new().unwrap().keep().join("abs");
    nss_support::new_db(&elsewhere);
    let ini = w.h().join(".mozilla/firefox/profiles.ini");
    let mut text = fs::read_to_string(&ini).unwrap();
    text.push_str(&format!(
        "[Profile2]\nName=abs\nIsRelative=0\nPath={}\n",
        elsewhere.display()
    ));
    fs::write(&ini, text).unwrap();
    let zen = w.h().join(".config/zen/z");
    nss_support::new_db(&zen);
    fs::write(
        w.h().join(".config/zen/profiles.ini"),
        "[Profile0]\nName=z\nIsRelative=1\nPath=z\n",
    )
    .unwrap();

    let stdout = ok(&w.cm(&[]));

    for dir in [&first, &second, &elsewhere, &zen] {
        let listing = nss_support::list(dir);
        assert_eq!(count(&listing, CA), 1, "{}: {listing}", dir.display());
        assert!(stdout.contains(&dir.display().to_string()), "{stdout}");
    }
    assert_eq!(stdout.matches("imported").count(), 4, "{stdout}");
    fs::remove_dir_all(elsewhere.parent().unwrap()).unwrap();
}

#[test]
fn a_profile_in_use_is_refused_until_forced() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    let mut running = Command::new("sleep").arg("60").spawn().unwrap();
    symlink(format!("127.0.1.1:+{}", running.id()), profile.join("lock")).unwrap();

    let refused = w.cm(&["--browser", "firefox"]);
    let listing_after_refusal = nss_support::list(&profile);
    let backups_after_refusal = backups(&profile);
    let forced = w.cm(&["--browser", "firefox", "--force"]);
    running.kill().unwrap();
    running.wait().unwrap();

    assert_eq!(refused.status.code(), Some(3), "{}", err(&refused));
    assert!(
        err(&refused).contains("refused: in use"),
        "{}",
        err(&refused)
    );
    assert_eq!(count(&listing_after_refusal, CA), 0);
    assert!(backups_after_refusal.is_empty());
    assert!(ok(&forced).contains("imported"));
    assert_eq!(count(&nss_support::list(&profile), CA), 1);
}

#[test]
fn a_profile_with_a_primary_password_is_skipped_without_hanging() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let plain = w.firefox_profile("plain");
    let locked = w.h().join(".mozilla/firefox/locked.default");
    let pw = w.h().join("primary-pw");
    fs::write(&pw, "primary\n").unwrap();
    nss_support::new_db_with_password(&locked, &pw);
    let ini = w.h().join(".mozilla/firefox/profiles.ini");
    let mut text = fs::read_to_string(&ini).unwrap();
    text.push_str("[Profile1]\nName=locked\nIsRelative=1\nPath=locked.default\n");
    fs::write(&ini, text).unwrap();

    let started = std::time::Instant::now();
    let result = w.cm(&["--browser", "firefox"]);

    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(result.status.code(), Some(3), "{}", err(&result));
    assert!(
        err(&result).contains("skipped: a primary password"),
        "{}",
        err(&result)
    );
    assert_eq!(
        count(&nss_support::list(&plain), CA),
        1,
        "the plain one still works"
    );
    assert_eq!(count(&nss_support::list(&locked), CA), 0);
    assert!(backups(&locked).is_empty());
}

#[test]
fn the_backup_is_named_with_a_utc_iso_8601_stamp_and_holds_the_old_databases() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    let before_cert = fs::read(profile.join("cert9.db")).unwrap();

    ok(&w.cm(&["--browser", "firefox"]));

    let names = backups(&profile);
    assert_eq!(names.len(), 1, "{names:?}");
    let stamp = names[0].strip_prefix("claude-monitor-backup-").unwrap();
    let bytes = stamp.as_bytes();
    assert_eq!(bytes.len(), 16, "{stamp}");
    assert!(bytes[..8].iter().all(u8::is_ascii_digit), "{stamp}");
    assert_eq!(bytes[8], b'T');
    assert!(bytes[9..15].iter().all(u8::is_ascii_digit), "{stamp}");
    assert_eq!(bytes[15], b'Z');
    let backup = profile.join(&names[0]);
    assert_eq!(fs::read(backup.join("cert9.db")).unwrap(), before_cert);
    assert!(backup.join("key4.db").is_file());
}

#[test]
fn auto_select_inserts_replaces_in_place_and_keeps_the_rest() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    let user_js = profile.join("user.js");
    fs::write(&user_js, "user_pref(\"mine\", 1);\n").unwrap();
    fs::write(profile.join("prefs.js"), "// prefs\n").unwrap();

    ok(&w.cm(&["--browser", "firefox"]));
    assert_eq!(
        fs::read_to_string(&user_js).unwrap(),
        "user_pref(\"mine\", 1);\n",
        "absent without --auto-select"
    );

    let on = ok(&w.cm(&["--browser", "firefox", "--auto-select"]));
    let written = fs::read_to_string(&user_js).unwrap();
    assert!(on.contains("auto-select"), "{on}");
    assert!(written.starts_with("user_pref(\"mine\", 1);\n"));
    assert!(written.contains("// claude-monitor begin\n"));
    assert!(written.contains("security.default_personal_cert"));

    let stale = written.replace("Select Automatically", "Ask Every Time");
    fs::write(&user_js, format!("{stale}user_pref(\"after\", 2);\n")).unwrap();
    ok(&w.cm(&["--browser", "firefox", "--auto-select"]));
    let fixed = fs::read_to_string(&user_js).unwrap();
    assert!(fixed.contains("Select Automatically") && !fixed.contains("Ask Every Time"));
    assert_eq!(fixed.matches("// claude-monitor begin").count(), 1);
    assert!(fixed.ends_with("user_pref(\"after\", 2);\n"), "{fixed}");
    assert_eq!(
        fs::read_to_string(profile.join("prefs.js")).unwrap(),
        "// prefs\n"
    );
}

#[test]
fn a_dry_run_reports_what_would_change_and_writes_nothing() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");
    let home_before = tree(w.h());
    let list_before = nss_support::list(&profile);

    let stdout = ok(&w.cm(&["--browser", "firefox", "--auto-select", "--dry-run"]));

    assert!(
        stdout.contains("would import (CA, client certificate, auto-select)"),
        "{stdout}"
    );
    assert_eq!(tree(w.h()), home_before);
    assert_eq!(nss_support::list(&profile), list_before);
    assert_eq!(
        fs::read_dir(w.state.path()).unwrap().count(),
        0,
        "no TLS material made"
    );
}

#[test]
fn chromium_gets_the_policy_printed_and_nothing_written_but_its_nss_database() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let nssdb = w.h().join(".pki/nssdb");
    nss_support::new_db(&nssdb);

    let stdout = ok(&w.cm(&["--browser", "chromium", "--auto-select"]));

    assert!(stdout.contains("AutoSelectCertificateForUrls"), "{stdout}");
    assert!(
        stdout.contains("/etc/chromium/policies/managed/"),
        "{stdout}"
    );
    assert!(stdout.contains("programs.chromium.extraOpts"), "{stdout}");
    assert!(stdout.contains("https://127.0.0.1:*"), "{stdout}");
    assert!(!nssdb.join("user.js").exists());
    assert_eq!(count(&nss_support::list(&nssdb), CA), 1);
    let plain = ok(&w.cm(&["--browser", "chromium"]));
    assert!(!plain.contains("AutoSelectCertificateForUrls"), "{plain}");
}

#[test]
fn json_output_has_the_documented_shape() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let profile = w.firefox_profile("main");

    let value: serde_json::Value =
        serde_json::from_str(&ok(&w.cm_json(&["--browser", "firefox"]))).unwrap();

    assert_eq!(value["ok"], true);
    assert_eq!(value["dry_run"], false);
    let target = &value["targets"][0];
    assert_eq!(target["browser"], "firefox");
    assert_eq!(target["name"], "main");
    assert_eq!(target["dir"], profile.display().to_string());
    assert_eq!(target["status"], "imported");
    assert_eq!(target["detail"]["ca"], true);
    assert_eq!(target["detail"]["client"], true);
    assert_eq!(target["detail"]["auto_select"], false);
    assert!(target["detail"]["backup"]
        .as_str()
        .unwrap()
        .contains("claude-monitor-backup-"));
    assert!(value["notes"].is_array());
    assert!(value["chromium_policy"].is_null());

    let again: serde_json::Value =
        serde_json::from_str(&ok(&w.cm_json(&["--browser", "firefox"]))).unwrap();
    assert_eq!(again["targets"][0]["status"], "up_to_date");
}

#[test]
fn an_explicit_profile_is_set_up_and_no_key_material_is_ever_printed() {
    if !nss_support::nss_tools_or_skip() {
        return;
    }
    let w = World::new();
    let custom = w.h().join("custom-nss");
    nss_support::new_db(&custom);

    let result = w.cm(&[
        "--browser",
        "chromium",
        "--profile",
        custom.to_str().unwrap(),
    ]);
    let stdout = ok(&result);

    assert!(stdout.contains("custom-nss"), "{stdout}");
    assert_eq!(count(&nss_support::list(&custom), CLIENT), 1);
    for stream in [out(&result), err(&result)] {
        assert!(!stream.contains("PRIVATE KEY"));
    }
    let leftovers: Vec<_> = fs::read_dir(w.state.path().join("tls"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("browser-setup-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "secret files left behind: {leftovers:?}"
    );
}
