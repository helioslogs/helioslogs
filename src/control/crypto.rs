// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Encryption envelope for the JSON control plane: a plaintext `schema_version`+
//! encoding header wrapping the payload, in `aes-256-gcm` (version bound as AAD) or
//! `none` mode. Key from `HELIOS_CONTROL_KEY_PATH`; key loss = control-data loss.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::crypto::aead::NONCE_BYTES;
use crate::crypto::AeadKey;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;
const ENC_AES: &str = "aes-256-gcm";
const ENC_NONE: &str = "none";

/// On-disk envelope. `nonce`/`ct` are present for `aes-256-gcm`; `data` (inline
/// readable JSON) is present for `none`.
#[derive(Serialize, Deserialize)]
struct Envelope {
    schema_version: u32,
    enc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// The control-plane sealing key, or `Disabled` when encryption is off. Cheap to
/// share (the cipher is keyed once at startup).
pub enum Crypto {
    Disabled,
    Aes(AeadKey),
}

impl Crypto {
    /// Build from config. `false` → `Disabled`; otherwise load the key from
    /// `HELIOS_CONTROL_KEY_PATH` or load-or-create `./secret-control.json`.
    pub fn new(encryption_enabled: bool) -> Result<Self> {
        if !encryption_enabled {
            return Ok(Crypto::Disabled);
        }
        let key_bytes = load_key()?;
        Ok(Crypto::Aes(AeadKey::new(&key_bytes)?))
    }

    /// Seal `plaintext` into an envelope, binding `schema_version` as AAD. In
    /// `Disabled` mode the payload is embedded as readable JSON (must parse as JSON).
    pub fn seal(&self, plaintext: &[u8], schema_version: u32) -> Result<Vec<u8>> {
        let env = match self {
            Crypto::Disabled => {
                let data: serde_json::Value = serde_json::from_slice(plaintext)
                    .context("control crypto: encryption-disabled payload must be JSON")?;
                Envelope {
                    schema_version,
                    enc: ENC_NONE.to_string(),
                    nonce: None,
                    ct: None,
                    data: Some(data),
                }
            }
            Crypto::Aes(cipher) => {
                let mut nonce_bytes = [0u8; NONCE_BYTES];
                crate::crypto::rand::fill(&mut nonce_bytes);
                let aad = schema_version.to_le_bytes();
                let ct = cipher.seal(&nonce_bytes, &aad, plaintext)?;
                Envelope {
                    schema_version,
                    enc: ENC_AES.to_string(),
                    nonce: Some(B64.encode(nonce_bytes)),
                    ct: Some(B64.encode(ct)),
                    data: None,
                }
            }
        };
        serde_json::to_vec_pretty(&env).context("control crypto: serialize envelope")
    }

    /// Open an envelope back to `(plaintext, schema_version)`. Reads `none`
    /// regardless of mode (migrate-on-read); rejects `aes-256-gcm` when disabled.
    pub fn open(&self, bytes: &[u8]) -> Result<(Vec<u8>, u32)> {
        let env: Envelope =
            serde_json::from_slice(bytes).context("control crypto: malformed envelope")?;
        match env.enc.as_str() {
            ENC_NONE => {
                let data = env
                    .data
                    .ok_or_else(|| anyhow!("control crypto: `none` envelope missing `data`"))?;
                let pt = serde_json::to_vec(&data)?;
                Ok((pt, env.schema_version))
            }
            ENC_AES => {
                let cipher = match self {
                    Crypto::Aes(c) => c,
                    Crypto::Disabled => bail!(
                        "control crypto: file is encrypted but encryption is disabled — \
                         provide the key (HELIOS_CONTROL_KEY_PATH) or re-enable control encryption"
                    ),
                };
                let nonce_vec = B64
                    .decode(
                        env.nonce
                            .ok_or_else(|| anyhow!("control crypto: missing nonce"))?,
                    )
                    .context("control crypto: bad nonce base64")?;
                let nonce: [u8; NONCE_BYTES] = nonce_vec
                    .try_into()
                    .map_err(|_| anyhow!("control crypto: nonce must be {NONCE_BYTES} bytes"))?;
                let ct = B64
                    .decode(
                        env.ct
                            .ok_or_else(|| anyhow!("control crypto: missing ct"))?,
                    )
                    .context("control crypto: bad ct base64")?;
                let aad = env.schema_version.to_le_bytes();
                let pt = cipher.open(&nonce, &aad, &ct)?;
                Ok((pt, env.schema_version))
            }
            other => bail!("control crypto: unknown encoding {other:?}"),
        }
    }
}

/// Persisted key file format (hex-encoded 32-byte key).
#[derive(Serialize, Deserialize)]
struct KeyFile {
    key: String,
}

/// Resolve the 32-byte control key from `HELIOS_CONTROL_KEY_PATH`, else
/// load-or-create `./secret-control.json`. Never the data-dir (a per-node cache).
fn load_key() -> Result<[u8; 32]> {
    let path = crate::auth::secret_file_path("HELIOS_CONTROL_KEY_PATH", "secret-control.json");
    if path.exists() {
        restrict_perms(&path);
        let txt = std::fs::read_to_string(&path)
            .with_context(|| format!("reading control key {}", path.display()))?;
        let kf: KeyFile = serde_json::from_str(&txt)
            .with_context(|| format!("parsing control key {}", path.display()))?;
        return parse_key_hex(&kf.key).with_context(|| format!("key in {}", path.display()));
    }
    // First run: generate, persist 0600, warn loudly.
    let key = crate::crypto::rand::bytes::<32>();
    crate::auth::ensure_parent_dir(&path)?;
    let kf = KeyFile {
        key: hex::encode(key),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&kf)?)
        .with_context(|| format!("writing control key {}", path.display()))?;
    restrict_perms(&path);
    tracing::warn!(
        path = %path.display(),
        "control: generated a new encryption key — BACK THIS UP. Key loss = control-data loss. \
         For multi-node, set HELIOS_CONTROL_KEY_PATH to a shared/persistent path on every node."
    );
    Ok(key)
}

