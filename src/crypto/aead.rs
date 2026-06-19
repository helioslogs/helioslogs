// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! AES-256-GCM AEAD: caller owns the 12-byte nonce and AAD; `seal`/`open` use a
//! `ciphertext‖tag` layout compatible with the previous `aes-gcm` envelope.

use anyhow::{anyhow, Result};
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};

/// 12-byte GCM nonce length.
pub const NONCE_BYTES: usize = NONCE_LEN;

/// An AES-256-GCM key ready for sealing/opening. Keyed once at startup.
pub struct AeadKey(LessSafeKey);

impl AeadKey {
    /// `key` must be exactly 32 bytes.
    pub fn new(key: &[u8]) -> Result<Self> {
        let unbound =
            UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow!("aead: invalid AES-256 key"))?;
        Ok(Self(LessSafeKey::new(unbound)))
    }

    /// Encrypt `plaintext`, binding `aad`. Returns `ciphertext‖tag`.
    pub fn seal(&self, nonce: &[u8; NONCE_BYTES], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut buf = plaintext.to_vec();
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(*nonce),
                Aad::from(aad),
                &mut buf,
            )
            .map_err(|_| anyhow!("aead: encrypt failed"))?;
        Ok(buf)
    }

    /// Decrypt `ct` (`ciphertext‖tag`) under `aad`. Fails on a wrong key, wrong
    /// AAD, or any tampering.
    pub fn open(&self, nonce: &[u8; NONCE_BYTES], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
        let mut buf = ct.to_vec();
        let pt = self
            .0
            .open_in_place(
                Nonce::assume_unique_for_key(*nonce),
                Aad::from(aad),
                &mut buf,
            )
            .map_err(|_| anyhow!("aead: decrypt failed — wrong key or tampered data"))?;
        Ok(pt.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_aad_binding() {
        let key = AeadKey::new(&[7u8; 32]).unwrap();
        let nonce = [3u8; NONCE_BYTES];
        let ct = key.seal(&nonce, b"v11", b"hello").unwrap();
        assert_eq!(key.open(&nonce, b"v11", &ct).unwrap(), b"hello");
        // Wrong AAD fails.
        assert!(key.open(&nonce, b"v12", &ct).is_err());
        // Tampered ciphertext fails.
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(key.open(&nonce, b"v11", &bad).is_err());
    }
}
