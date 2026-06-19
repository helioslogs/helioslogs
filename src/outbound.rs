// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Shared outbound HTTP client for webhook delivery. One pooled client,
//! no redirects (SSRF hygiene), 10s timeout.

use anyhow::{bail, Result};
use std::sync::OnceLock;

pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("outbound http client")
    })
}

/// Validate a user-supplied outbound URL: http/https only.
pub fn validate_outbound_url(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| anyhow::anyhow!("bad url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => bail!("unsupported url scheme `{other}` — use http or https"),
    }
    if parsed.host_str().is_none() {
        bail!("url has no host");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_validation() {
        assert!(validate_outbound_url("https://example.com/hook").is_ok());
        assert!(validate_outbound_url("http://example.com").is_ok());
        assert!(validate_outbound_url("file:///etc/passwd").is_err());
        assert!(validate_outbound_url("ftp://example.com").is_err());
        assert!(validate_outbound_url("not a url").is_err());
    }
}
