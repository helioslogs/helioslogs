// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Agentic chat loop: drives the model through tool-call iterations and emits
//! normalized [`AgentEvent`]s over SSE. `suggest_followups` ends the turn.

use std::time::Instant;

use anyhow::{anyhow, Result};
use chrono::Utc;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::catalog::Catalog;
use crate::control::Control;
use crate::llm::{LlmEvent, LlmMessage, LlmProvider, LlmRole, LlmToolCall};
use crate::schema::Fields;

pub mod prompt;
pub mod settings;
pub mod store;
pub mod tools;

use settings::{AgentSettings, Provider};
pub use tools::{ToolContext, ToolMode};

/// History-replay window sizes; match the legacy frontend values.
const HISTORY_TURNS: usize = 30;
const TOOL_HISTORY_TURNS: usize = 5;

/// Tool-call caps guarding against a degenerate model chewing CPU on a
/// stuck loop: per-iter truncates the array, per-turn aborts the loop.
const MAX_TOOL_CALLS_PER_ITER: usize = 8;
const MAX_TOOL_CALLS_PER_TURN: usize = 30;
/// Consecutive identical calls (name + args) before we treat it as stuck.
const MAX_IDENTICAL_REPEATS: usize = 3;
/// Outer agentic-loop ceiling on model-call → tool-result rounds per turn.
const MAX_AGENT_ITERATIONS: u32 = 10;
/// Low temperature: investigation work wants determinism, not creativity.
const AGENT_TEMPERATURE: f32 = 0.2;

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Emitted once when the assistant turn record is created.
    TurnStart {
        turn_idx: i64,
    },
    ContentDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    /// Args streaming. `id` / `name` arrive once (first delta for an
    /// index); `arguments_delta` keeps appending.
    ToolDelta {
        index: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments_delta: Option<String>,
    },
    /// All args complete; executor is starting.
    ToolRunning {
        index: usize,
        id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        index: usize,
        id: String,
        name: String,
        status: String, // "ok" | "error"
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        duration_ms: u64,
    },
    /// Turn settled. Frontend uses these durations for the footer.
    TurnEnd {
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_duration_ms: Option<u64>,
        content: String,
    },
    Error {
        message: String,
    },
}

/// Snapshot of a finished tool call kept on the assistant turn payload.
#[derive(Debug, Clone, Serialize)]
struct PersistedToolCall {
    id: String,
    name: String,
    arguments: Value,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    duration_ms: u64,
}

/// Construct an `LlmProvider` from settings, surfacing config gaps (missing
/// key, unavailable Bedrock client) as `Err`. Shared by the agent loop and
/// the `/admin/agent/test` connectivity check.
pub async fn build_provider(settings: &AgentSettings) -> Result<Box<dyn LlmProvider>> {
    match settings.provider {
        Provider::Openai => Ok(Box::new(crate::llm::openai::OpenAiProvider::new(
            settings.openai_endpoint.clone(),
            settings.openai_api_key.clone(),
            settings.openai_model.clone(),
        ))),
        Provider::Anthropic => {
            let key = settings.anthropic_api_key.clone().ok_or_else(|| {
                anyhow!("Anthropic provider selected but no API key is configured — set one in /admin/agent.")
            })?;
            Ok(Box::new(crate::llm::anthropic::AnthropicProvider::new(
                settings.anthropic_endpoint.clone(),
                key,
                settings.anthropic_model.clone(),
            )))
        }
        Provider::Bedrock => {
            let creds = crate::llm::bedrock::BedrockCreds {
                access_key_id: settings.bedrock_access_key_id.clone(),
                secret_access_key: settings.bedrock_secret_access_key.clone(),
                session_token: settings.bedrock_session_token.clone(),
                bearer_token: settings.bedrock_bearer_token.clone(),
            };
            let p = crate::llm::bedrock::BedrockProvider::new(
                settings.bedrock_region.clone(),
                settings.bedrock_auth_mode,
                creds,
                settings.bedrock_model.clone(),
            )
            .await
            .map_err(|e| anyhow!("Bedrock provider unavailable: {e:#}"))?;
            Ok(Box::new(p))
        }
    }
}

