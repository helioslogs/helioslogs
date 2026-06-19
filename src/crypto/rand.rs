// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Randomness — the validated DRBG (FIPS build) or AWS-LC's RNG otherwise. All
//! key/nonce/secret/token generation goes through here.

use aws_lc_rs::rand::{SecureRandom, SystemRandom};

/// Fill `buf` with cryptographically secure random bytes. The system DRBG
/// failing is unrecoverable, so this panics rather than threading an error
/// through every key-generation site.
pub fn fill(buf: &mut [u8]) {
    SystemRandom::new()
        .fill(buf)
        .expect("crypto: system DRBG failed");
}

/// A fresh `[u8; N]` of random bytes.
pub fn bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    fill(&mut b);
    b
}

/// A random `u32` — for collision-resistant ID suffixes (not a security value).
pub fn u32() -> u32 {
    u32::from_le_bytes(bytes::<4>())
}

/// `n` uniformly-random alphanumeric chars (`A–Z a–z 0–9`, ~5.95 bits each).
/// Rejection-sampled so the distribution is unbiased.
pub fn alphanumeric(n: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    // 62 * 4 = 248: bytes 0..248 map evenly to a char; 248..256 are rejected.
    const LIMIT: u8 = 248;
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 64];
    while out.len() < n {
        fill(&mut buf);
        for &b in buf.iter() {
            if out.len() >= n {
                break;
            }
            if b < LIMIT {
                out.push(CHARS[(b % 62) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphanumeric_length_and_charset() {
        let s = alphanumeric(40);
        assert_eq!(s.len(), 40);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn bytes_are_not_constant() {
        assert_ne!(bytes::<32>(), [0u8; 32]);
    }
}