/// Decode 64 hex chars into a 32-byte key, with a precise length error.
fn parse_key_hex(s: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(s.trim()).context("must be 64 hex characters")?;
    if raw.len() != 32 {
        bail!("must decode to 32 bytes (64 hex chars), got {}", raw.len());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    Ok(key)
}

#[cfg(unix)]
fn restrict_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_perms(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn aes() -> Crypto {
        let mut key = [7u8; 32];
        key[0] = 1;
        Crypto::Aes(AeadKey::new(&key).unwrap())
    }

    #[test]
    fn aes_roundtrip() {
        let c = aes();
        let pt = br#"{"hello":"world","n":42}"#;
        let sealed = c.seal(pt, 3).unwrap();
        // Ciphertext must not leak plaintext.
        assert!(!sealed.windows(5).any(|w| w == b"world"));
        let (out, ver) = c.open(&sealed).unwrap();
        assert_eq!(out, pt);
        assert_eq!(ver, 3);
    }

    #[test]
    fn nonce_is_fresh_per_write() {
        let c = aes();
        let a = c.seal(b"{}", 1).unwrap();
        let b = c.seal(b"{}", 1).unwrap();
        assert_ne!(a, b, "two seals must differ (fresh nonce)");
    }

    #[test]
    fn wrong_key_is_rejected() {
        let sealed = aes().seal(br#"{"x":1}"#, 1).unwrap();
        let other = Crypto::Aes(AeadKey::new(&[9u8; 32]).unwrap());
        assert!(other.open(&sealed).is_err());
    }

    #[test]
    fn aad_tamper_is_rejected() {
        let c = aes();
        let sealed = c.seal(br#"{"x":1}"#, 1).unwrap();
        // Flip the plaintext schema_version; GCM AAD binding must fail decrypt.
        let mut env: Envelope = serde_json::from_slice(&sealed).unwrap();
        env.schema_version = 2;
        let tampered = serde_json::to_vec(&env).unwrap();
        assert!(c.open(&tampered).is_err());
    }

    #[test]
    fn disabled_mode_is_readable_and_roundtrips() {
        let c = Crypto::Disabled;
        let pt = br#"{"users":["a","b"]}"#;
        let sealed = c.seal(pt, 5).unwrap();
        // Inspectable: the values appear in the file verbatim.
        let text = String::from_utf8(sealed.clone()).unwrap();
        assert!(text.contains("\"none\""));
        assert!(text.contains("users"));
        let (out, ver) = c.open(&sealed).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap(),
            serde_json::json!({"users":["a","b"]})
        );
        assert_eq!(ver, 5);
    }

    #[test]
    fn enabled_node_can_read_a_disabled_file() {
        // Migrate-on-read: turning encryption on must still read old plaintext.
        let plain = Crypto::Disabled.seal(br#"{"k":1}"#, 1).unwrap();
        let (out, _) = aes().open(&plain).unwrap();
        assert_eq!(out, br#"{"k":1}"#);
    }

    #[test]
    fn disabled_node_rejects_encrypted_file() {
        let sealed = aes().seal(br#"{"k":1}"#, 1).unwrap();
        assert!(Crypto::Disabled.open(&sealed).is_err());
    }

    #[test]
    fn key_hex_must_be_32_bytes() {
        assert!(parse_key_hex("00").is_err());
        assert!(parse_key_hex(&"ab".repeat(32)).is_ok()); // 64 hex chars
        assert!(parse_key_hex("zz").is_err());
    }
}