pub struct AgentEngine {
    pub catalog: Catalog,
    pub fields: Fields,
    pub control: Control,
    /// Read-only demo instance: write tools are withheld from interactive chat.
    pub demo_mode: bool,
}

impl AgentEngine {
    /// Run one user → assistant exchange: persist both turns, stream events
    /// on `tx`, finalize the assistant turn when the loop settles or aborts.
    pub async fn run_turn(
        &self,
        conv_id: String,
        user_id: &str,
        env: &str,
        user_message: &str,
        view_context: Option<Value>,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        // Pull TZ + local-now from view_context so the prompt can resolve
        // wall-clock references ("yesterday", "9am"); degrades to UTC if absent.
        let (tz, now_local) = match &view_context {
            Some(vc) => (
                vc.get("timezone").and_then(Value::as_str),
                vc.get("nowLocal")
                    .or_else(|| vc.get("now_local"))
                    .and_then(Value::as_str),
            ),
            None => (None, None),
        };
        let system_prompt = prompt::build_with_env(tz, now_local, env);

        self.run_turn_with_mode(
            ToolMode::InteractiveChat {
                user_id: user_id.to_string(),
            },
            system_prompt,
            conv_id,
            user_id,
            env,
            user_message,
            view_context,
            tx,
        )
        .await
    }

    /// Full agent loop with explicit mode + system prompt; `run_turn` wraps
    /// this for chat, the scheduler calls it directly for monitor runs.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_with_mode(
        &self,
        mode: ToolMode,
        system_prompt: String,
        conv_id: String,
        user_id: &str,
        env: &str,
        user_message: &str,
        view_context: Option<Value>,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        let settings = AgentSettings::load(&self.control).await?;
        if !settings.enabled {
            return abort_with_error(
                &self.control,
                user_id,
                &conv_id,
                &tx,
                "AI agent functionality is disabled by an administrator.",
            )
            .await;
        }
        let ctx = ToolContext {
            catalog: self.catalog.clone(),
            fields: self.fields,
            control: self.control.clone(),
            mode: mode.clone(),
            env: env.to_string(),
            demo_mode: self.demo_mode,
        };

        // On a demo instance, withhold write tools so the model never offers to
        // mutate anything; reads (query, discover, list_*) stay available.
        let tool_defs = {
            let mut defs = tools::tool_defs(&mode);
            if self.demo_mode {
                defs.retain(|d| !tools::is_demo_write_tool(&d.name));
            }
            defs
        };

        // Persist the user turn first so a refresh during streaming
        // still shows the question.
        let user_payload = match &view_context {
            Some(vc) => json!({ "content": user_message, "view_context": vc }),
            None => json!({ "content": user_message }),
        };
        self.control
            .conv_append_turn(user_id, &conv_id, "user", &user_payload)
            .await?;

        // If the conversation just got its first user message and is
        // still untitled, set a derived title now.
        let detail = self
            .control
            .conv_get(user_id, &conv_id)
            .await?
            .ok_or_else(|| anyhow!("conversation gone after append"))?;
        if detail.meta.title.is_empty() {
            let _ = self
                .control
                .conv_rename(user_id, &conv_id, &store::derive_title(user_message))
                .await;
        }

