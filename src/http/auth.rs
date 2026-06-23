// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Authentication: the `Principal` extractor, JWT auth middleware, and `/api/auth/*`
//! handlers. Identity rides a stateless `Authorization: Bearer <jwt>`; the middleware
//! re-reads the user record each request (authz details + revocation check).

use axum::async_trait;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{header, request::Parts, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::jwt;
use crate::control::users::User;

use super::AppState;

/// Sliding-renewal token header; cross-origin callers need it in CORS expose-headers.
const REFRESH_HEADER: &str = "x-helios-token-refresh";

/// Prefix marking a bearer token as a REST API key (a JWT is base64 `eyJ…`).
const API_KEY_PREFIX: &str = "hlk_";

/// How stale a key's `last_used_at` may get before we write a fresh stamp.
const API_KEY_TOUCH_THROTTLE_MS: i64 = 60_000;

/// The authenticated caller behind a request, built by the auth middleware from the
/// token + user record. `active_env` is the request's `?env=`; see [`resolve_request_env`].
#[derive(Clone, Serialize, Debug)]
pub(crate) struct Principal {
    pub user_id: String,
    pub userid: String,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
    pub active_env: String,
    /// Display preferences echoed on `/me` + login so the UI hydrates timezone/theme.
    pub timezone: Option<String>,
    pub theme: Option<String>,
    pub palette: Option<String>,
}

impl Principal {
    fn from_user_env(u: &User, active_env: String) -> Self {
        Self {
            user_id: u.id.clone(),
            userid: u.userid.clone(),
            email: u.email.clone(),
            display_name: u.display_name.clone(),
            is_admin: u.is_admin,
            active_env,
            timezone: u.timezone.clone(),
            theme: u.theme.clone(),
            palette: u.palette.clone(),
        }
    }

    /// Synthesize a principal from an API key. `user_id` is the key id (no real
    /// user row), so RBAC sees an empty allowlist = unrestricted for the key's scope.
    fn from_api_key(k: &crate::control::api_keys::ApiKey, active_env: String) -> Self {
        Self {
            user_id: k.id.clone(),
            userid: k.name.clone(),
            email: String::new(),
            display_name: format!("API key: {}", k.name),
            is_admin: k.scopes.admin,
            active_env,
            timezone: None,
            theme: None,
            palette: None,
        }
    }
}

/// Effective env for a request: handler's explicit `?env=` wins, else the caller's
/// active env, else `DEFAULT_ENV`. RBAC is NOT enforced here — use [`enforce_env_access`].
pub(crate) fn resolve_request_env(
    query_env: Option<&str>,
    principal: &Principal,
) -> anyhow::Result<String> {
    if let Some(s) = query_env.map(str::trim).filter(|s| !s.is_empty()) {
        return crate::catalog::env_or_default(Some(s));
    }
    let active = principal.active_env.trim();
    if active.is_empty() {
        Ok(crate::catalog::DEFAULT_ENV.to_string())
    } else {
        crate::catalog::env_or_default(Some(active))
    }
}

/// Verify a non-admin user has access to `env`. Admins bypass; system envs are admin-only;
/// an empty allowlist is the "unrestricted" sentinel (every non-system env).
pub(crate) async fn enforce_env_access(
    control: &crate::control::Control,
    principal: &Principal,
    env: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    if principal.is_admin {
        return Ok(());
    }
    if env.starts_with('_') {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("env '{env}' is admin-only") })),
        ));
    }
    // Unrestricted sentinel: zero allowlist rows means every non-system env
    // and no per-index restriction.
    let rules = control
        .user_allowed(&principal.user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
    if rules.is_empty() {
        return Ok(());
    }
    if rules.iter().any(|r| r.env.eq_ignore_ascii_case(env)) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("no access to env '{env}'") })),
        ))
    }
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _s: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .ok_or_else(unauthorized_tuple)
    }
}

