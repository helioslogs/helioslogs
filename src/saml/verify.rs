// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The security core: verify a SAML Response's XML signature against the pinned IdP
//! cert (via `xmlsig-lc-rs`; inline `<KeyInfo>` ignored), then read claims ONLY from
//! the assertion the signature covers — the defense against signature-wrapping.

use chrono::{DateTime, Utc};

use super::config::SamlConfig;

const NS_ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const NS_PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";
/// Clock-skew tolerance for the assertion validity window.
const SKEW_SECS: i64 = 120;

/// The trustworthy result of consuming a SAML response.
#[derive(Debug, Clone)]
pub struct VerifiedAssertion {
    /// The match key — `<NameID>` text, or the configured attribute's value.
    pub subject: String,
    /// `Assertion/@ID`, used for replay dedup.
    pub assertion_id: String,
    /// `Conditions/@NotOnOrAfter` (epoch secs) — the replay-record TTL.
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamlError {
    Disabled,
    NotConfigured,
    Xml(String),
    BadCert(String),
    SignatureInvalid(String),
    Unsigned,
    SignatureCoverage,
    StatusNotSuccess(String),
    ConditionsInvalid,
    AudienceMismatch,
    NoSubject,
}

impl std::fmt::Display for SamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SamlError::Disabled => write!(f, "SAML is not enabled"),
            SamlError::NotConfigured => write!(f, "SAML is not fully configured"),
            SamlError::Xml(e) => write!(f, "malformed SAML XML: {e}"),
            SamlError::BadCert(e) => write!(f, "pinned IdP certificate is invalid: {e}"),
            SamlError::SignatureInvalid(e) => write!(f, "signature verification failed: {e}"),
            SamlError::Unsigned => write!(f, "response/assertion is not signed"),
            SamlError::SignatureCoverage => {
                write!(
                    f,
                    "signature does not cover a single assertion (possible wrapping)"
                )
            }
            SamlError::StatusNotSuccess(c) => write!(f, "SAML status was not Success: {c}"),
            SamlError::ConditionsInvalid => write!(f, "assertion is expired or not yet valid"),
            SamlError::AudienceMismatch => write!(f, "audience restriction does not match this SP"),
            SamlError::NoSubject => write!(f, "no subject (NameID/attribute) in the assertion"),
        }
    }
}

impl std::error::Error for SamlError {}

/// Verify + validate a base64-decoded SAML Response XML and return the subject
/// to match against a Helios user. `now` is injectable for tests.
pub fn verify_and_extract(
    xml: &str,
    cfg: &SamlConfig,
    now: DateTime<Utc>,
) -> Result<VerifiedAssertion, SamlError> {
    if !cfg.enabled {
        return Err(SamlError::Disabled);
    }
    if !cfg.is_usable() {
        return Err(SamlError::NotConfigured);
    }

    // 1. Cryptographically verify the signature against the pinned cert.
    let signed_ids = verify_signature(xml, cfg)?;

    // 2. Parse and locate the assertion the signature actually covers.
    let doc = roxmltree::Document::parse(xml).map_err(|e| SamlError::Xml(e.to_string()))?;
    let assertion = signed_assertion(&doc, &signed_ids)?;

    // 3. Response status (best-effort — Status sits on the unsigned Response, so
    //    it's a friendly early-out, not the security boundary).
    if let Some(code) = status_code(&doc) {
        if code != STATUS_SUCCESS {
            return Err(SamlError::StatusNotSuccess(code));
        }
    }

    // 4. SAML-layer validation on the SIGNED assertion.
    validate_conditions(assertion, cfg, now)?;
    let subject = extract_subject(assertion, cfg)?;

    let assertion_id = assertion.attribute("ID").unwrap_or_default().to_string();
    let expires_at = conditions_not_on_or_after(assertion)
        .map(|t| t.timestamp())
        .unwrap_or_else(|| (now + chrono::Duration::minutes(5)).timestamp());

    Ok(VerifiedAssertion {
        subject,
        assertion_id,
        expires_at,
    })
}