        // Build wire history from stored turns (the user turn we just
        // added is included).
        let mut wire = vec![LlmMessage {
            role: LlmRole::System,
            content: Some(system_prompt.clone()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }];
        let allowed_tools: std::collections::HashSet<String> =
            tool_defs.iter().map(|d| d.name.clone()).collect();
        wire.extend(store::turns_to_llm_messages(
            &detail.turns,
            HISTORY_TURNS,
            TOOL_HISTORY_TURNS,
            &allowed_tools,
        ));

        // Provider construction.
        let provider: Box<dyn LlmProvider> = match build_provider(&settings).await {
            Ok(p) => p,
            Err(e) => {
                return abort_with_error(&self.control, user_id, &conv_id, &tx, &format!("{e:#}"))
                    .await;
            }
        };

        // Turns are append-only: keep the in-flight payload in memory and
        // persist a separate row once it settles (or aborts).
        let asst_turn_idx = detail.turns.len() as i64; // user turn is already in `detail`
        let _ = tx
            .send(AgentEvent::TurnStart {
                turn_idx: asst_turn_idx,
            })
            .await;

        let turn_started = Instant::now();
        let mut turn_content = String::new();
        let mut turn_reasoning = String::new();
        let mut first_reasoning_at: Option<Instant> = None;
        let mut last_reasoning_at: Option<Instant> = None;
        let mut persisted_calls: Vec<PersistedToolCall> = Vec::new();

        // Outer loop = iterations of the agentic dance (model → tool calls → model → ...).
        for _ in 0..MAX_AGENT_ITERATIONS {
            let mut stream = match provider
                .stream(wire.clone(), tool_defs.clone(), AGENT_TEMPERATURE)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx
                        .send(AgentEvent::Error {
                            message: format!("{e:#}"),
                        })
                        .await;
                    finalize(
                        &self.control,
                        user_id,
                        &conv_id,
                        turn_started,
                        &turn_content,
                        &persisted_calls,
                        &turn_reasoning,
                        compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                        &tx,
                    )
                    .await?;
                    return Ok(());
                }
            };

            // Per-iter accumulators.
            let mut iter_content = String::new();
            let mut iter_calls: Vec<AccumulatedCall> = Vec::new();
            // Base offset to map per-iter tool indices onto a single
            // turn-global index across all iterations.
            let iter_base = persisted_calls.len();

            while let Some(evt) = stream.next().await {
                let evt = match evt {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx
                            .send(AgentEvent::Error {
                                message: format!("stream: {e:#}"),
                            })
                            .await;
                        finalize(
                            &self.control,
                            user_id,
                            &conv_id,
                            turn_started,
                            &turn_content,
                            &persisted_calls,
                            &turn_reasoning,
                            compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                            &tx,
                        )
                        .await?;
                        return Ok(());
                    }
                };
                match evt {
                    LlmEvent::Content(text) => {
                        iter_content.push_str(&text);
                        if tx
                            .send(AgentEvent::ContentDelta { delta: text })
                            .await
                            .is_err()
                        {
                            return Ok(()); // client dropped — abort silently
                        }
                    }
                    LlmEvent::Reasoning(text) => {
                        let now = Instant::now();
                        if first_reasoning_at.is_none() {
                            first_reasoning_at = Some(now);
                        }
                        last_reasoning_at = Some(now);
                        turn_reasoning.push_str(&text);
                        if tx
                            .send(AgentEvent::ReasoningDelta { delta: text })
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    LlmEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    } => {
                        // Drop deltas beyond the per-iter cap rather than
                        // growing the tool list unbounded.
                        if index >= MAX_TOOL_CALLS_PER_ITER {
                            continue;
                        }
                        while iter_calls.len() <= index {
                            iter_calls.push(AccumulatedCall::default());
                        }
                        let c = &mut iter_calls[index];
                        if let Some(i) = &id {
                            c.id = i.clone();
                        }
                        if let Some(n) = &name {
                            c.name = n.clone();
                        }
                        if let Some(a) = &arguments_delta {
                            c.arguments.push_str(a);
                        }
                        if tx
                            .send(AgentEvent::ToolDelta {
                                index: iter_base + index,
                                id,
                                name,
                                arguments_delta,
                            })
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    LlmEvent::Done => {
                        break;
                    }
                }
            }

            // Carry content forward across iters so previous commentary
            // stays visible above newly-streaming text.
            if !iter_content.is_empty() {
                if !turn_content.is_empty() {
                    turn_content.push_str("\n\n");
                }
                turn_content.push_str(&iter_content);
            }

