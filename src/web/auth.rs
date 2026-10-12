//! The bearer token: generated per `serve` start, kept in a 0600 file, handed
//! to the browser once as an `HttpOnly` cookie.

use std::fmt;
use std::path::Path;

use super::WebError;

pub const TOKEN_FILE_NAME: &str = "web.token";
pub const COOKIE_NAME: &str = "cc_token";
pub const CSRF_HEADER: &str = "x-cc-token";

/// 256 bits from the OS RNG, hex encoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Result<Self, WebError> {
        Err(WebError::Unimplemented)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// Writes a fresh token to `<dir>/web.token` (0600), replacing any old one.
pub fn issue_token_file(dir: &Path) -> Result<Token, WebError> {
    let _ = dir;
    Err(WebError::Unimplemented)
}
