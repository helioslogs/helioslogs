// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Anthropic Messages API provider (`POST /v1/messages`, streaming). Reshapes the
//! flat OpenAI-style message list into Anthropic's content-block format and emits the
//! same normalized [`LlmEvent`] stream (mapping block-index → tool-index ourselves).

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use super::{LlmEvent, LlmMessage, LlmProvider, LlmRole, LlmToolDef};

pub struct AnthropicProvider {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    /// Mandatory on the Anthropic API; defaulted high for long investigations.
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key,
            model,
            max_tokens: 4096,
        }
    }
}

// ---- request shape ------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    temperature: f32,
    stream: bool,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str, // "user" | "assistant"
    content: Value,     // string OR array of content blocks
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

/// Convert our flat message list into Anthropic's shape: hoist the system
/// prompt, fold tool calls/results into assistant/user content blocks.
fn build_request_body(
    msgs: &[LlmMessage],
    tools: &[LlmToolDef],
    model: &str,
    max_tokens: u32,
    temperature: f32,
) -> AnthropicRequest {
    let mut system_parts: Vec<String> = Vec::new();
    let mut out: Vec<AnthropicMessage> = Vec::new();

    for m in msgs {
        match m.role {
            LlmRole::System => {
                if let Some(c) = &m.content {
                    system_parts.push(c.clone());
                }
            }
            LlmRole::User => {
                out.push(AnthropicMessage {
                    role: "user",
                    content: Value::String(m.content.clone().unwrap_or_default()),
                });
            }
            LlmRole::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = &m.content {
                    if !text.is_empty() {
                        blocks.push(json!({ "type": "text", "text": text }));
                    }
                }
                for tc in &m.tool_calls {
                    let input: Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or_else(|_| Value::Object(Map::new()));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": input,
                    }));
                }
                if blocks.is_empty() {
                    // Anthropic rejects empty assistant content; substitute a
                    // single empty text block (shouldn't happen in practice).
                    blocks.push(json!({ "type": "text", "text": "" }));
                }
                out.push(AnthropicMessage {
                    role: "assistant",
                    content: Value::Array(blocks),
                });
            }
            LlmRole::Tool => {
                // Merge into the previous user message if it was a
                // tool-results batch, else start a new one.
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content.clone().unwrap_or_default(),
                });
                if let Some(last) = out.last_mut() {
                    if last.role == "user" {
                        if let Value::Array(arr) = &mut last.content {
                            arr.push(block);
                            continue;
                        }
                        // Previous user message was a plain string — convert to
                        // an array to append (unreachable: results follow assistant turns).
                        let prev = std::mem::replace(&mut last.content, Value::Null);
                        let mut arr = vec![json!({ "type": "text", "text": prev })];
                        arr.push(block);
                        last.content = Value::Array(arr);
                        continue;
                    }
                }
                out.push(AnthropicMessage {
                    role: "user",
                    content: Value::Array(vec![block]),
                });
            }
        }
    }

    let tools_out: Vec<AnthropicTool> = tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.parameters.clone(),
        })
        .collect();

    AnthropicRequest {
        model: model.to_string(),
        max_tokens,
        system: if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        },
        messages: out,
        tools: tools_out,
        temperature,
        stream: true,
    }
}

// ---- streaming event shapes ---------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart {},
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: BlockDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {},
    #[serde(rename = "message_delta")]
    MessageDelta {},
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Error { error: AnthropicError },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicError {
    #[serde(default)]
    #[allow(dead_code)]
    r#type: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text {},
    #[serde(rename = "thinking")]
    Thinking {},
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        #[allow(dead_code)]
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum BlockDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(other)]
    Other,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<LlmToolDef>,
        temperature: f32,
    ) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let url = format!("{}/messages", self.endpoint.trim_end_matches('/'));
        let body = build_request_body(&messages, &tools, &self.model, self.max_tokens, temperature);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic {}: {}", status, body));
        }
        if resp.headers().get("content-type").map_or(false, |v| {
            v.to_str().unwrap_or("").starts_with("application/json")
        }) {
            // Server returned a non-streaming response (usually an
            // error envelope despite a 200). Surface its body.
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic returned non-stream JSON: {body}"));
        }
        let byte_stream = resp.bytes_stream();
        Ok(parse_sse(byte_stream))
    }
}

