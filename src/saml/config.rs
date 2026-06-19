// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Typed view of the single trusted-IdP SAML config. Stored as individual
//! `saml.*` keys in the control-plane settings doc (encrypted at rest) and
//! loaded via [`crate::control::Control::saml_settings`].

/// One trusted IdP + this SP's identity. v1 supports a single IdP.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SamlConfig {
    pub enabled: bool,
    /// IdP's SAML entityID (informational / metadata).
    pub idp_entity_id: String,
    /// IdP SSO endpoint we redirect to for SP-initiated login (HTTP-Redirect).
    pub idp_sso_url: String,
    /// IdP signing cert (PEM) — the pin; assertions must be signed by this key.
    pub idp_cert_pem: String,
    /// This SP's entityID, which is also the expected `AudienceRestriction`.
    pub sp_entity_id: String,
    /// This SP's Assertion Consumer Service URL (where the IdP POSTs the response).
    pub acs_url: String,
    /// Optional: a SAML attribute Name to match on instead of the `<NameID>`.
    pub email_attr: Option<String>,
    /// Login-button label on the sign-in page.
    pub button_label: String,
    /// SSO-only for non-admins; admins keep password login as break-glass.
    pub local_login_disabled: bool,
}

impl SamlConfig {
    pub fn default_button_label() -> String {
        "Sign in with SSO".to_string()
    }

    /// Fully usable for verifying assertions: enabled + the security-critical
    /// fields (pinned cert + audience) are set.
    pub fn is_usable(&self) -> bool {
        self.enabled && !self.idp_cert_pem.trim().is_empty() && !self.sp_entity_id.trim().is_empty()
    }

    /// Ready to initiate SP-initiated login (needs the IdP SSO URL on top).
    pub fn can_initiate(&self) -> bool {
        self.is_usable() && !self.idp_sso_url.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usable() -> SamlConfig {
        SamlConfig {
            enabled: true,
            idp_cert_pem: "-----BEGIN CERTIFICATE-----...".into(),
            sp_entity_id: "https://helios.example/sp".into(),
            ..Default::default()
        }
    }

    #[test]
    fn default_button_label_is_stable() {
        assert_eq!(SamlConfig::default_button_label(), "Sign in with SSO");
    }

    #[test]
    fn is_usable_requires_enabled_cert_and_audience() {
        assert!(usable().is_usable());

        let mut c = usable();
        c.enabled = false;
        assert!(!c.is_usable());

        let mut c = usable();
        c.idp_cert_pem = "   ".into(); // whitespace-only is empty
        assert!(!c.is_usable());

        let mut c = usable();
        c.sp_entity_id = String::new();
        assert!(!c.is_usable());
    }

    #[test]
    fn can_initiate_additionally_needs_sso_url() {
        let mut c = usable();
        assert!(!c.can_initiate()); // usable but no SSO URL
        c.idp_sso_url = "https://idp.example/sso".into();
        assert!(c.can_initiate());
        // Not usable -> cannot initiate even with a URL.
        c.enabled = false;
        assert!(!c.can_initiate());
    }
}
