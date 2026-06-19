// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! LLM provider abstraction: one trait, three impls (OpenAI-compatible, Anthropic,
//! Bedrock). The agent loop talks only to the trait and gets normalized events back.

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod anthropic;
pub mod bedrock;
pub mod openai;

/// Normalized chat-transcript message; mirrors the OpenAI shape (lowest
/// common denominator) which the Anthropic/Bedrock impls rewrite on output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    /// Text content; `None` on a tool-only assistant turn, result JSON on tool messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Set on assistant turns that called tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
    /// On tool messages, pairs with the assistant's `tool_calls[i].id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name on tool-result messages (Bedrock surfaces it, OpenAI ignores).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A model tool-call. `arguments` stays a (possibly partial) JSON string
/// rather than a parsed object, since providers stream it that way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Tool definition advertised to the model. JSON Schema for `parameters`.
#[derive(Debug, Clone, Serialize)]
pub struct LlmToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Normalized stream events emitted by every provider. The agent loop
/// dispatches on these without knowing which provider produced them.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// A chunk of assistant text content.
    Content(String),
    /// A chunk of reasoning trace; only for models that emit it.
    Reasoning(String),
    /// Incremental tool-call delta; `index` selects the call, `id`/`name` arrive once.
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    /// Stream ended; the caller has accumulated content/calls from the deltas.
    Done,
}

/// Shared streaming-completion interface. Errors surface as `Err` items on
/// the stream (not early return) so callers can drain deltas before the failure.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<LlmToolDef>,
        temperature: f32,
    ) -> Result<BoxStream<'static, Result<LlmEvent>>>;
}
