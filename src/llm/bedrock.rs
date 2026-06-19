// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! AWS Bedrock Converse Stream provider (same `LlmProvider` contract). Auth has two
//! modes from the admin panel: `default_chain` (standard AWS credential chain) and
//! `bearer_token`; admin-supplied [`BedrockCreds`] override the `AWS_*` env vars.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_bedrockruntime::config::Region;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta, ContentBlockStart, ConversationRole,
    ConverseStreamOutput as StreamItem, InferenceConfiguration, Message,
    ReasoningContentBlockDelta, SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema,
    ToolResultBlock, ToolResultContentBlock, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::Document;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;

use crate::agent::settings::BedrockAuthMode;

use super::{LlmEvent, LlmMessage, LlmProvider, LlmRole, LlmToolDef};

pub struct BedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    model: String,
}

/// Admin-supplied credential overrides. Each field is `Option<String>`;
/// `Some` wins over the corresponding environment variable.
#[derive(Default, Debug, Clone)]
pub struct BedrockCreds {
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub bearer_token: Option<String>,
}

impl BedrockProvider {
    /// Build a client; auth_mode only gates env validation, the SDK routes
    /// via whichever scheme matches the loaded config.
    pub async fn new(
        region: String,
        auth_mode: BedrockAuthMode,
        creds: BedrockCreds,
        model: String,
    ) -> Result<Self> {
        // Stash the bearer override in process env (the only injection point
        // the SDK exposes); concurrent setters all write the same value.
        if let Some(tok) = creds.bearer_token.as_deref() {
            if !tok.is_empty() {
                std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", tok);
            }
        }

        if matches!(auth_mode, BedrockAuthMode::BearerToken)
            && std::env::var("AWS_BEARER_TOKEN_BEDROCK")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none()
        {
            return Err(anyhow!(
                "no Bedrock bearer token configured — required for the bearer-token auth mode. \
                 Set one in /admin/agent or export AWS_BEARER_TOKEN_BEDROCK, \
                 or switch to the default credential chain."
            ));
        }

        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .http_client(crate::crypto::tls::aws_http_client())
            .region(Region::new(region));

        // Static-credentials override only when both keys are present;
        // a half-config would silently break SigV4, so fall through instead.
        if let (Some(akid), Some(secret)) = (
            creds.access_key_id.as_deref().filter(|v| !v.is_empty()),
            creds.secret_access_key.as_deref().filter(|v| !v.is_empty()),
        ) {
            let session = creds
                .session_token
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let static_creds = Credentials::new(akid, secret, session, None, "helios-admin");
            loader = loader.credentials_provider(static_creds);
        }

        let cfg = loader.load().await;
        let client = aws_sdk_bedrockruntime::Client::new(&cfg);
        Ok(Self { client, model })
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    async fn stream(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<LlmToolDef>,
        temperature: f32,
    ) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let (system_blocks, msgs) = build_messages(&messages)?;
        let tool_cfg = build_tool_config(&tools)?;
        let infer = InferenceConfiguration::builder()
            .temperature(temperature)
            .max_tokens(4096)
            .build();

        let mut req = self
            .client
            .converse_stream()
            .model_id(&self.model)
            .set_messages(Some(msgs))
            .inference_config(infer);
        if !system_blocks.is_empty() {
            req = req.set_system(Some(system_blocks));
        }
        if let Some(cfg) = tool_cfg {
            req = req.tool_config(cfg);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| anyhow!("bedrock converse_stream: {}", display_sdk_error(&e)))?;

        // The SDK's event stream is a `EventReceiver`; wrap it into
        // our normalized stream and translate each event.
        let mut event_stream = resp.stream;

        let s = async_stream::stream! {
            // Bedrock block index → our tool-call ordinal, set only for
            // `tool_use` blocks (mirrors the Anthropic provider).
            let mut block_to_tool: HashMap<i32, usize> = HashMap::new();
            let mut next_tool_index: usize = 0;
            let mut done = false;

            loop {
                let evt = match event_stream.recv().await {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(e) => {
                        yield Err(anyhow!("bedrock stream: {}", display_sdk_error(&e)));
                        return;
                    }
                };
                match evt {
                    StreamItem::MessageStart(_) => {}
                    StreamItem::ContentBlockStart(ev) => {
                        let block_idx = ev.content_block_index();
                        if let Some(start) = ev.start() {
                            if let ContentBlockStart::ToolUse(tool) = start {
                                let our_idx = next_tool_index;
                                next_tool_index += 1;
                                block_to_tool.insert(block_idx, our_idx);
                                yield Ok(LlmEvent::ToolCallDelta {
                                    index: our_idx,
                                    id: Some(tool.tool_use_id().to_string()),
                                    name: Some(tool.name().to_string()),
                                    arguments_delta: None,
                                });
                            }
                        }
                    }
                    StreamItem::ContentBlockDelta(ev) => {
                        let block_idx = ev.content_block_index();
                        if let Some(delta) = ev.delta() {
                            match delta {
                                ContentBlockDelta::Text(text) => {
                                    if !text.is_empty() {
                                        yield Ok(LlmEvent::Content(text.clone()));
                                    }
                                }
                                ContentBlockDelta::ToolUse(tu) => {
                                    if let Some(&tool_idx) = block_to_tool.get(&block_idx) {
                                        yield Ok(LlmEvent::ToolCallDelta {
                                            index: tool_idx,
                                            id: None,
                                            name: None,
                                            arguments_delta: Some(tu.input().to_string()),
                                        });
                                    }
                                }
                                ContentBlockDelta::ReasoningContent(r) => {
                                    if let ReasoningContentBlockDelta::Text(text) = r {
                                        if !text.is_empty() {
                                            yield Ok(LlmEvent::Reasoning(text.clone()));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    StreamItem::ContentBlockStop(_) => {}
                    StreamItem::MessageStop(_) => {
                        yield Ok(LlmEvent::Done);
                        done = true;
                        break;
                    }
                    StreamItem::Metadata(_) => {}
                    _ => {}
                }
            }
            if !done {
                yield Ok(LlmEvent::Done);
            }
        };
        Ok(Box::pin(s))
    }
}

/// Reshape our flat message list into Bedrock's `(system, messages)` pair;
/// same folding rules as the Anthropic provider.
fn build_messages(msgs: &[LlmMessage]) -> Result<(Vec<SystemContentBlock>, Vec<Message>)> {
    let mut system: Vec<SystemContentBlock> = Vec::new();
    let mut out: Vec<Message> = Vec::new();

    for m in msgs {
        match m.role {
            LlmRole::System => {
                if let Some(c) = &m.content {
                    if !c.is_empty() {
                        system.push(SystemContentBlock::Text(c.clone()));
                    }
                }
            }
            LlmRole::User => {
                let text = m.content.clone().unwrap_or_default();
                let blocks = vec![ContentBlock::Text(text)];
                out.push(
                    Message::builder()
                        .role(ConversationRole::User)
                        .set_content(Some(blocks))
                        .build()
                        .map_err(|e| anyhow!("bedrock user message: {e}"))?,
                );
            }
            LlmRole::Assistant => {
                let mut blocks: Vec<ContentBlock> = Vec::new();
                if let Some(text) = &m.content {
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Text(text.clone()));
                    }
                }
                for tc in &m.tool_calls {
                    let input: Value = serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);
                    let input_doc = json_to_document(&input);
                    let tool_use = ToolUseBlock::builder()
                        .tool_use_id(&tc.id)
                        .name(&tc.name)
                        .input(input_doc)
                        .build()
                        .map_err(|e| anyhow!("bedrock tool_use: {e}"))?;
                    blocks.push(ContentBlock::ToolUse(tool_use));
                }
                if blocks.is_empty() {
                    blocks.push(ContentBlock::Text(String::new()));
                }
                out.push(
                    Message::builder()
                        .role(ConversationRole::Assistant)
                        .set_content(Some(blocks))
                        .build()
                        .map_err(|e| anyhow!("bedrock assistant message: {e}"))?,
                );
            }
            LlmRole::Tool => {
                let result_text = m.content.clone().unwrap_or_default();
                let tool_use_id = m.tool_call_id.clone().unwrap_or_default();
                let result = ToolResultBlock::builder()
                    .tool_use_id(tool_use_id)
                    .content(ToolResultContentBlock::Text(result_text))
                    .build()
                    .map_err(|e| anyhow!("bedrock tool_result: {e}"))?;
                let block = ContentBlock::ToolResult(result);

                // Fold into the previous user message if it exists.
                let mut folded = false;
                if let Some(last) = out.last_mut() {
                    if matches!(last.role(), ConversationRole::User) {
                        let mut new_content: Vec<ContentBlock> = last.content().to_vec();
                        new_content.push(block.clone());
                        *last = Message::builder()
                            .role(ConversationRole::User)
                            .set_content(Some(new_content))
                            .build()
                            .map_err(|e| anyhow!("bedrock user merge: {e}"))?;
                        folded = true;
                    }
                }
                if !folded {
                    out.push(
                        Message::builder()
                            .role(ConversationRole::User)
                            .set_content(Some(vec![block]))
                            .build()
                            .map_err(|e| anyhow!("bedrock tool_result user: {e}"))?,
                    );
                }
            }
        }
    }

    Ok((system, out))
}

fn build_tool_config(tools: &[LlmToolDef]) -> Result<Option<ToolConfiguration>> {
    if tools.is_empty() {
        return Ok(None);
    }
    let mut out: Vec<Tool> = Vec::new();
    for t in tools {
        let schema_doc = json_to_document(&t.parameters);
        let spec = ToolSpecification::builder()
            .name(&t.name)
            .description(&t.description)
            .input_schema(ToolInputSchema::Json(schema_doc))
            .build()
            .map_err(|e| anyhow!("bedrock tool spec `{}`: {e}", t.name))?;
        out.push(Tool::ToolSpec(spec));
    }
    let cfg = ToolConfiguration::builder()
        .set_tools(Some(out))
        .build()
        .map_err(|e| anyhow!("bedrock tool config: {e}"))?;
    Ok(Some(cfg))
}

/// Rewrite a `serde_json::Value` tree into the structurally-identical
/// `aws_smithy_types::Document` the Bedrock API requires.
fn json_to_document(v: &Value) -> Document {
    match v {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(aws_smithy_types::Number::NegInt(i))
            } else if let Some(u) = n.as_u64() {
                Document::Number(aws_smithy_types::Number::PosInt(u))
            } else if let Some(f) = n.as_f64() {
                Document::Number(aws_smithy_types::Number::Float(f))
            } else {
                Document::Null
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(arr) => Document::Array(arr.iter().map(json_to_document).collect()),
        Value::Object(map) => Document::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect(),
        ),
    }
}

/// Format an AWS SDK error chain for display by walking the source chain,
/// since the top-level `Debug` impl is enormous and unhelpful.
fn display_sdk_error<E: std::error::Error>(e: &E) -> String {
    let mut s = format!("{e}");
    let mut src = e.source();
    while let Some(cause) = src {
        s.push_str(&format!(" — {cause}"));
        src = cause.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmToolCall;
    use aws_smithy_types::Number;
    use serde_json::json;

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
    fn json_to_document_scalar_mapping() {
        assert!(matches!(json_to_document(&Value::Null), Document::Null));
        assert!(matches!(
            json_to_document(&json!(true)),
            Document::Bool(true)
        ));
        assert!(matches!(
            json_to_document(&json!(-5)),
            Document::Number(Number::NegInt(-5))
        ));
        assert!(matches!(
            json_to_document(&json!(1.5)),
            Document::Number(Number::Float(_))
        ));
        match json_to_document(&json!("hi")) {
            Document::String(s) => assert_eq!(s, "hi"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn json_to_document_nested() {
        let doc = json_to_document(&json!({"a": [1, "x"], "b": {"c": true}}));
        match doc {
            Document::Object(map) => {
                assert!(matches!(map.get("a"), Some(Document::Array(_))));
                assert!(matches!(map.get("b"), Some(Document::Object(_))));
            }
            other => panic!("expected object, got {other:?}"),
        }
    }

    #[test]
    fn build_messages_hoists_system_and_folds_tool_results() {
        let mut assistant = msg(LlmRole::Assistant, Some("checking"));
        assistant.tool_calls = vec![LlmToolCall {
            id: "tu_1".into(),
            name: "query_logs".into(),
            arguments: r#"{"q":"*"}"#.into(),
        }];
        let mut t1 = msg(LlmRole::Tool, Some("r1"));
        t1.tool_call_id = Some("tu_1".into());
        let mut t2 = msg(LlmRole::Tool, Some("r2"));
        t2.tool_call_id = Some("tu_2".into());

        let (system, msgs) = build_messages(&[
            msg(LlmRole::System, Some("sys")),
            msg(LlmRole::User, Some("hi")),
            assistant,
            t1,
            t2,
        ])
        .unwrap();

        assert_eq!(system.len(), 1);
        // user, assistant, and a single folded user(tool-results) message.
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[2].role(), ConversationRole::User));
        assert_eq!(msgs[2].content().len(), 2); // both tool results folded in
    }

    #[test]
    fn build_messages_empty_system_when_no_system_msg() {
        let (system, msgs) = build_messages(&[msg(LlmRole::User, Some("hi"))]).unwrap();
        assert!(system.is_empty());
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn display_sdk_error_walks_source_chain() {
        use std::fmt;
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "root cause")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "outer")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        assert_eq!(display_sdk_error(&Outer(Inner)), "outer — root cause");
    }
}
