// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Hashing. SHA-256 only (the one Helios uses: cert fingerprints, content
//! version tokens).

use aws_lc_rs::digest;

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = digest::digest(&digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// SHA-256 of `data` as lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256(data))
}
