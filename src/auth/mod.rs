// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Authentication primitives shared by the HTTP layer; owns the signing-secret
//! lifecycle. Identity rides a stateless signed JWT (see [`jwt`]).

pub mod jwt;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct SecretFile {
    /// 32 random bytes, lowercase-hex encoded.
    secret: String,
}

/// Loads the JWT signing secret from `HELIOS_JWT_SECRET_PATH`, else auto-creates
/// `./secret-jwt.json`. Multi-node needs the same secret on every node or 401s.
pub fn load_or_create_secret() -> Result<Vec<u8>> {
    let path = secret_file_path("HELIOS_JWT_SECRET_PATH", "secret-jwt.json");

    if path.exists() {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let parsed: SecretFile =
            serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
        let bytes = hex_decode(parsed.secret.trim())
            .with_context(|| format!("{}: secret is not valid hex", path.display()))?;
        return Ok(bytes);
    }

    let secret = crate::crypto::rand::bytes::<32>();
    let body = serde_json::to_string_pretty(&SecretFile {
        secret: hex_encode(&secret),
    })?;
    ensure_parent_dir(&path)?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    tracing::warn!(
        "auth: generated a new JWT signing secret at {} — set HELIOS_JWT_SECRET_PATH to a \
         persistent path for multi-node",
        path.display()
    );
    Ok(secret.to_vec())
}

/// Resolve a secret file path: `env_var` if set, else `default_file` in cwd.
/// Never the data-dir — that's a per-node cache under a shared store.
pub(crate) fn secret_file_path(env_var: &str, default_file: &str) -> std::path::PathBuf {
    match std::env::var(env_var) {
        Ok(v) if !v.trim().is_empty() => std::path::PathBuf::from(v.trim()),
        _ => std::path::PathBuf::from(default_file),
    }
}

/// `create_dir_all` the path's parent when it has a non-empty one (a bare
/// cwd-relative filename like `secret-jwt.json` has an empty parent — nothing to do).
pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        anyhow::bail!("odd-length hex string");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow::anyhow!("bad hex byte: {e}"))
        })
        .collect()
}