/// Verify the enveloped XML signature against the pinned IdP cert, returning the
/// covered reference IDs. Inline `<KeyInfo>` is ignored — our key alone is the pin.
fn verify_signature(xml: &str, cfg: &SamlConfig) -> Result<Vec<String>, SamlError> {
    let key = xmlsig_lc_rs::PublicKey::from_cert_pem(&cfg.idp_cert_pem)
        .map_err(|e| SamlError::BadCert(e.to_string()))?;
    match xmlsig_lc_rs::verify(xml, &key) {
        Ok(v) if v.references.is_empty() => Err(SamlError::Unsigned),
        Ok(v) => Ok(v.ids().map(str::to_owned).collect()),
        Err(e) => {
            // A missing <Signature> reads as "not signed"; everything else is a
            // verification failure.
            let msg = e.to_string();
            if msg.contains("Signature") {
                Err(SamlError::Unsigned)
            } else {
                Err(SamlError::SignatureInvalid(msg))
            }
        }
    }
}

/// Find the single `<Assertion>` covered by the signature (signed directly or via
/// a signed `<Response>` ancestor). Zero or >1 (the wrapping shape) is rejected.
fn signed_assertion<'a, 'i>(
    doc: &'a roxmltree::Document<'i>,
    signed_ids: &[String],
) -> Result<roxmltree::Node<'a, 'i>, SamlError> {
    let covered = |n: &roxmltree::Node| -> bool {
        // The element itself is signed, or an ancestor (the Response) is.
        std::iter::once(*n).chain(n.ancestors()).any(|a| {
            a.attribute("ID")
                .is_some_and(|id| signed_ids.iter().any(|s| s == id))
        })
    };
    let mut hits = doc
        .descendants()
        .filter(|n| is_saml(n, NS_ASSERTION, "Assertion") && covered(n));
    let first = hits.next().ok_or(SamlError::SignatureCoverage)?;
    if hits.next().is_some() {
        return Err(SamlError::SignatureCoverage);
    }
    Ok(first)
}

fn validate_conditions(
    assertion: roxmltree::Node,
    cfg: &SamlConfig,
    now: DateTime<Utc>,
) -> Result<(), SamlError> {
    let conditions = child(&assertion, NS_ASSERTION, "Conditions");

    if let Some(c) = conditions {
        if let Some(nb) = c.attribute("NotBefore").and_then(parse_time) {
            if now + chrono::Duration::seconds(SKEW_SECS) < nb {
                return Err(SamlError::ConditionsInvalid);
            }
        }
        if let Some(na) = c.attribute("NotOnOrAfter").and_then(parse_time) {
            if now - chrono::Duration::seconds(SKEW_SECS) >= na {
                return Err(SamlError::ConditionsInvalid);
            }
        }
    }

    // Audience must name this SP. We require an AudienceRestriction (absence is
    // treated as a mismatch rather than silently accepted).
    let mut audiences = assertion
        .descendants()
        .filter(|n| is_saml(n, NS_ASSERTION, "Audience"))
        .filter_map(|n| n.text().map(|t| t.trim().to_string()))
        .peekable();
    if audiences.peek().is_none() {
        return Err(SamlError::AudienceMismatch);
    }
    if !audiences.any(|a| a == cfg.sp_entity_id.trim()) {
        return Err(SamlError::AudienceMismatch);
    }
    Ok(())
}

