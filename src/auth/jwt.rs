// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! JWT encode/decode/minting. HS256 with a shared secret, validated locally with
//! no DB round-trip; claims are minimal (`sub` + revocation hook `cv`).

use anyhow::{bail, Result};
use base64::Engine as _;
use chrono::Utc;
use serde::{Deserialize, Serialize};

const B64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The one header we ever emit. Pinning it (and exact-matching on decode) is the
/// `alg`-confusion / `alg:none` defense — we only accept HS256 tokens we issued.
const HEADER_JSON: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

fn header_b64() -> String {
    B64URL.encode(HEADER_JSON)
}

/// Upper bound on the sliding-renewal threshold: even with a long token lifetime,
/// active users get a fresh token via `X-Helios-Token-Refresh` at least this often.
/// The effective threshold is `min(this, ttl / 2)` (see the auth middleware).
pub const RENEW_AFTER_SECONDS: i64 = 24 * 60 * 60;

/// The signed claim set. See module docs for why it's this small.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the opaque `user_id`. The single thing the token asserts.
    pub sub: String,
    /// Credentials version at issue time — compared against the user record's
    /// counter on every request; a mismatch is the revocation signal.
    pub cv: u32,
    /// Issued-at (unix seconds). Drives sliding renewal.
    pub iat: i64,
    /// Expiry (unix seconds).
    pub exp: i64,
}

/// Encode `claims` as a compact HS256 JWT: `b64url(header).b64url(payload)`
/// signed with HMAC-SHA256, then `.b64url(sig)` appended.
pub fn encode_token(claims: &Claims, secret: &[u8]) -> Result<String> {
    let payload = B64URL.encode(serde_json::to_vec(claims)?);
    let signing_input = format!("{}.{}", header_b64(), payload);
    let sig = crate::crypto::mac::hmac_sha256(secret, signing_input.as_bytes());
    Ok(format!("{signing_input}.{}", B64URL.encode(sig)))
}

/// Verify the signature (constant-time) + header + expiry, returning the claims.
pub fn decode_token(token: &str, secret: &[u8]) -> Result<Claims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        bail!("jwt: malformed (expected 3 segments)");
    }
    // Reject anything but the exact HS256 header we issue (no alg:none, no RS/ES).
    if parts[0] != header_b64() {
        bail!("jwt: unexpected header / algorithm");
    }
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let sig = B64URL
        .decode(parts[2])
        .map_err(|_| anyhow::anyhow!("jwt: bad signature encoding"))?;
    if !crate::crypto::mac::hmac_sha256_verify(secret, signing_input.as_bytes(), &sig) {
        bail!("jwt: signature verification failed");
    }
    let claims: Claims = serde_json::from_slice(
        &B64URL
            .decode(parts[1])
            .map_err(|_| anyhow::anyhow!("jwt: bad payload encoding"))?,
    )
    .map_err(|_| anyhow::anyhow!("jwt: bad claims"))?;
    if Utc::now().timestamp() >= claims.exp {
        bail!("jwt: expired");
    }
    Ok(claims)
}

/// Mints a fresh token for `user_id` carrying credentials-version `cv`, valid for
/// the admin-configurable session lifetime (`auth_token_ttl_hours`) from now.
pub fn mint(user_id: &str, cv: u32, secret: &[u8]) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        cv,
        iat: now,
        exp: now + crate::runtime_config::auth_token_ttl_seconds(),
    };
    encode_token(&claims, secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_carries_claims() {
        let secret = b"test-secret-bytes";
        let token = mint("usr_abc", 3, secret).unwrap();
        let claims = decode_token(&token, secret).unwrap();
        assert_eq!(claims.sub, "usr_abc");
        assert_eq!(claims.cv, 3);
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn rejects_wrong_secret() {
        let token = mint("usr_abc", 1, b"secret-a").unwrap();
        assert!(decode_token(&token, b"secret-b").is_err());
    }

    #[test]
    fn rejects_expired_token() {
        let now = Utc::now().timestamp();
        let claims = Claims {
            sub: "usr_abc".into(),
            cv: 1,
            iat: now - 10_000,
            exp: now - 100,
        };
        let token = encode_token(&claims, b"secret").unwrap();
        assert!(decode_token(&token, b"secret").is_err());
    }
}