/// axum middleware gating `/api/*` (bar public routes): validates the token, re-reads the
/// user (revocation + admin), injects a [`Principal`], applies sliding renewal. 401/403 on failure.
pub(crate) async fn auth_layer(
    State(s): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();

    // Public endpoints: login, health, and ingest work without a token (gate behind a proxy
    // on an untrusted net). Non-`/api` shim paths fall through the check below.
    if path == "/api/auth/login"
        // First-run setup: reached before any user (and thus any token) exists.
        // Both self-close once `user_count() > 0`.
        || path == "/api/auth/setup"
        || path == "/api/auth/setup_status"
        || path == "/api/health"
        || path == "/api/ingest"
        || path == "/api/ingest/raw"
        || path == "/api/es/_bulk"
        || path == "/api/otlp/v1/logs"
        // SAML SP endpoints: reached before a session exists. `/api/admin/saml`
        // (config) is NOT here — it stays behind the admin gate.
        || path == "/api/auth/saml/status"
        || path == "/api/auth/saml/metadata"
        || path == "/api/auth/saml/login"
        || path == "/api/auth/saml/acs"
    {
        return next.run(req).await;
    }
    // Only gate the API surface; the SPA shell is served unauthenticated and
    // bootstraps auth itself.
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }

    let Some(token) = bearer_token(req.headers()) else {
        return unauthorized();
    };
    // API keys (Admin → API keys) ride the same bearer header but are static
    // secrets with their own admin/standard scope — handled before JWT decode.
    if token.starts_with(API_KEY_PREFIX) {
        return api_key_request(s, &token, req, next).await;
    }
    let claims = match jwt::decode_token(&token, &s.jwt_secret) {
        Ok(c) => c,
        Err(_) => return unauthorized(),
    };
    let user = match s.control.get_user_by_id(&claims.sub).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(e) => return internal_error(e),
    };
    // Revocation check: a stale token (logout / password change / admin
    // force-logout bumped the counter) no longer matches.
    if claims.cv != user.credentials_version {
        return unauthorized();
    }

    if path.starts_with("/api/admin/") && !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }

    // Demo mode: reject mutating APIs for the one restricted demo account only;
    // every other user keeps full write access. Agent chat and auth (e.g. logout)
    // stay open even for the demo user; ingest is public and returned earlier.
    if s.demo.restricts(&user.userid, &user.email) && is_demo_blocked_write(req.method(), path) {
        return demo_write_blocked();
    }

    let active_env = env_query(req.uri()).unwrap_or_default();
    req.extensions_mut()
        .insert(Principal::from_user_env(&user, active_env));

    let mut resp = next.run(req).await;

    // Sliding renewal: hand back a fresh token once the current one crosses the
    // renewal threshold, so active users never get logged out. Cap the threshold
    // at half the configured lifetime so short sessions still refresh mid-window.
    let renew_after =
        jwt::RENEW_AFTER_SECONDS.min(crate::runtime_config::auth_token_ttl_seconds() / 2);
    if chrono::Utc::now().timestamp() - claims.iat > renew_after {
        if let Ok(fresh) = jwt::mint(&user.id, user.credentials_version, &s.jwt_secret) {
            if let Ok(v) = axum::http::HeaderValue::from_str(&fresh) {
                resp.headers_mut().insert(REFRESH_HEADER, v);
            }
        }
    }
    resp
}

/// Authorize a request bearing an `hlk_` API key: validate the secret + expiry,
/// apply the admin gate, inject a synthesized [`Principal`], and best-effort stamp
/// `last_used_at`. No sliding renewal — API keys are long-lived static secrets.
async fn api_key_request(s: AppState, token: &str, mut req: Request, next: Next) -> Response {
    let key = match s.control.api_key_find(token).await {
        Ok(Some(k)) => k,
        Ok(None) => return unauthorized(),
        Err(e) => return internal_error(e),
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    if key.is_expired(now_ms) {
        return unauthorized();
    }
    // Scope gate: admin endpoints need the admin scope; the rest of `/api/*`
    // needs the standard API scope (which admin implies). A key scoped only for
    // MCP can't drive the REST surface.
    if req.uri().path().starts_with("/api/admin/") {
        if !key.scopes.admin {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "admin only" })),
            )
                .into_response();
        }
    } else if !key.scopes.allows_api() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "this API key is not authorized for the REST API" })),
        )
            .into_response();
    }
    // Throttled, off-path last-used stamp; a lost CAS race here is harmless.
    let stale = key
        .last_used_at
        .map(|t| now_ms - t > API_KEY_TOUCH_THROTTLE_MS)
        .unwrap_or(true);
    if stale {
        let control = s.control.clone();
        let id = key.id.clone();
        tokio::spawn(async move {
            let _ = control.api_key_touch(&id, now_ms).await;
        });
    }
    let active_env = env_query(req.uri()).unwrap_or_default();
    req.extensions_mut()
        .insert(Principal::from_api_key(&key, active_env));
    next.run(req).await
}

fn unauthorized() -> Response {
    let (code, body) = unauthorized_tuple();
    (code, body).into_response()
}

fn unauthorized_tuple() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
}

/// Pulls the bearer token out of the `Authorization` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let t = rest.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Reads the active-env hint from the request's `?env=` query param.
/// Empty / absent / unparseable → None.
fn env_query(uri: &axum::http::Uri) -> Option<String> {
    #[derive(Deserialize)]
    struct EnvQuery {
        env: Option<String>,
    }
    let env = axum::extract::Query::<EnvQuery>::try_from_uri(uri)
        .ok()?
        .0
        .env?;
    let env = env.trim();
    (!env.is_empty()).then(|| env.to_string())
}

