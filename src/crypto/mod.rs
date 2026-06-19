// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The single crypto boundary for Helios (AEAD, hashing, MACs, passwords,
//! signatures, randomness), backed by [`aws_lc_rs`]. FIPS-ness is a build property
//! via the `fips` feature; call sites are identical either way.

pub mod aead;
pub mod digest;
pub mod mac;
pub mod password;
pub mod rand;
pub mod tls;

pub use aead::AeadKey;

/// True when built against the FIPS-validated AWS-LC module *and* it initialized
/// in approved mode. Surfaced on the admin runtime-config view.
pub fn fips_active() -> bool {
    aws_lc_rs::try_fips_mode().is_ok()
}
