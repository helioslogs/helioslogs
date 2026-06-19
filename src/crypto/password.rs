// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Password hashing — PBKDF2-HMAC-SHA256 (FIPS-approved). Stored format
//! `pbkdf2$sha256$<iterations>$<b64-salt>$<b64-hash>` carries the work factor.

use std::num::NonZeroU32;

use anyhow::{bail, Result};
use aws_lc_rs::pbkdf2;
use base64::Engine as _;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD_NO_PAD;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

// OWASP 2023 guidance for PBKDF2-HMAC-SHA256. Tests use far fewer rounds (the
// stored count makes verify work regardless) so the suite stays fast.
#[cfg(not(test))]
const ITERATIONS: u32 = 600_000;
#[cfg(test)]
const ITERATIONS: u32 = 10_000;

/// Hash `password` into the storable string form.
pub fn hash(password: &str) -> Result<String> {
    if password.is_empty() {
        bail!("password cannot be empty");
    }
    let salt = crate::crypto::rand::bytes::<SALT_LEN>();
    let mut out = [0u8; HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).unwrap(),
        &salt,
        password.as_bytes(),
        &mut out,
    );
    Ok(format!(
        "pbkdf2$sha256${ITERATIONS}${}${}",
        B64.encode(salt),
        B64.encode(out)
    ))
}

/// Verify `password` against a stored hash (malformed/unknown → `false`).
pub fn verify(password: &str, stored: &str) -> bool {
    let p: Vec<&str> = stored.split('$').collect();
    if p.len() != 5 || p[0] != "pbkdf2" || p[1] != "sha256" {
        return false;
    }
    let Ok(iters) = p[2].parse::<u32>() else {
        return false;
    };
    let Some(iters) = NonZeroU32::new(iters) else {
        return false;
    };
    let (Ok(salt), Ok(expected)) = (B64.decode(p[3]), B64.decode(p[4])) else {
        return false;
    };
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iters,
        &salt,
        password.as_bytes(),
        &expected,
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_roundtrip() {
        let h = hash("correct horse battery staple").unwrap();
        assert!(h.starts_with("pbkdf2$sha256$"));
        assert!(verify("correct horse battery staple", &h));
        assert!(!verify("wrong", &h));
    }

    #[test]
    fn salts_make_hashes_unique() {
        assert_ne!(hash("same").unwrap(), hash("same").unwrap());
    }

    #[test]
    fn empty_password_rejected() {
        assert!(hash("").is_err());
    }

    #[test]
    fn legacy_and_garbage_hashes_reject() {
        assert!(!verify("x", "$argon2id$v=19$m=19456,t=2,p=1$abc$def"));
        assert!(!verify("x", "not-a-hash"));
    }
}