// ---- handlers ---------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct LoginRequest {
    /// Userid or email — both look up the same row via NOCASE collation.
    pub login: String,
    pub password: String,
}

pub(super) async fn login_handler(
    State(s): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Response {
    let user = match s
        .control
        .authenticate(req.login.trim(), &req.password)
        .await
    {
        Ok(Some(u)) => {
            tracing::info!(login = %u.userid, "auth: login succeeded");
            u
        }
        Ok(None) => {
            // Audit-shaped warn so a brute-force pattern is visible by
            // searching `index:_helioslogs auth login_failed` in the UI.
            tracing::warn!(
                attempted_login = %req.login.trim(),
                event = "login_failed",
                "auth: invalid credentials"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid credentials" })),
            )
                .into_response();
        }
        Err(e) => return internal_error(e),
    };

    // SSO-only enforcement: when local login is disabled, only admins may use a
    // password (break-glass). Everyone else must go through SAML.
    if !user.is_admin {
        match s.control.saml_settings().await {
            Ok(cfg) if cfg.local_login_disabled => {
                tracing::warn!(
                    login = %user.userid,
                    event = "local_login_blocked",
                    "auth: password login disabled for non-admin (SSO-only)"
                );
                return (
                    StatusCode::FORBIDDEN,
                    Json(
                        json!({ "error": "Password login is disabled. Please sign in with SSO." }),
                    ),
                )
                    .into_response();
            }
            Ok(_) => {}
            Err(e) => return internal_error(e),
        }
    }

    let token = match jwt::mint(&user.id, user.credentials_version, &s.jwt_secret) {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };

    // Echo the admin-configured default env; the client applies it only when the
    // browser has no stored env preference (returning users keep their last env).
    let active_env = initial_env_for(&s.control, &user).await;
    (
        StatusCode::OK,
        Json(json!({
            "token": token,
            "user": Principal::from_user_env(&user, active_env),
        })),
    )
        .into_response()
}

/// Initial active env echoed at login: the admin-configured default (validated),
/// narrowed to an env the caller may actually reach. Falls back to `DEFAULT_ENV`.
async fn initial_env_for(control: &crate::control::Control, user: &User) -> String {
    let default_env = control
        .default_env()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::catalog::DEFAULT_ENV.to_string());
    if user.is_admin {
        return default_env;
    }
    // Non-admins with an allowlist that excludes the default land on their first
    // granted env instead of a view they can't open. Empty allowlist = unrestricted.
    let allowed = control.user_allowed(&user.id).await.unwrap_or_default();
    if allowed.is_empty()
        || allowed
            .iter()
            .any(|r| r.env.eq_ignore_ascii_case(&default_env))
    {
        return default_env;
    }
    allowed
        .into_iter()
        .map(|r| r.env)
        .next()
        .unwrap_or_else(|| crate::catalog::DEFAULT_ENV.to_string())
}

