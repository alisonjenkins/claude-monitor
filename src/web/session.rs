//! Server-side sessions: the browser holds only a random id.
//!
//! Nothing here is persisted, so restarting `serve` signs everyone out. The
//! id is not derived from the login code or from anything on disk, and each
//! session carries its own CSRF value.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeroize::Zeroizing;

use super::clock::Ticks;
use super::secret::{ct_eq, random_hex};
use super::WebError;

pub const IDLE_EXPIRY: Duration = Duration::from_secs(8 * 3600);
pub const ABSOLUTE_EXPIRY: Duration = Duration::from_secs(24 * 3600);
/// A full table drops its oldest session, so repeated logins cannot grow it.
pub const MAX_SESSIONS: usize = 64;

pub struct NewSession {
    pub id: Zeroizing<String>,
    pub csrf: Zeroizing<String>,
}

struct Entry {
    id: Zeroizing<String>,
    csrf: Zeroizing<String>,
    created: Duration,
    last_used: Duration,
}

pub struct Sessions {
    clock: Arc<dyn Ticks>,
    live: Mutex<Vec<Entry>>,
}

impl Sessions {
    pub fn new(clock: Arc<dyn Ticks>) -> Self {
        Self {
            clock,
            live: Mutex::new(Vec::new()),
        }
    }

    pub fn create(&self) -> Result<NewSession, WebError> {
        let (id, csrf) = (random_hex()?, random_hex()?);
        let now = self.clock.now();
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut live, now);
        while live.len() >= MAX_SESSIONS {
            live.remove(0);
        }
        live.push(Entry {
            id: id.clone(),
            csrf: csrf.clone(),
            created: now,
            last_used: now,
        });
        Ok(NewSession { id, csrf })
    }

    /// The CSRF value of the live session with this id, which counts as use.
    /// Every session is compared, so timing does not say how close an id was.
    pub fn lookup(&self, id: &str) -> Option<String> {
        let now = self.clock.now();
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut live, now);
        let mut found = None;
        for entry in live.iter_mut() {
            if ct_eq(&entry.id, id) {
                found = Some(entry);
            }
        }
        let entry = found?;
        entry.last_used = now;
        Some(entry.csrf.to_string())
    }
}

fn prune(live: &mut Vec<Entry>, now: Duration) {
    live.retain(|e| {
        now.saturating_sub(e.last_used) < IDLE_EXPIRY
            && now.saturating_sub(e.created) < ABSOLUTE_EXPIRY
    });
}
