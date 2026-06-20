// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! HTTP surface for the agent: conversation CRUD and the SSE streaming
//! message endpoint. Plus the `/api/admin/agent` settings get/put pair.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use super::auth::Principal;
use super::AppState;
use crate::agent::settings::{self as agent_settings, AgentSettings, BedrockAuthMode, Provider};
use crate::agent::{build_provider, AgentEngine};
use crate::llm::{LlmEvent, LlmMessage, LlmRole};

// ---- conversation CRUD (user-scoped; not env-scoped) --------------------

pub(super) async fn list_conversations_handler(
    State(s): State<AppState>,
    principal: Principal,
) -> Response {
    match s.control.conv_list(&principal.user_id).await {
        Ok(items) => Json(json!({ "conversations": items })).into_response(),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct CreateConvRequest {
    #[serde(default)]
    pub title: Option<String>,
}

pub(super) async fn create_conversation_handler(
    State(s): State<AppState>,
    principal: Principal,
    Json(req): Json<CreateConvRequest>,
) -> Response {
    let title = req.title.unwrap_or_default();
    match s.control.conv_create(&principal.user_id, &title).await {
        Ok(meta) => Json(json!({ "conversation": meta })).into_response(),
        Err(e) => internal_error(e),
    }
}

pub(super) async fn get_conversation_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    match s.control.conv_get(&principal.user_id, &id).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => not_found(),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize)]
pub(super) struct RenameRequest {
    pub title: String,
}

pub(super) async fn rename_conversation_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(req): Json<RenameRequest>,
) -> Response {
    match s
        .control
        .conv_rename(&principal.user_id, &id, req.title.trim())
        .await
    {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => not_found(),
        Err(e) => internal_error(e),
    }
}

pub(super) async fn delete_conversation_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    match s.control.conv_delete(&principal.user_id, &id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => not_found(),
        Err(e) => internal_error(e),
    }
}

// ---- streaming message endpoint -----------------------------------------

#[derive(Deserialize)]
pub(super) struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub view_context: Option<Value>,
}

