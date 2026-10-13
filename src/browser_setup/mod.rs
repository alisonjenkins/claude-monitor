//! `claude-monitor web setup-browser`: trust the install's CA and import its
//! client certificate into browser NSS databases, idempotently.

pub mod chromium_policy;
pub mod discover;
pub mod nss;
pub mod profile;
pub mod report;
pub mod runner;
pub mod secret_file;
pub mod setup;
pub mod userjs;
