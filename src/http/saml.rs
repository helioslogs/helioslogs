// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/auth/saml/*` (public SP endpoints) + `/api/admin/saml` (config). The IdP
//! POSTs a signed Response to [`acs_handler`]; we verify against the pinned cert,
//! match an existing user, mint the usual JWT, and redirect with it in the URL fragment.

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Redirect, Response};
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::auth::jwt;
use crate::control::settings::{
    KEY_SAML_ACS_URL, KEY_SAML_BUTTON_LABEL, KEY_SAML_EMAIL_ATTR, KEY_SAML_ENABLED,
    KEY_SAML_IDP_CERT, KEY_SAML_IDP_ENTITY_ID, KEY_SAML_IDP_SSO_URL, KEY_SAML_LOCAL_LOGIN_DISABLED,
    KEY_SAML_SP_ENTITY_ID,
};
use crate::saml;

use super::AppState;

/// SPA landing for the post-login redirect. The frontend `/sso` route reads the
/// `#token=` / `#error=` fragment.
fn sso_redirect(fragment: &str) -> Response {
    Redirect::to(&format!("/sso#{fragment}")).into_response()
}

// ---- public SP endpoints ----------------------------------------------------

/// `GET /api/auth/saml/status` — drives the "Sign in with SSO" button. Public.
pub(super) async fn status_handler(State(s): State<AppState>) -> Response {
    match s.control.saml_settings().await {
        Ok(cfg) => (
            StatusCode::OK,
            Json(json!({
                "enabled": cfg.can_initiate(),
                "label": cfg.button_label,
                "local_login_disabled": cfg.local_login_disabled,
            })),
        )
            .into_response(),
        // Fail closed: no SSO button if config can't be read.
        Err(_) => (
            StatusCode::OK,
            Json(json!({ "enabled": false, "label": saml::SamlConfig::default_button_label(), "local_login_disabled": false })),
        )
            .into_response(),
    }
}

/// `GET /api/auth/saml/metadata` — SP metadata XML for the IdP admin. Public.
pub(super) async fn metadata_handler(State(s): State<AppState>) -> Response {
    let cfg = match s.control.saml_settings().await {
        Ok(c) => c,
        Err(e) => return internal_err(e),
    };
    if cfg.sp_entity_id.trim().is_empty() || cfg.acs_url.trim().is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "SAML SP entity ID / ACS URL not configured" })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        saml::sp_metadata_xml(&cfg),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(super) struct LoginQuery {
    /// Optional post-login target (a local path); carried as RelayState.
    next: Option<String>,
}

/// `GET /api/auth/saml/login` — SP-initiated: redirect to the IdP. Public.
pub(super) async fn login_handler(
    State(s): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> Response {
    let cfg = match s.control.saml_settings().await {
        Ok(c) => c,
        Err(e) => return internal_err(e),
    };
    if !cfg.can_initiate() {
        return sso_redirect("error=SSO%20is%20not%20configured");
    }
    let relay = q.next.as_deref().filter(|p| is_safe_path(p)).unwrap_or("");
    let request_id = format!("_{}", uuid::Uuid::new_v4().simple());
    let issue_instant = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let url = saml::redirect_to_idp(&cfg, relay, &request_id, &issue_instant);
    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
pub(super) struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: String,
    #[serde(rename = "RelayState")]
    #[allow(dead_code)]
    relay_state: Option<String>,
}

/// `POST /api/auth/saml/acs` — Assertion Consumer Service. Public (the IdP, via
/// the browser, posts here). On success redirects into the SPA with a JWT.
pub(super) async fn acs_handler(
    State(s): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<AcsForm>,
) -> Response {
    let cfg = match s.control.saml_settings().await {
        Ok(c) => c,
        Err(e) => return internal_err(e),
    };

    // Decode base64 (HTTP-POST binding) → XML.
    let xml = match base64::engine::general_purpose::STANDARD.decode(form.saml_response.trim()) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return sso_fail("malformed SAMLResponse encoding"),
        },
        Err(_) => return sso_fail("SAMLResponse is not valid base64"),
    };

    // Verify signature + SAML conditions, extract the subject.
    let now = Utc::now();
    let assertion = match saml::verify_and_extract(&xml, &cfg, now) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(event = "saml_rejected", error = %e, "auth: SAML assertion rejected");
            return sso_fail("assertion rejected");
        }
    };

    // Replay guard: a given assertion may be consumed once.
    match s
        .control
        .saml_replay_check_and_record(
            &assertion.assertion_id,
            assertion.expires_at,
            now.timestamp(),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                event = "saml_replay",
                "auth: SAML assertion replay rejected"
            );
            return sso_fail("assertion already used");
        }
        Err(e) => return internal_err(e),
    }

    // Match-only: the subject must already be a Helios user (email, then userid).
    let user = match s.control.find_user_by_login(&assertion.subject).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            tracing::warn!(
                subject = %assertion.subject,
                event = "saml_no_user",
                "auth: SAML subject has no matching Helios user"
            );
            return sso_fail("no matching user");
        }
        Err(e) => return internal_err(e),
    };

    let token = match jwt::mint(&user.id, user.credentials_version, &s.jwt_secret) {
        Ok(t) => t,
        Err(e) => return internal_err(e),
    };
    tracing::info!(login = %user.userid, event = "saml_login", "auth: SAML login succeeded");
    sso_redirect(&format!("token={token}"))
}