/// Decode Anthropic's SSE stream into our normalized events; a side map
/// turns intermixed block indices into our flat tool-call ordinals.
fn parse_sse<S>(mut byte_stream: S) -> BoxStream<'static, Result<LlmEvent>>
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + Unpin + 'static,
{
    let stream = async_stream::stream! {
        let mut buf = String::new();
        // Anthropic block-index → our tool-call ordinal. Only populated
        // for `tool_use` blocks; text / thinking blocks aren't in here.
        let mut block_to_tool: HashMap<usize, usize> = HashMap::new();
        let mut next_tool_index: usize = 0;
        let mut done = false;

        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => { yield Err(anyhow!("stream read: {e}")); return; }
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(nl) = buf.find("\n\n") {
                let event = buf[..nl].to_string();
                buf.drain(..nl + 2);
                // Only `data:` lines matter; the JSON `type` field gives the kind.
                for line in event.lines() {
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data.is_empty() { continue; }

                    let parsed: SseEvent = match serde_json::from_str(data) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    match parsed {
                        SseEvent::MessageStart {} | SseEvent::MessageDelta {} => {}
                        SseEvent::ContentBlockStart { index, content_block } => {
                            match content_block {
                                ContentBlock::ToolUse { id, name, .. } => {
                                    let our_idx = next_tool_index;
                                    next_tool_index += 1;
                                    block_to_tool.insert(index, our_idx);
                                    yield Ok(LlmEvent::ToolCallDelta {
                                        index: our_idx,
                                        id: Some(id),
                                        name: Some(name),
                                        arguments_delta: None,
                                    });
                                }
                                ContentBlock::Text {} | ContentBlock::Thinking {} | ContentBlock::Other => {}
                            }
                        }
                        SseEvent::ContentBlockDelta { index, delta } => {
                            match delta {
                                BlockDelta::Text { text } => {
                                    if !text.is_empty() {
                                        yield Ok(LlmEvent::Content(text));
                                    }
                                }
                                BlockDelta::Thinking { thinking } => {
                                    if !thinking.is_empty() {
                                        yield Ok(LlmEvent::Reasoning(thinking));
                                    }
                                }
                                BlockDelta::InputJson { partial_json } => {
                                    if let Some(&tool_idx) = block_to_tool.get(&index) {
                                        yield Ok(LlmEvent::ToolCallDelta {
                                            index: tool_idx,
                                            id: None,
                                            name: None,
                                            arguments_delta: Some(partial_json),
                                        });
                                    }
                                }
                                BlockDelta::Other => {}
                            }
                        }
                        SseEvent::ContentBlockStop {} => {}
                        SseEvent::MessageStop => {
                            yield Ok(LlmEvent::Done);
                            done = true;
                        }
                        SseEvent::Error { error } => {
                            yield Err(anyhow!("Anthropic stream error: {}", error.message));
                            return;
                        }
                        SseEvent::Other => {}
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

    fn body(msgs: &[LlmMessage], tools: &[LlmToolDef]) -> Value {
        serde_json::to_value(build_request_body(msgs, tools, "claude", 4096, 0.0)).unwrap()
    }

    #[test]
    fn system_message_hoisted_to_top_level_field() {
        let v = body(
            &[
                msg(LlmRole::System, Some("you are helioslogs")),
                msg(LlmRole::User, Some("hi")),
            ],
            &[],
        );
        assert_eq!(v["system"], "you are helioslogs");
        // The system message is removed from the messages array.
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
        assert_eq!(v["messages"][0]["role"], "user");
    }

    #[test]
    fn multiple_system_parts_joined() {
        let v = body(
            &[
                msg(LlmRole::System, Some("a")),
                msg(LlmRole::System, Some("b")),
                msg(LlmRole::User, Some("hi")),
            ],
            &[],
        );
        assert_eq!(v["system"], "a\n\nb");
    }

    #[test]
    fn tool_calls_become_tool_use_blocks() {
        let mut assistant = msg(LlmRole::Assistant, Some("let me check"));
        assistant.tool_calls = vec![LlmToolCall {
            id: "tu_1".into(),
            name: "query_logs".into(),
            arguments: r#"{"q":"*"}"#.into(),
        }];
        let v = body(&[assistant], &[]);
        let blocks = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "tu_1");
        assert_eq!(blocks[1]["input"]["q"], "*"); // arguments parsed into object
    }

    #[test]
    fn consecutive_tool_results_fold_into_one_user_message() {
        let mut t1 = msg(LlmRole::Tool, Some("r1"));
        t1.tool_call_id = Some("tu_1".into());
        let mut t2 = msg(LlmRole::Tool, Some("r2"));
        t2.tool_call_id = Some("tu_2".into());
        let v = body(&[t1, t2], &[]);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1); // folded
        let blocks = msgs[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "tu_1");
        assert_eq!(blocks[1]["tool_use_id"], "tu_2");
    }

    #[test]
    fn tools_use_input_schema_key() {
        let defs = vec![LlmToolDef {
            name: "query_logs".into(),
            description: "search".into(),
            parameters: json!({"type": "object"}),
        }];
        let v = body(&[msg(LlmRole::User, Some("hi"))], &defs);
        assert_eq!(v["tools"][0]["name"], "query_logs");
        assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
    }

    fn byte_stream(chunks: &[&str]) -> impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> {
        let owned: Vec<reqwest::Result<bytes::Bytes>> = chunks
            .iter()
            .map(|c| Ok(bytes::Bytes::from(c.to_string())))
            .collect();
        futures::stream::iter(owned)
    }

    async fn collect(chunks: &[&str]) -> Vec<String> {
        parse_sse(Box::pin(byte_stream(chunks)))
            .collect::<Vec<_>>()
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
    async fn parse_sse_maps_block_index_to_tool_ordinal() {
        // A text block (index 0) then a tool_use block (index 1) -> our tool
        // ordinal is 0, and the input_json_delta on block 1 maps to it.
        let events = collect(&[
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"query_logs\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\"\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ])
        .await;
        assert_eq!(
            events,
            vec![
                "content:hi",
                "tool[0]:query_logs:",
                "tool[0]::{\"q\"",
                "done",
            ]
        );
    }

    #[tokio::test]
    async fn parse_sse_thinking_becomes_reasoning() {
        let events = collect(&[
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ])
        .await;
        assert_eq!(events, vec!["reasoning:hmm", "done"]);
    }
}
