// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Message authentication. HMAC-SHA256 only (JWT HS256). `verify` is
//! constant-time.

use aws_lc_rs::hmac;

/// HMAC-SHA256 tag of `msg` under `key` (32 bytes).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&k, msg).as_ref().to_vec()
}

/// Constant-time check that `tag` is the HMAC-SHA256 of `msg` under `key`.
pub fn hmac_sha256_verify(key: &[u8], msg: &[u8], tag: &[u8]) -> bool {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::verify(&k, msg, tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let tag = hmac_sha256(b"k", b"message");
        assert_eq!(tag.len(), 32);
        assert!(hmac_sha256_verify(b"k", b"message", &tag));
        assert!(!hmac_sha256_verify(b"k", b"message!", &tag));
        assert!(!hmac_sha256_verify(b"k2", b"message", &tag));
    }
}