/// The match key: the configured attribute's value, else the Subject NameID.
fn extract_subject(assertion: roxmltree::Node, cfg: &SamlConfig) -> Result<String, SamlError> {
    if let Some(attr_name) = cfg
        .email_attr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let val = assertion
            .descendants()
            .filter(|n| is_saml(n, NS_ASSERTION, "Attribute"))
            .find(|n| n.attribute("Name") == Some(attr_name))
            .and_then(|a| {
                a.descendants()
                    .find(|n| is_saml(n, NS_ASSERTION, "AttributeValue"))
                    .and_then(|v| v.text())
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        return val.ok_or(SamlError::NoSubject);
    }
    assertion
        .descendants()
        .find(|n| is_saml(n, NS_ASSERTION, "Subject"))
        .and_then(|s| child(&s, NS_ASSERTION, "NameID"))
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(SamlError::NoSubject)
}

fn status_code(doc: &roxmltree::Document) -> Option<String> {
    doc.descendants()
        .find(|n| is_saml(n, NS_PROTOCOL, "StatusCode"))
        .and_then(|n| n.attribute("Value"))
        .map(|s| s.to_string())
}

fn conditions_not_on_or_after(assertion: roxmltree::Node) -> Option<DateTime<Utc>> {
    child(&assertion, NS_ASSERTION, "Conditions")?
        .attribute("NotOnOrAfter")
        .and_then(parse_time)
}

// ---- small XML helpers (namespace-aware, roxmltree) -------------------------

fn is_saml(n: &roxmltree::Node, ns: &str, local: &str) -> bool {
    n.is_element() && n.tag_name().name() == local && n.tag_name().namespace() == Some(ns)
}

fn child<'a, 'i>(
    n: &roxmltree::Node<'a, 'i>,
    ns: &str,
    local: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| is_saml(c, ns, local))
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures signed by signxml (xmlsec-class), ported from the saml-spike.
    const SIGNED: &str = include_str!("testdata/saml_response_signed.xml");
    const ATTACKER: &str = include_str!("testdata/saml_response_attacker.xml");
    const IDP_CERT: &str = include_str!("testdata/idp_cert.pem");
    // Matches gen_fixture.py.
    const AUDIENCE: &str = "https://helios.example.com/saml/metadata";
    const SUBJECT: &str = "alice@example.com";

    fn cfg() -> SamlConfig {
        SamlConfig {
            enabled: true,
            idp_cert_pem: IDP_CERT.to_string(),
            sp_entity_id: AUDIENCE.to_string(),
            ..Default::default()
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-06T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn legit_assertion_extracts_subject() {
        let v = verify_and_extract(SIGNED, &cfg(), now()).expect("valid");
        assert_eq!(v.subject, SUBJECT);
        assert!(!v.assertion_id.is_empty());
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let tampered = SIGNED.replace(SUBJECT, "attacker@evil.com");
        assert!(verify_and_extract(&tampered, &cfg(), now()).is_err());
    }

    #[test]
    fn untrusted_key_is_rejected() {
        let err = verify_and_extract(ATTACKER, &cfg(), now()).unwrap_err();
        assert!(matches!(err, SamlError::SignatureInvalid(_)), "got {err:?}");
    }

    #[test]
    fn unsigned_response_is_rejected() {
        // Strip the <Signature>…</Signature> element → no signature to verify.
        let start = SIGNED.find("<ds:Signature").expect("has signature");
        let end = SIGNED.find("</ds:Signature>").expect("has close") + "</ds:Signature>".len();
        let unsigned = format!("{}{}", &SIGNED[..start], &SIGNED[end..]);
        assert!(verify_and_extract(&unsigned, &cfg(), now()).is_err());
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let mut c = cfg();
        c.sp_entity_id = "https://someone-else.example.com/saml".to_string();
        assert_eq!(
            verify_and_extract(SIGNED, &c, now()).unwrap_err(),
            SamlError::AudienceMismatch
        );
    }

    #[test]
    fn expired_conditions_are_rejected() {
        let future = DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            verify_and_extract(SIGNED, &cfg(), future).unwrap_err(),
            SamlError::ConditionsInvalid
        );
    }

    #[test]
    fn disabled_or_unconfigured_short_circuits() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            verify_and_extract(SIGNED, &c, now()).unwrap_err(),
            SamlError::Disabled
        );
        let mut c = cfg();
        c.idp_cert_pem = String::new();
        assert_eq!(
            verify_and_extract(SIGNED, &c, now()).unwrap_err(),
            SamlError::NotConfigured
        );
    }
}