/// First-run probe: tells the SPA whether to show the setup screen instead of login.
/// Public + cheap; `needs_setup` is true only while zero users exist.
pub(super) async fn setup_status_handler(State(s): State<AppState>) -> Response {
    use crate::control::settings::{
        palette_or_default, KEY_THEME_DEFAULT_APPEARANCE, KEY_THEME_DEFAULT_PALETTE,
        THEME_DEFAULT_APPEARANCE,
    };
    // Theme defaults ride along (best-effort) so the pre-login UI and users
    // without a personal override render the admin-chosen look.
    let appearance = s
        .control
        .get_setting(KEY_THEME_DEFAULT_APPEARANCE)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| THEME_DEFAULT_APPEARANCE.into());
    let palette = palette_or_default(
        s.control
            .get_setting(KEY_THEME_DEFAULT_PALETTE)
            .await
            .ok()
            .flatten(),
    );
    // Demo mode rides along so the login page can flip into read-only mode and
    // pre-fill the throwaway demo account; creds are only advertised when demo is on.
    let (demo_login, demo_password) = if s.demo.enabled {
        (s.demo.login.clone(), s.demo.password.clone())
    } else {
        (None, None)
    };
    match s.control.user_count().await {
        Ok(n) => (
            StatusCode::OK,
            Json(json!({
                "needs_setup": n == 0,
                "default_appearance": appearance,
                "default_palette": palette,
                "demo_mode": s.demo.enabled,
                "demo_login": demo_login,
                "demo_password": demo_password,
            })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize)]
pub(super) struct SetupRequest {
    pub userid: String,
    pub password: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// First-run admin claim. Creates the very first user (admin) and logs them in.
/// Guarded by zero-users: once anyone exists this 409s permanently, so it can't be
/// used to mint extra admins. Exposed unauthenticated only because no token can
/// exist yet — a public deploy should set `HELIOS_ADMIN_*` to skip the wizard.
pub(super) async fn setup_handler(
    State(s): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Response {
    let userid = req.userid.trim();
    if userid.is_empty() {
        return bad_request("username is required");
    }
    if req.password.len() < 8 {
        return bad_request("password must be at least 8 characters");
    }

    // Re-check under the create call's CAS too, but fail fast here for a clean 409.
    match s.control.user_count().await {
        Ok(0) => {}
        Ok(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": "already set up" })),
            )
                .into_response();
        }
        Err(e) => return internal_error(e),
    }

    let email = req
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{userid}@localhost"));
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Administrator");

    let user = match s
        .control
        .create_user(userid, &email, display_name, &req.password, true)
        .await
    {
        Ok(u) => {
            tracing::warn!(login = %u.userid, event = "first_run_setup", "auth: first-run admin created");
            u
        }
        // A racing setup request won the CAS — surface as a clean conflict.
        Err(e) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": format!("{e:#}") })),
            )
                .into_response();
        }
    };

    let token = match jwt::mint(&user.id, user.credentials_version, &s.jwt_secret) {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };
    (
        StatusCode::OK,
        Json(json!({
            "token": token,
            "user": Principal::from_user_env(&user, crate::catalog::DEFAULT_ENV.to_string()),
        })),
    )
        .into_response()
}

/// Logout = revoke. Bumps `credentials_version` so every outstanding token stops
/// validating. Requires a valid token to identify whose credentials to bump.
pub(super) async fn logout_handler(State(s): State<AppState>, principal: Principal) -> Response {
    if let Err(e) = s.control.bump_credentials_version(&principal.user_id).await {
        return internal_error(e);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub(super) async fn me_handler(principal: Principal) -> Json<Value> {
    Json(json!({ "user": principal }))
}

#[derive(Deserialize)]
pub(super) struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

pub(super) async fn change_password_handler(
    State(s): State<AppState>,
    principal: Principal,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    if req.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "new password must be at least 8 characters" })),
        )
            .into_response();
    }
    // Always re-verify the current password — a stolen token shouldn't be
    // enough to lock a user out of their own account.
    match s
        .control
        .authenticate(&principal.userid, &req.current_password)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "current password is incorrect" })),
            )
                .into_response();
        }
        Err(e) => return internal_error(e),
    }
    if let Err(e) = s
        .control
        .set_password(&principal.user_id, &req.new_password)
        .await
    {
        return internal_error(e);
    }
    // A password change revokes every outstanding token; bump the counter, then mint
    // a fresh token for this caller so they aren't logged out by their own action.
    let new_cv = match s.control.bump_credentials_version(&principal.user_id).await {
        Ok(cv) => cv,
        Err(e) => return internal_error(e),
    };
    let token = match jwt::mint(&principal.user_id, new_cv, &s.jwt_secret) {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };
    (StatusCode::OK, Json(json!({ "ok": true, "token": token }))).into_response()
}

#[derive(Deserialize)]
pub(super) struct PreferencesRequest {
    pub timezone: Option<String>,
    pub theme: Option<String>,
    pub palette: Option<String>,
}

/// Self-service timezone/theme update for the calling user; omitted fields unchanged.
/// Echoes the updated user so the client can reconcile.
pub(super) async fn update_preferences_handler(
    State(s): State<AppState>,
    principal: Principal,
    Json(req): Json<PreferencesRequest>,
) -> Response {
    match s
        .control
        .set_user_preferences(
            &principal.user_id,
            req.timezone.as_deref(),
            req.theme.as_deref(),
            req.palette.as_deref(),
        )
        .await
    {
        Ok(user) => (
            StatusCode::OK,
            Json(json!({
                "user": Principal::from_user_env(&user, principal.active_env.clone()),
            })),
        )
            .into_response(),
        // Validation failures (e.g. a bad theme value) are the caller's fault.
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

/// True for a mutating request that demo mode rejects. Agent chat (`/api/agent/`),
/// auth (`/api/auth/`, e.g. logout), and alert dismiss/ack stay open; ingest is
/// public and already returned earlier. Read methods (GET/HEAD/OPTIONS) are always allowed.
fn is_demo_blocked_write(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    if !matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return false;
    }
    // Dismissing/acking alerts stays open even for the demo account.
    if (*method == Method::POST && path == "/api/alerts/dismiss-all")
        || (*method == Method::PATCH && path.starts_with("/api/alerts/"))
    {
        return false;
    }
    !(path.starts_with("/api/agent/") || path.starts_with("/api/auth/"))
}

/// 403 carrying `demo_mode: true` so the SPA can show a friendly read-only notice.
fn demo_write_blocked() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "This is a read-only demo \u{2014} changes are disabled.",
            "demo_mode": true,
        })),
    )
        .into_response()
}

fn internal_error(e: anyhow::Error) -> Response {
    tracing::error!("auth: {e:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}
