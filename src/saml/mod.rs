// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! SAML 2.0 SP support: pin one trusted IdP, verify its signed assertions, map the
//! subject to an existing user (HTTP surface in [`crate::http::saml`]). v1 scope: one
//! IdP, signed assertion over HTTP-POST; no SLO/encryption/JIT provisioning.

mod config;
mod metadata;
mod request;
mod verify;

pub use config::SamlConfig;
pub use metadata::sp_metadata_xml;
pub use request::redirect_to_idp;
pub use verify::verify_and_extract;

/// Escape a string for inclusion in XML text or a double-quoted attribute.
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn escapes_all_five_entities() {
        assert_eq!(xml_escape(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&apos;");
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        assert_eq!(xml_escape("https://idp/sso?a=1"), "https://idp/sso?a=1");
        assert_eq!(xml_escape("résumé"), "résumé"); // non-ASCII passthrough
    }
}
