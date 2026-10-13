//! `claude-monitor web setup-browser`: trust the install's CA and import its
//! client certificate into browser NSS databases, idempotently.

pub mod discover;
pub mod runner;
