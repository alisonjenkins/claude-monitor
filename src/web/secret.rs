//! Random secrets and the one way they are compared.

use zeroize::Zeroizing;

use super::WebError;

const SECRET_BYTES: usize = 32;

/// 256 bits from the OS RNG, hex encoded, wiped from memory when dropped.
pub fn random_hex() -> Result<Zeroizing<String>, WebError> {
    let mut bytes = Zeroizing::new([0u8; SECRET_BYTES]);
    getrandom::fill(bytes.as_mut()).map_err(|e| WebError::Random {
        detail: e.to_string(),
    })?;
    Ok(Zeroizing::new(
        bytes.iter().map(|b| format!("{b:02x}")).collect(),
    ))
}

/// Compares without stopping at the first differing byte, so response time
/// does not leak how much of a guess was right. Length is not secret.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
