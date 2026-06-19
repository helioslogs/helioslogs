// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! SP-initiated login: build a minimal unsigned AuthnRequest and encode it for the
//! HTTP-Redirect binding (DEFLATE → base64 → percent-encode). v1 doesn't sign these.

use std::io::Write;

use base64::Engine;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use super::config::SamlConfig;
use super::xml_escape;

const NS_PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const NS_ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const BINDING_POST: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";

/// Build the IdP SSO redirect URL carrying the AuthnRequest and optional RelayState.
/// `request_id` / `issue_instant` are injected so the caller owns time/uuid.
pub fn redirect_to_idp(
    cfg: &SamlConfig,
    relay_state: &str,
    request_id: &str,
    issue_instant: &str,
) -> String {
    let xml = authn_request_xml(cfg, request_id, issue_instant);
    let b64 = base64::engine::general_purpose::STANDARD.encode(deflate(xml.as_bytes()));

    let mut url = cfg.idp_sso_url.trim().to_string();
    url.push(if url.contains('?') { '&' } else { '?' });
    url.push_str("SAMLRequest=");
    url.push_str(&percent_encode(&b64));
    if !relay_state.is_empty() {
        url.push_str("&RelayState=");
        url.push_str(&percent_encode(relay_state));
    }
    url
}

fn authn_request_xml(cfg: &SamlConfig, id: &str, issue_instant: &str) -> String {
    format!(
        r#"<samlp:AuthnRequest xmlns:samlp="{NS_PROTOCOL}" xmlns:saml="{NS_ASSERTION}" ID="{id}" Version="2.0" IssueInstant="{issue}" Destination="{dest}" AssertionConsumerServiceURL="{acs}" ProtocolBinding="{BINDING_POST}"><saml:Issuer>{issuer}</saml:Issuer></samlp:AuthnRequest>"#,
        id = xml_escape(id),
        issue = xml_escape(issue_instant),
        dest = xml_escape(cfg.idp_sso_url.trim()),
        acs = xml_escape(cfg.acs_url.trim()),
        issuer = xml_escape(cfg.sp_entity_id.trim()),
    )
}

/// Raw DEFLATE (no zlib header), per the SAML HTTP-Redirect binding.
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Percent-encode a query-component value (everything outside the unreserved set).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    fn cfg() -> SamlConfig {
        SamlConfig {
            idp_sso_url: "https://idp.example/sso".into(),
            acs_url: "https://helios.example/api/saml/acs".into(),
            sp_entity_id: "https://helios.example/sp".into(),
            ..Default::default()
        }
    }

    fn percent_decode(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }

    fn inflate(data: &[u8]) -> String {
        let mut d = DeflateDecoder::new(data);
        let mut s = String::new();
        d.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn deflate_roundtrips() {
        let original = b"the quick brown fox";
        assert_eq!(inflate(&deflate(original)), "the quick brown fox");
    }

    #[test]
    fn percent_encode_unreserved_passthrough() {
        assert_eq!(percent_encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a+b/c=d"), "a%2Bb%2Fc%3Dd");
    }

    #[test]
    fn redirect_url_carries_decodable_authn_request() {
        let c = cfg();
        let url = redirect_to_idp(&c, "state-123", "id-1", "2026-06-07T00:00:00Z");
        assert!(url.starts_with("https://idp.example/sso?SAMLRequest="));
        assert!(url.contains("&RelayState=state-123"));

        // Extract, decode, inflate -> must equal the unsigned AuthnRequest XML.
        let raw = url
            .split("SAMLRequest=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let b64 = percent_decode(raw);
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let xml = inflate(&compressed);
        assert_eq!(xml, authn_request_xml(&c, "id-1", "2026-06-07T00:00:00Z"));
        assert!(xml.contains(r#"Destination="https://idp.example/sso""#));
        assert!(xml.contains("<saml:Issuer>https://helios.example/sp</saml:Issuer>"));
    }

    #[test]
    fn relay_state_omitted_when_empty_and_amp_when_url_has_query() {
        let mut c = cfg();
        c.idp_sso_url = "https://idp.example/sso?tenant=acme".into();
        let url = redirect_to_idp(&c, "", "id-1", "2026-06-07T00:00:00Z");
        assert!(url.contains("?tenant=acme&SAMLRequest="));
        assert!(!url.contains("RelayState"));
    }
}
