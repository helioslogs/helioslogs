// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! OpenAI-compatible streaming chat completions — any `/chat/completions` SSE server
//! (OpenAI, llama.cpp, vLLM, etc.). SSE parsing is inline, no `eventsource-stream` dep.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{LlmEvent, LlmMessage, LlmProvider, LlmRole, LlmToolDef};

pub struct OpenAiProvider {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiProvider {
    pub fn new(endpoint: String, api_key: Option<String>, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key,
            model,
        }
    }
}

/// Wire-format chat message. Mirrors the OpenAI schema; produced by
/// `LlmMessage -> WireMessage` conversion in `to_wire`.
#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    ty: &'static str,
    function: WireToolCallFn,
}

#[derive(Serialize)]
struct WireToolCallFn {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct WireToolDef {
    #[serde(rename = "type")]
    ty: &'static str,
    function: WireToolDefFn,
}

#[derive(Serialize)]
struct WireToolDefFn {
    name: String,
    description: String,
    parameters: Value,
}

fn to_wire(msgs: &[LlmMessage]) -> Vec<WireMessage> {
    msgs.iter()
        .map(|m| WireMessage {
            role: match m.role {
                LlmRole::System => "system",
                LlmRole::User => "user",
                LlmRole::Assistant => "assistant",
                LlmRole::Tool => "tool",
            },
            content: m.content.clone(),
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| WireToolCall {
                    id: tc.id.clone(),
                    ty: "function",
                    function: WireToolCallFn {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect(),
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
        })
        .collect()
}

fn tools_to_wire(tools: &[LlmToolDef]) -> Vec<WireToolDef> {
    tools
        .iter()
        .map(|t| WireToolDef {
            ty: "function",
            function: WireToolDefFn {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        })
        .collect()
}

// ---- streaming deltas (incoming) ----------------------------------------

#[derive(Deserialize)]
struct ChunkRoot {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    /// Reasoning trace; vendors split between `reasoning_content` and `reasoning`.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChunkToolCall>,
}

#[derive(Deserialize)]
struct ChunkToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChunkToolCallFn>,
}

#[derive(Deserialize)]
struct ChunkToolCallFn {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn stream(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<LlmToolDef>,
        temperature: f32,
    ) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let url = format!("{}/chat/completions", self.endpoint.trim_end_matches('/'));
        let body = json!({
            "model": self.model,
            "messages": to_wire(&messages),
            "tools": if tools.is_empty() { Value::Null } else { Value::Array(tools_to_wire(&tools).into_iter().map(|t| serde_json::to_value(t).unwrap()).collect()) },
            "temperature": temperature,
            "stream": true,
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("LLM {}: {}", status, body));
        }

        // Drive the SSE protocol off the chunked byte stream.
        let byte_stream = resp.bytes_stream();
        let event_stream = parse_sse(byte_stream);
        Ok(event_stream)
    }
}

/// Parse the OpenAI SSE wire format inline into a stream of LlmEvents;
/// network / parse errors surface as `Err` items.
fn parse_sse<S>(mut byte_stream: S) -> BoxStream<'static, Result<LlmEvent>>
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + Unpin + 'static,
{
    let stream = async_stream::stream! {
        let mut buf = String::new();
        let mut done = false;
        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => { yield Err(anyhow!("stream read: {e}")); return; }
            };
            // SSE is text; lossy utf-8 is fine — compat servers emit clean utf-8.
            buf.push_str(&String::from_utf8_lossy(&chunk));

            // Drain complete events (`\n\n` separated).
            while let Some(nl) = buf.find("\n\n") {
                let event = buf[..nl].to_string();
                buf.drain(..nl + 2);
                for line in event.lines() {
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data == "[DONE]" {
                        yield Ok(LlmEvent::Done);
                        done = true;
                        break;
                    }
                    if data.is_empty() { continue; }
                    let parsed: ChunkRoot = match serde_json::from_str(data) {
                        Ok(p) => p,
                        Err(_) => continue, // ignore malformed
                    };
                    for ch in parsed.choices {
                        if let Some(text) = ch.delta.content {
                            if !text.is_empty() {
                                yield Ok(LlmEvent::Content(text));
                            }
                        }
                        let reasoning = ch.delta.reasoning_content.or(ch.delta.reasoning);
                        if let Some(text) = reasoning {
                            if !text.is_empty() {
                                yield Ok(LlmEvent::Reasoning(text));
                            }
                        }
                        for tc in ch.delta.tool_calls {
                            let (name, args) = match tc.function {
                                Some(f) => (f.name, f.arguments),
                                None => (None, None),
                            };
                            yield Ok(LlmEvent::ToolCallDelta {
                                index: tc.index,
                                id: tc.id,
                                name,
                                arguments_delta: args,
                            });
                        }
                        if ch.finish_reason.is_some() {
                            yield Ok(LlmEvent::Done);
                            done = true;
                        }
                    }
                }
                if done { return; }
            }
        }
        if !done {
            yield Ok(LlmEvent::Done);
        }
    };
    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmToolCall;
    use futures::StreamExt;