pub(super) async fn send_message_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Response {
    let owns = match s.control.conv_owns(&principal.user_id, &id).await {
        Ok(v) => v,
        Err(e) => return internal_error(e),
    };
    if !owns {
        return not_found();
    }
    if req.content.trim().is_empty() {
        return bad_request("empty message".into());
    }

    // Headroom to bridge SSE-writer slowness; the agent loop awaits `tx.send` so it's backpressured anyway.
    let (tx, rx) = mpsc::channel(64);

    let engine = AgentEngine {
        catalog: s.catalog.clone(),
        fields: s.fields,
        control: s.control.clone(),
    };
    let user_id = principal.user_id.clone();
    // Always run in the session's active env so tool calls target the right partitions; no URL override.
    let env = principal.active_env.clone();
    let content = req.content.clone();
    let view_context = req.view_context.clone();

    tokio::spawn(async move {
        if let Err(e) = engine
            .run_turn(id, &user_id, &env, &content, view_context, tx.clone())
            .await
        {
            // Surface unexpected internal errors to the client too.
            let _ = tx
                .send(crate::agent::AgentEvent::Error {
                    message: format!("{e:#}"),
                })
                .await;
        }
    });

    let event_stream: ReceiverStream<crate::agent::AgentEvent> = ReceiverStream::new(rx);
    let sse_stream = to_sse_stream(event_stream);
    Sse::new(sse_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

fn to_sse_stream(
    src: ReceiverStream<crate::agent::AgentEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    src.map(|evt| {
        let payload = serde_json::to_string(&evt).unwrap_or_else(|_| "{}".into());
        Ok(Event::default().data(payload))
    })
}

// ---- LLM settings (admin) ----------------------------------------------

pub(super) async fn get_llm_settings_handler(State(s): State<AppState>) -> Response {
    match AgentSettings::load(&s.control).await {
        Ok(cfg) => Json(redact(&cfg)).into_response(),
        Err(e) => internal_error(e),
    }
}

/// Public (any authenticated user) agent availability flag, so the chat panel
/// and monitor UI can show a disabled state without the admin-only settings.
pub(super) async fn agent_status_handler(State(s): State<AppState>) -> Response {
    match AgentSettings::load(&s.control).await {
        Ok(cfg) => Json(json!({ "enabled": cfg.enabled })).into_response(),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize)]
pub(super) struct UpdateLlmSettings {
    /// Master on/off switch for all agent functionality.
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    provider: Option<String>,
    /// Per-provider models; each persisted independently so switching providers preserves them.
    #[serde(default)]
    openai_model: Option<String>,
    #[serde(default)]
    anthropic_model: Option<String>,
    #[serde(default)]
    bedrock_model: Option<String>,
    #[serde(default)]
    openai_endpoint: Option<String>,
    /// `Some("")` clears; `Some("...")` sets; `None` leaves untouched (plaintext on the wire).
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    anthropic_endpoint: Option<String>,
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    bedrock_region: Option<String>,
    #[serde(default)]
    bedrock_auth_mode: Option<String>,
    /// AWS SigV4 keys. Same write-only semantics as the API keys.
    #[serde(default)]
    bedrock_access_key_id: Option<String>,
    #[serde(default)]
    bedrock_secret_access_key: Option<String>,
    #[serde(default)]
    bedrock_session_token: Option<String>,
    /// Admin override for `AWS_BEARER_TOKEN_BEDROCK`.
    #[serde(default)]
    bedrock_bearer_token: Option<String>,
}

pub(super) async fn put_llm_settings_handler(
    State(s): State<AppState>,
    Json(req): Json<UpdateLlmSettings>,
) -> Response {
    if let Some(b) = req.enabled {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_AGENT_ENABLED, &b.to_string())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.provider {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_PROVIDER, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.openai_model {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_MODEL_OPENAI, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.anthropic_model {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_MODEL_ANTHROPIC, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_model {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_MODEL_BEDROCK, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.openai_endpoint {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_OPENAI_ENDPOINT, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.openai_api_key {
        if let Err(e) = write_secret(&s, agent_settings::KEY_OPENAI_API_KEY, &v).await {
            return internal_error(e);
        }
    }
    if let Some(v) = req.anthropic_endpoint {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_ANTHROPIC_ENDPOINT, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.anthropic_api_key {
        if let Err(e) = write_secret(&s, agent_settings::KEY_ANTHROPIC_API_KEY, &v).await {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_region {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_BEDROCK_REGION, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_auth_mode {
        if let Err(e) = s
            .control
            .set_setting(agent_settings::KEY_BEDROCK_AUTH_MODE, v.trim())
            .await
        {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_access_key_id {
        if let Err(e) = write_secret(&s, agent_settings::KEY_BEDROCK_ACCESS_KEY_ID, &v).await {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_secret_access_key {
        if let Err(e) = write_secret(&s, agent_settings::KEY_BEDROCK_SECRET_ACCESS_KEY, &v).await {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_session_token {
        if let Err(e) = write_secret(&s, agent_settings::KEY_BEDROCK_SESSION_TOKEN, &v).await {
            return internal_error(e);
        }
    }
    if let Some(v) = req.bedrock_bearer_token {
        if let Err(e) = write_secret(&s, agent_settings::KEY_BEDROCK_BEARER_TOKEN, &v).await {
            return internal_error(e);
        }
    }
    match AgentSettings::load(&s.control).await {
        Ok(cfg) => Json(redact(&cfg)).into_response(),
        Err(e) => internal_error(e),
    }
}

/// Overlay an `UpdateLlmSettings` patch onto loaded settings for the test
/// path: present non-secret fields replace; secrets set when non-empty, clear
/// on empty string, and stay (using the stored key) when absent.
fn apply_overrides(cfg: &mut AgentSettings, req: UpdateLlmSettings) {
    if let Some(v) = req.provider.as_deref().and_then(Provider::parse) {
        cfg.provider = v;
    }
    if let Some(v) = req.openai_model {
        cfg.openai_model = v.trim().to_string();
    }
    if let Some(v) = req.anthropic_model {
        cfg.anthropic_model = v.trim().to_string();
    }
    if let Some(v) = req.bedrock_model {
        cfg.bedrock_model = v.trim().to_string();
    }
    if let Some(v) = req.openai_endpoint {
        cfg.openai_endpoint = v.trim().to_string();
    }
    if let Some(v) = req.anthropic_endpoint {
        cfg.anthropic_endpoint = v.trim().to_string();
    }
    if let Some(v) = req.bedrock_region {
        cfg.bedrock_region = v.trim().to_string();
    }
    if let Some(v) = req
        .bedrock_auth_mode
        .as_deref()
        .and_then(BedrockAuthMode::parse)
    {
        cfg.bedrock_auth_mode = v;
    }
    overlay_secret(&mut cfg.openai_api_key, req.openai_api_key);
    overlay_secret(&mut cfg.anthropic_api_key, req.anthropic_api_key);
    overlay_secret(&mut cfg.bedrock_access_key_id, req.bedrock_access_key_id);
    overlay_secret(
        &mut cfg.bedrock_secret_access_key,
        req.bedrock_secret_access_key,
    );
    overlay_secret(&mut cfg.bedrock_session_token, req.bedrock_session_token);
    overlay_secret(&mut cfg.bedrock_bearer_token, req.bedrock_bearer_token);
    // Re-resolve the active model after provider / per-provider edits.
    cfg.model = cfg.active_model().to_string();
}

// None = leave stored key; Some("") = clear; Some(v) = use the typed value.
fn overlay_secret(slot: &mut Option<String>, patch: Option<String>) {
    if let Some(v) = patch {
        let t = v.trim();
        *slot = if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        };
    }
}

/// Connectivity check: build the configured provider and run one tiny
/// non-streaming-style completion, returning the model's reply (or the error).
/// The request body (same shape as the PUT) is overlaid on the saved settings
/// so unsaved form edits are tested; blank/absent secrets fall back to stored
/// keys, exactly like the redacted GET implies.
pub(super) async fn test_llm_handler(
    State(s): State<AppState>,
    Json(req): Json<UpdateLlmSettings>,
) -> Response {
    let mut cfg = match AgentSettings::load(&s.control).await {
        Ok(c) => c,
        Err(e) => return internal_error(e),
    };
    apply_overrides(&mut cfg, req);
    let provider = match build_provider(&cfg).await {
        Ok(p) => p,
        Err(e) => return Json(json!({ "ok": false, "error": format!("{e:#}") })).into_response(),
    };

    let system_text = "You are a connectivity test. Reply briefly.";
    let user_text = "Reply with exactly: Helios connection OK";
    let messages = vec![
        LlmMessage {
            role: LlmRole::System,
            content: Some(system_text.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        },
        LlmMessage {
            role: LlmRole::User,
            content: Some(user_text.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        },
    ];

    // Echo the exact request Helios sends, so the panel can show the full exchange.
    let request_json = json!([
        { "role": "system", "content": system_text },
        { "role": "user", "content": user_text },
    ]);

    let provider_str = match cfg.provider {
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Bedrock => "bedrock",
    };

    let mut stream = match provider.stream(messages, vec![], 0.0).await {
        Ok(s) => s,
        Err(e) => {
            return Json(json!({
                "ok": false,
                "error": format!("{e:#}"),
                "request": request_json,
            }))
            .into_response()
        }
    };

    let mut reply = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(LlmEvent::Content(c)) => reply.push_str(&c),
            Ok(LlmEvent::Done) => break,
            Ok(_) => {}
            Err(e) => {
                return Json(json!({
                    "ok": false,
                    "error": format!("{e:#}"),
                    "request": request_json,
                }))
                .into_response()
            }
        }
    }

    Json(json!({
        "ok": true,
        "provider": provider_str,
        "model": cfg.model,
        "request": request_json,
        "reply": reply.trim(),
    }))
    .into_response()
}

async fn write_secret(s: &AppState, key: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        s.control.unset_setting(key).await
    } else {
        s.control.set_setting(key, value.trim()).await
    }
}

/// Build the GET response: API keys redacted to a `*_set` flag rather
/// than echoed. Same convention as the MCP token.
fn redact(cfg: &AgentSettings) -> Value {
    let provider = match cfg.provider {
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Bedrock => "bedrock",
    };
    let bedrock_auth_mode = match cfg.bedrock_auth_mode {
        BedrockAuthMode::DefaultChain => "default_chain",
        BedrockAuthMode::BearerToken => "bearer_token",
    };
    json!({
        "enabled": cfg.enabled,
        "provider": provider,
        "model": cfg.model,
        "openai_model": cfg.openai_model,
        "anthropic_model": cfg.anthropic_model,
        "bedrock_model": cfg.bedrock_model,
        "openai_endpoint": cfg.openai_endpoint,
        "openai_api_key_set": cfg.openai_api_key.is_some(),
        "anthropic_endpoint": cfg.anthropic_endpoint,
        "anthropic_api_key_set": cfg.anthropic_api_key.is_some(),
        "bedrock_region": cfg.bedrock_region,
        "bedrock_auth_mode": bedrock_auth_mode,
        "bedrock_access_key_id_set": cfg.bedrock_access_key_id.is_some(),
        "bedrock_secret_access_key_set": cfg.bedrock_secret_access_key.is_some(),
        "bedrock_session_token_set": cfg.bedrock_session_token.is_some(),
        "bedrock_bearer_token_set": cfg.bedrock_bearer_token.is_some(),
    })
}

// ---- response helpers ---------------------------------------------------

fn internal_error(e: anyhow::Error) -> Response {
    tracing::error!("agent http: {e:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
}
