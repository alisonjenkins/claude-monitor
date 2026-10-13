//! `claude-monitor web setup-browser`: trust the install's CA and import its
//! client certificate into browser NSS databases, idempotently.

pub mod discover;
pub mod nss;
pub mod runner;
pub mod secret_file;
pub mod userjs;
