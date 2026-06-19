// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! SP metadata XML — the admin hands this (or its URL) to the IdP so ADFS/Okta
//! can register Helios as a relying party. v1 advertises HTTP-POST ACS,
//! unsigned AuthnRequests, and that we want signed assertions.

use super::config::SamlConfig;
use super::xml_escape;

const NS_MD: &str = "urn:oasis:names:tc:SAML:2.0:metadata";
const NS_PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const BINDING_POST: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";
const NAMEID_EMAIL: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

pub fn sp_metadata_xml(cfg: &SamlConfig) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="{NS_MD}" entityID="{entity}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="{NS_PROTOCOL}">
    <md:NameIDFormat>{NAMEID_EMAIL}</md:NameIDFormat>
    <md:AssertionConsumerService Binding="{BINDING_POST}" Location="{acs}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>"#,
        entity = xml_escape(cfg.sp_entity_id.trim()),
        acs = xml_escape(cfg.acs_url.trim()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_trimmed_entity_and_acs() {
        let cfg = SamlConfig {
            sp_entity_id: "  https://helios.example/sp  ".into(),
            acs_url: "https://helios.example/api/saml/acs".into(),
            ..Default::default()
        };
        let xml = sp_metadata_xml(&cfg);
        assert!(xml.contains(r#"entityID="https://helios.example/sp""#));
        assert!(xml.contains(r#"Location="https://helios.example/api/saml/acs""#));
        assert!(xml.contains("WantAssertionsSigned=\"true\""));
        assert!(xml.starts_with("<?xml"));
    }

    #[test]
    fn escapes_special_chars_in_attributes() {
        let cfg = SamlConfig {
            sp_entity_id: "a&b".into(),
            acs_url: "https://x/acs?a=1&b=2".into(),
            ..Default::default()
        };
        let xml = sp_metadata_xml(&cfg);
        assert!(xml.contains(r#"entityID="a&amp;b""#));
        assert!(xml.contains("a=1&amp;b=2"));
        // No raw ampersand-then-letter that would break XML parsing.
        assert!(!xml.contains("a&b"));
    }
}
