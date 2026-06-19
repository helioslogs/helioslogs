// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Agent conversation history helpers: turn stored turns into LLM wire messages
//! and derive a title. Persistence lives on [`crate::control::Control`].

use serde_json::Value;

use crate::control::backend::ConvTurn;

/// Auto-title from a user message: first non-empty line, trimmed, truncated to
/// ~60 chars. Empty input → "Untitled".
pub fn derive_title(first_user_message: &str) -> String {
    let line = first_user_message
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("Untitled");
    if line.chars().count() <= 60 {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(57).collect();
        format!("{truncated}…")
    }
}

/// Build LLM wire history from stored turns: last `history_turns` exchanges,
/// tool payloads only on the last `tool_history_turns`, dropping calls for tools outside `allowed_tools`.
pub fn turns_to_llm_messages(
    turns: &[ConvTurn],
    history_turns: usize,
    tool_history_turns: usize,
    allowed_tools: &std::collections::HashSet<String>,
) -> Vec<crate::llm::LlmMessage> {
    use crate::llm::{LlmMessage, LlmRole, LlmToolCall};

    // Window by user-turn count so the slice starts cleanly on a user message.
    let user_idx: Vec<usize> = turns
        .iter()
        .enumerate()
        .filter(|(_, t)| t.role == "user")
        .map(|(i, _)| i)
        .collect();
    let start = if user_idx.len() > history_turns {
        user_idx[user_idx.len() - history_turns]
    } else {
        0
    };
    let windowed = &turns[start..];

    // Which assistant turns keep their tool-call payloads.
    let asst_positions: Vec<usize> = windowed
        .iter()
        .enumerate()
        .filter(|(_, t)| t.role == "assistant")
        .map(|(i, _)| i)
        .collect();
    let keep_tools_from = asst_positions.len().saturating_sub(tool_history_turns);
    let keep_tools: std::collections::HashSet<usize> = asst_positions
        .iter()
        .skip(keep_tools_from)
        .copied()
        .collect();

    let mut out = Vec::with_capacity(windowed.len());
    for (i, t) in windowed.iter().enumerate() {
        match t.role.as_str() {
            "user" => {
                let content = t
                    .payload
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let view_context = t.payload.get("view_context");
                let wire_text = if let Some(vc) = view_context {
                    format_user_with_context(&content, vc)
                } else {
                    content
                };
                out.push(LlmMessage {
                    role: LlmRole::User,
                    content: Some(wire_text),
                    tool_calls: vec![],
                    tool_call_id: None,
                    name: None,
                });
            }
            "assistant" => {
                let content = t
                    .payload
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let tool_calls = t.payload.get("tool_calls").and_then(Value::as_array);

                if let Some(tcs) = tool_calls {
                    if !tcs.is_empty() && keep_tools.contains(&i) {
                        // Keep only well-formed calls whose tool still exists,
                        // paired with their result so tool_call_ids stay aligned.
                        let kept: Vec<(LlmToolCall, String)> = tcs
                            .iter()
                            .filter_map(|v| {
                                let id = v.get("id")?.as_str()?.to_string();
                                let name = v.get("name")?.as_str()?.to_string();
                                if !allowed_tools.contains(&name) {
                                    return None;
                                }
                                let args = v.get("arguments").cloned().unwrap_or(Value::Null);
                                let status =
                                    v.get("status").and_then(Value::as_str).unwrap_or("ok");
                                let body = if status == "ok" {
                                    v.get("result").cloned().unwrap_or(Value::Null)
                                } else {
                                    serde_json::json!({
                                        "error": v.get("error").cloned().unwrap_or(Value::Null)
                                    })
                                };
                                Some((
                                    LlmToolCall {
                                        id,
                                        name,
                                        arguments: serde_json::to_string(&args).unwrap_or_default(),
                                    },
                                    serde_json::to_string(&body).unwrap_or_default(),
                                ))
                            })
                            .collect();
                        if !kept.is_empty() {
                            out.push(LlmMessage {
                                role: LlmRole::Assistant,
                                content: None,
                                tool_calls: kept.iter().map(|(c, _)| c.clone()).collect(),
                                tool_call_id: None,
                                name: None,
                            });
                            for (c, body) in &kept {
                                out.push(LlmMessage {
                                    role: LlmRole::Tool,
                                    content: Some(body.clone()),
                                    tool_calls: vec![],
                                    tool_call_id: Some(c.id.clone()),
                                    name: Some(c.name.clone()),
                                });
                            }
                        }
                    }
                }

                if let Some(text) = content {
                    if !text.is_empty() {
                        out.push(LlmMessage {
                            role: LlmRole::Assistant,
                            content: Some(text),
                            tool_calls: vec![],
                            tool_call_id: None,
                            name: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Build the "[Current view context]\n...\n\n[User's question]\n..." block the
/// system prompt expects.
fn format_user_with_context(question: &str, ctx: &Value) -> String {
    let route = ctx.get("route").and_then(Value::as_str).unwrap_or("");
    let mut header = format!("Route: {route}\n");
    if let Some(q) = ctx.get("query").and_then(Value::as_str) {
        if !q.is_empty() {
            header.push_str(&format!("Query: {q}\n"));
        }
    }
    if let Some(r) = ctx.get("time_range").and_then(Value::as_str) {
        if !r.is_empty() {
            header.push_str(&format!("Time range: {r}\n"));
        }
    } else if let Some(r) = ctx.get("timeRange").and_then(Value::as_str) {
        if !r.is_empty() {
            header.push_str(&format!("Time range: {r}\n"));
        }
    }
    if let Some(idx) = ctx.get("index").and_then(Value::as_str) {
        if !idx.is_empty() {
            header.push_str(&format!("Index: {idx}\n"));
        }
    }
    format!("[Current view context]\n{header}\n[User's question]\n{question}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::backend::ConvTurn;
    use crate::llm::LlmRole;
    use serde_json::json;
    use std::collections::HashSet;

    fn turn(idx: i64, role: &str, payload: Value) -> ConvTurn {
        ConvTurn {
            id: format!("t{idx}"),
            turn_idx: idx,
            role: role.to_string(),
            payload,
            created_at: idx,
        }
    }

    // A monitor turn replayed into chat must drop `raise_alert` (absent from
    // the chat catalog) and its result, keeping the rest paired correctly.
    #[test]
    fn drops_calls_for_tools_outside_catalog() {
        let turns = vec![
            turn(0, "user", json!({ "content": "Run this monitor." })),
            turn(
                1,
                "assistant",
                json!({
                    "content": "Analysis done.",
                    "tool_calls": [
                        { "id": "a", "name": "query_logs", "arguments": { "q": "*" }, "status": "ok", "result": { "hits": 1 } },
                        { "id": "b", "name": "raise_alert", "arguments": {}, "status": "ok", "result": { "ok": true } },
                    ],
                }),
            ),
            turn(2, "user", json!({ "content": "what's going on?" })),
        ];
        let allowed: HashSet<String> = ["query_logs".to_string()].into_iter().collect();
        let msgs = turns_to_llm_messages(&turns, 30, 5, &allowed);

        let asst = msgs
            .iter()
            .find(|m| m.role == LlmRole::Assistant && !m.tool_calls.is_empty())
            .expect("assistant tool-call message present");
        assert_eq!(asst.tool_calls.len(), 1);
        assert_eq!(asst.tool_calls[0].name, "query_logs");

        let tool_msgs: Vec<_> = msgs.iter().filter(|m| m.role == LlmRole::Tool).collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("a"));
        assert!(!msgs
            .iter()
            .any(|m| m.name.as_deref() == Some("raise_alert")));
    }
}
