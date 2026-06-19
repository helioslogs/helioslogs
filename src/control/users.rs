// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! User account types + password primitives: the API-safe [`User`] wire shape (no
//! hash) and PBKDF2 wrappers. CRUD lives on [`crate::control::Control`].

use anyhow::Result;
use serde::Serialize;

/// A user account, without the password hash — safe to serialize into API
/// responses.
#[derive(Serialize, Clone, Debug)]
pub struct User {
    pub id: String,
    pub userid: String,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
    pub created_at: String,
    /// Per-account display preferences. `None` = unset (the client falls back
    /// to its localStorage cache / instance default).
    pub timezone: Option<String>,
    pub theme: Option<String>,
    pub palette: Option<String>,
    /// Token-revocation counter; the auth middleware rejects JWTs whose `cv` is stale.
    #[serde(skip)]
    pub credentials_version: u32,
}

/// Hashes a password
pub(crate) fn hash_password(password: &str) -> Result<String> {
    crate::crypto::password::hash(password)
}

/// Verifies `password` against a stored hash. A malformed/legacy hash returns
/// `false` rather than erroring (treated as a non-match).
pub(crate) fn verify_password(password: &str, stored_hash: &str) -> bool {
    crate::crypto::password::verify(password, stored_hash)
}

/// Generates a random 20-character alphanumeric password (~119 bits of
/// entropy) for admin-provisioned accounts.
pub fn generate_password() -> String {
    crate::crypto::rand::alphanumeric(20)
}