fn sso_fail(_reason: &'static str) -> Response {
    // The reason is logged server-side; the user just sees a generic SSO error.
    sso_redirect("error=Single%20sign-on%20failed")
}

// ---- admin config -----------------------------------------------------------

/// `GET /api/admin/saml` — current config. The cert is never echoed; only a
/// SHA-256 fingerprint + a "set" flag are returned.
pub(super) async fn get_config_handler(State(s): State<AppState>) -> Response {
    let cfg = match s.control.saml_settings().await {
        Ok(c) => c,
        Err(e) => return internal_err(e),
    };
    (
        StatusCode::OK,
        Json(json!({
            "enabled": cfg.enabled,
            "idp_entity_id": cfg.idp_entity_id,
            "idp_sso_url": cfg.idp_sso_url,
            "sp_entity_id": cfg.sp_entity_id,
            "acs_url": cfg.acs_url,
            "email_attr": cfg.email_attr,
            "button_label": cfg.button_label,
            "local_login_disabled": cfg.local_login_disabled,
            "cert_set": !cfg.idp_cert_pem.trim().is_empty(),
            "cert_fingerprint": cert_fingerprint(&cfg.idp_cert_pem),
        })),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
pub(super) struct SamlConfigPatch {
    enabled: Option<bool>,
    idp_entity_id: Option<String>,
    idp_sso_url: Option<String>,
    /// New cert PEM. Empty string clears it; omitted leaves the existing one.
    idp_cert: Option<String>,
    sp_entity_id: Option<String>,
    acs_url: Option<String>,
    email_attr: Option<String>,
    button_label: Option<String>,
    local_login_disabled: Option<bool>,
}

/// `POST /api/admin/saml` — update config. Validates the cert parses and URLs
/// are https before persisting.
pub(super) async fn post_config_handler(
    State(s): State<AppState>,
    Json(p): Json<SamlConfigPatch>,
) -> Response {
    // Validate before writing anything.
    if let Some(cert) = p.idp_cert.as_deref() {
        let trimmed = cert.trim();
        if !trimmed.is_empty() && xmlsig_lc_rs::PublicKey::from_cert_pem(trimmed).is_err() {
            return bad_req("IdP certificate is not a valid PEM X.509 certificate (RSA or EC)");
        }
    }
    for (label, url) in [
        ("IdP SSO URL", p.idp_sso_url.as_deref()),
        ("ACS URL", p.acs_url.as_deref()),
    ] {
        if let Some(u) = url {
            let u = u.trim();
            if !u.is_empty() && !u.starts_with("https://") && !u.starts_with("http://localhost") {
                return bad_req(&format!("{label} must be an https:// URL"));
            }
        }
    }

    let writes: [(&str, Option<String>); 9] = [
        (KEY_SAML_ENABLED, p.enabled.map(|b| b.to_string())),
        (KEY_SAML_IDP_ENTITY_ID, p.idp_entity_id),
        (KEY_SAML_IDP_SSO_URL, p.idp_sso_url),
        (KEY_SAML_IDP_CERT, p.idp_cert),
        (KEY_SAML_SP_ENTITY_ID, p.sp_entity_id),
        (KEY_SAML_ACS_URL, p.acs_url),
        (KEY_SAML_EMAIL_ATTR, p.email_attr),
        (KEY_SAML_BUTTON_LABEL, p.button_label),
        (
            KEY_SAML_LOCAL_LOGIN_DISABLED,
            p.local_login_disabled.map(|b| b.to_string()),
        ),
    ];
    for (key, val) in writes {
        let Some(v) = val else { continue };
        let res = if v.trim().is_empty() {
            s.control.unset_setting(key).await
        } else {
            s.control.set_setting(key, v.trim()).await
        };
        if let Err(e) = res {
            return internal_err(e);
        }
    }
    get_config_handler(State(s)).await
}

// ---- helpers ----------------------------------------------------------------

/// Colon-separated SHA-256 of the cert's DER (the conventional fingerprint), or
/// `None` when no/invalid cert.
fn cert_fingerprint(pem: &str) -> Option<String> {
    use base64::Engine as _;
    // The cert DER is just the base64 body of the PEM block.
    let body: String = pem
        .lines()
        .skip_while(|l| !l.contains("BEGIN CERTIFICATE"))
        .skip(1)
        .take_while(|l| !l.contains("END CERTIFICATE"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()?;
    Some(
        crate::crypto::digest::sha256(&der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// Open-redirect guard: only a local, non-protocol-relative path.
fn is_safe_path(p: &str) -> bool {
    p.starts_with('/') && !p.starts_with("//")
}

fn internal_err(e: anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn bad_req(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}