            // No tool calls → natural completion.
            if iter_calls.is_empty() {
                finalize(
                    &self.control,
                    user_id,
                    &conv_id,
                    turn_started,
                    &turn_content,
                    &persisted_calls,
                    &turn_reasoning,
                    compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                    &tx,
                )
                .await?;
                return Ok(());
            }

            // Pre-checks before executing (so no redundant queries fire): abort
            // on a stuck identical-call run or a per-turn cap overflow.
            if let Some((stuck_name, run_len)) = find_stuck_run(&persisted_calls, &iter_calls) {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: format!(
                            "stopped: model called `{stuck_name}` {run_len} times in a row with identical args (stuck loop)"
                        ),
                    })
                    .await;
                finalize(
                    &self.control,
                    user_id,
                    &conv_id,
                    turn_started,
                    &turn_content,
                    &persisted_calls,
                    &turn_reasoning,
                    compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                    &tx,
                )
                .await?;
                return Ok(());
            }
            if persisted_calls.len() + iter_calls.len() > MAX_TOOL_CALLS_PER_TURN {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: format!(
                            "stopped: would exceed {} tool calls in one turn (model isn't converging)",
                            MAX_TOOL_CALLS_PER_TURN
                        ),
                    })
                    .await;
                finalize(
                    &self.control,
                    user_id,
                    &conv_id,
                    turn_started,
                    &turn_content,
                    &persisted_calls,
                    &turn_reasoning,
                    compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                    &tx,
                )
                .await?;
                return Ok(());
            }

            // Execute sequentially — small models reason better when each
            // result lands deterministically.
            let assistant_tool_calls: Vec<LlmToolCall> = iter_calls
                .iter()
                .map(|c| LlmToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect();

            // Add the assistant tool-call message to wire for the next iter.
            wire.push(LlmMessage {
                role: LlmRole::Assistant,
                content: if iter_content.is_empty() {
                    None
                } else {
                    Some(iter_content.clone())
                },
                tool_calls: assistant_tool_calls.clone(),
                tool_call_id: None,
                name: None,
            });

            for c in &iter_calls {
                let args_json: Value = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                let global_idx = persisted_calls.len();

                let _ = tx
                    .send(AgentEvent::ToolRunning {
                        index: global_idx,
                        id: c.id.clone(),
                        name: c.name.clone(),
                        arguments: args_json.clone(),
                    })
                    .await;

                let started = Instant::now();
                let exec = tools::execute(&ctx, &c.name, &args_json).await;
                let dur_ms = started.elapsed().as_millis() as u64;

                let (status, result, error) = match exec {
                    Ok(v) => ("ok".to_string(), Some(v), None),
                    Err(e) => ("error".to_string(), None, Some(format!("{e:#}"))),
                };

                let _ = tx
                    .send(AgentEvent::ToolResult {
                        index: global_idx,
                        id: c.id.clone(),
                        name: c.name.clone(),
                        status: status.clone(),
                        result: result.clone(),
                        error: error.clone(),
                        duration_ms: dur_ms,
                    })
                    .await;

                // Feed the result back to the model for the next iter.
                let tool_body = if status == "ok" {
                    serde_json::to_string(&result).unwrap_or_else(|_| "null".into())
                } else {
                    serde_json::to_string(&json!({ "error": error })).unwrap_or_default()
                };
                wire.push(LlmMessage {
                    role: LlmRole::Tool,
                    content: Some(tool_body),
                    tool_calls: vec![],
                    tool_call_id: Some(c.id.clone()),
                    name: Some(c.name.clone()),
                });

                persisted_calls.push(PersistedToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: args_json,
                    status,
                    result,
                    error,
                    duration_ms: dur_ms,
                });
            }

            // `suggest_followups` is terminal — end the turn after running it.
            if iter_calls.iter().any(|c| tools::is_terminal(&c.name)) {
                finalize(
                    &self.control,
                    user_id,
                    &conv_id,
                    turn_started,
                    &turn_content,
                    &persisted_calls,
                    &turn_reasoning,
                    compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
                    &tx,
                )
                .await?;
                return Ok(());
            }

            // Otherwise loop — next iter streams the model's response to the tool results.
        }

        // Exhausted iterations without final text. Settle with what we have.
        finalize(
            &self.control,
            user_id,
            &conv_id,
            turn_started,
            &turn_content,
            &persisted_calls,
            &turn_reasoning,
            compute_reasoning_dur(first_reasoning_at, last_reasoning_at),
            &tx,
        )
        .await?;
        Ok(())
    }
}

