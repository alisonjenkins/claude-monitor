//! Single-use login codes.
//!
//! A code is the only thing in a login URL. It is held in memory, never
//! written anywhere, valid for [`LOGIN_CODE_TTL`] and for one redemption.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeroize::Zeroizing;

use super::clock::Ticks;
use super::secret::{ct_eq, random_hex};
use super::WebError;

pub const LOGIN_CODE_TTL: Duration = Duration::from_secs(300);

/// Codes valid at once: the printed one, a few from `web login`, no more. A
/// full table drops the oldest, so asking for codes cannot grow memory.
const MAX_LIVE_CODES: usize = 8;

struct Entry {
    code: Zeroizing<String>,
    expires: Duration,
}

pub struct LoginCodes {
    clock: Arc<dyn Ticks>,
    live: Mutex<Vec<Entry>>,
}

impl LoginCodes {
    pub fn new(clock: Arc<dyn Ticks>) -> Self {
        Self {
            clock,
            live: Mutex::new(Vec::new()),
        }
    }

    pub fn issue(&self) -> Result<Zeroizing<String>, WebError> {
        let code = random_hex()?;
        let now = self.clock.now();
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|e| e.expires > now);
        while live.len() >= MAX_LIVE_CODES {
            live.remove(0);
        }
        live.push(Entry {
            code: code.clone(),
            expires: now.saturating_add(LOGIN_CODE_TTL),
        });
        Ok(code)
    }

    /// True for a live code, which is then spent. Every live code is compared
    /// whatever the outcome, so timing does not say how close a guess was.
    pub fn redeem(&self, offered: &str) -> bool {
        let now = self.clock.now();
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|e| e.expires > now);
        let mut hit = None;
        for (index, entry) in live.iter().enumerate() {
            if ct_eq(&entry.code, offered) {
                hit = Some(index);
            }
        }
        match hit {
            Some(index) => {
                live.remove(index);
                true
            }
            None => false,
        }
    }
}