    fn msg(role: LlmRole, content: Option<&str>) -> LlmMessage {
        LlmMessage {
            role,
            content: content.map(str::to_string),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    #[test]
    fn to_wire_maps_roles_and_tool_calls() {
        let mut assistant = msg(LlmRole::Assistant, None);
        assistant.tool_calls = vec![LlmToolCall {
            id: "call_1".into(),
            name: "query_logs".into(),
            arguments: r#"{"q":"*"}"#.into(),
        }];
        let mut tool = msg(LlmRole::Tool, Some("result-json"));
        tool.tool_call_id = Some("call_1".into());
        tool.name = Some("query_logs".into());

        let wire = serde_json::to_value(to_wire(&[
            msg(LlmRole::System, Some("sys")),
            msg(LlmRole::User, Some("hi")),
            assistant,
            tool,
        ]))
        .unwrap();

        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[1]["role"], "user");
        assert_eq!(wire[2]["role"], "assistant");
        assert_eq!(wire[2]["tool_calls"][0]["type"], "function");
        assert_eq!(wire[2]["tool_calls"][0]["function"]["name"], "query_logs");
        assert_eq!(wire[3]["role"], "tool");
        assert_eq!(wire[3]["tool_call_id"], "call_1");
        // No content on the tool-call assistant turn -> omitted.
        assert!(wire[2].get("content").is_none() || wire[2]["content"].is_null());
    }

    #[test]
    fn tools_to_wire_nests_under_function() {
        let defs = vec![LlmToolDef {
            name: "query_logs".into(),
            description: "search".into(),
            parameters: json!({"type": "object"}),
        }];
        let wire = serde_json::to_value(tools_to_wire(&defs)).unwrap();
        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["function"]["name"], "query_logs");
        assert_eq!(wire[0]["function"]["description"], "search");
        assert_eq!(wire[0]["function"]["parameters"]["type"], "object");
    }

    /// Build a mock chunked byte stream from string slices.
    fn byte_stream(chunks: &[&str]) -> impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> {
        let owned: Vec<reqwest::Result<bytes::Bytes>> = chunks
            .iter()
            .map(|c| Ok(bytes::Bytes::from(c.to_string())))
            .collect();
        futures::stream::iter(owned)
    }

    async fn collect(chunks: &[&str]) -> Vec<String> {
        let s = parse_sse(Box::pin(byte_stream(chunks)));
        s.collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|e| match e.unwrap() {
                LlmEvent::Content(t) => format!("content:{t}"),
                LlmEvent::Reasoning(t) => format!("reasoning:{t}"),
                LlmEvent::ToolCallDelta {
                    index,
                    name,
                    arguments_delta,
                    ..
                } => format!(
                    "tool[{index}]:{}:{}",
                    name.unwrap_or_default(),
                    arguments_delta.unwrap_or_default()
                ),
                LlmEvent::Done => "done".into(),
            })
            .collect()
    }

    #[tokio::test]
    async fn parse_sse_content_and_done() {
        let events = collect(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n",
        ])
        .await;
        assert_eq!(events, vec!["content:Hel", "content:lo", "done"]);
    }

    #[tokio::test]
    async fn parse_sse_tool_call_delta() {
        let events = collect(&[
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"query_logs\",\"arguments\":\"{\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
        ])
        .await;
        assert_eq!(events, vec!["tool[0]:query_logs:{", "done"]);
    }

    #[tokio::test]
    async fn parse_sse_ignores_malformed_and_finishes() {
        // Malformed JSON is skipped; a finish_reason ends the stream.
        let events = collect(&[
            "data: not-json\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\n\n",
        ])
        .await;
        assert_eq!(events, vec!["content:x", "done"]);
    }
}