/// Per-iteration in-flight tool call (still streaming args).
#[derive(Default, Clone)]
struct AccumulatedCall {
    id: String,
    name: String,
    arguments: String,
}

/// Detect a stuck loop: `MAX_IDENTICAL_REPEATS`+ consecutive identical calls
/// (compared on parsed JSON so cosmetic formatting differences don't slip).
fn find_stuck_run(
    persisted: &[PersistedToolCall],
    iter_calls: &[AccumulatedCall],
) -> Option<(String, usize)> {
    // Materialize the full chain as (name, parsed-args) keys.
    let chain: Vec<(String, Value)> = persisted
        .iter()
        .map(|c| (c.name.clone(), c.arguments.clone()))
        .chain(iter_calls.iter().map(|c| {
            let args = serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null);
            (c.name.clone(), args)
        }))
        .collect();

    let mut run: usize = 0;
    let mut last_key: Option<&(String, Value)> = None;
    for k in &chain {
        if last_key == Some(k) {
            run += 1;
        } else {
            run = 1;
            last_key = Some(k);
        }
        if run >= MAX_IDENTICAL_REPEATS {
            return Some((k.0.clone(), run));
        }
    }
    None
}

fn compute_reasoning_dur(first: Option<Instant>, last: Option<Instant>) -> Option<u64> {
    match (first, last) {
        (Some(a), Some(b)) => Some(b.duration_since(a).as_millis() as u64),
        _ => None,
    }
}

/// Report a setup-time failure: emit an SSE `Error` then persist a settled
/// turn so it survives a refresh. Caller should short-circuit after this.
async fn abort_with_error(
    control: &Control,
    user_id: &str,
    conv_id: &str,
    tx: &mpsc::Sender<AgentEvent>,
    message: &str,
) -> Result<()> {
    let _ = tx
        .send(AgentEvent::Error {
            message: message.to_string(),
        })
        .await;
    let payload = json!({
        "content": format!("Sorry — {message}"),
        "tool_calls": Vec::<Value>::new(),
        "duration_ms": 0,
    });
    control
        .conv_append_turn(user_id, conv_id, "assistant", &payload)
        .await?;
    let _ = tx
        .send(AgentEvent::TurnEnd {
            duration_ms: 0,
            reasoning_duration_ms: None,
            content: format!("Sorry — {message}"),
        })
        .await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn finalize(
    control: &Control,
    user_id: &str,
    conv_id: &str,
    started_at: Instant,
    content: &str,
    calls: &[PersistedToolCall],
    reasoning: &str,
    reasoning_dur_ms: Option<u64>,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<()> {
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let mut payload = json!({
        "content": content,
        "tool_calls": calls,
        "duration_ms": duration_ms,
        "reasoning_duration_ms": reasoning_dur_ms,
        "settled_at": Utc::now().timestamp_millis(),
    });
    if !reasoning.is_empty() {
        if let Value::Object(map) = &mut payload {
            map.insert("reasoning".into(), Value::String(reasoning.to_string()));
        }
    }
    control
        .conv_append_turn(user_id, conv_id, "assistant", &payload)
        .await?;
    let _ = tx
        .send(AgentEvent::TurnEnd {
            duration_ms,
            reasoning_duration_ms: reasoning_dur_ms,
            content: content.to_string(),
        })
        .await;
    Ok(())
}
