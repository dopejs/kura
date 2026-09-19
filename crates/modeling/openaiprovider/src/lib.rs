//! kura-openai-provider — the live `kura_llm::Provider` for OpenAI-compatible
//! `/chat/completions` endpoints (OpenAI, vLLM, Ollama gateways, any
//! openai-compatible vendor), with tool calling (Stage 9.0).
//!
//! Recorded 2026-09-19 (D10): until this crate, the daemon registered only
//! the Claude/Codex CLI bridges and the echo provider. The `openai_compatible`
//! profile existed in `kura_providers` with a base URL and key, but nothing
//! implemented the dispatch trait for it, so a configured HTTP model could
//! not be reached at all.
//!
//! Tool calls: `tools` on the request are encoded as OpenAI function tools;
//! `tool_calls` in the reply (complete or streamed as fragments) come back on
//! `ProviderResponse::tool_calls` with the arguments text verbatim.

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use kura_llm::{
    Message, MessageRole, ProviderError, ProviderRequest, ProviderResponse, StreamChunk,
    StreamEmitter, ToolCall, ToolSpec, Usage,
};
use serde_json::{Value, json};

pub const PROVIDER_NAME: &str = "openai_compatible";

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleProvider {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        Self {
            base_url,
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        if self.base_url.ends_with("/chat/completions") {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        }
    }

    fn body(&self, request: &ProviderRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": request.model,
            "messages": encode_messages(&request.messages),
            "stream": stream,
        });
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(encode_tool).collect());
            body["tool_choice"] = Value::String("auto".into());
        }
        if stream {
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }

    async fn send(
        &self,
        request: &ProviderRequest,
        stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut req = self
            .http
            .post(self.endpoint())
            .json(&self.body(request, stream));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        if request.timeout_ms > 0 {
            req = req.timeout(std::time::Duration::from_millis(request.timeout_ms as u64));
        }
        let response = req.send().await.map_err(|e| {
            let retryable = e.is_timeout() || e.is_connect();
            ProviderError::provider("transport_error", e.to_string(), retryable)
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let retryable = status.as_u16() == 429 || status.is_server_error();
            let code = match status.as_u16() {
                401 | 403 => "auth_error",
                429 => "rate_limited",
                s if s >= 500 => "upstream_error",
                _ => "request_rejected",
            };
            return Err(ProviderError::provider(
                code,
                format!("{status}: {}", truncate(&text, 500)),
                retryable,
            ));
        }
        Ok(response)
    }
}

pub fn encode_tool(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": if tool.parameters.is_null() { json!({"type": "object", "properties": {}}) } else { tool.parameters.clone() },
        }
    })
}

pub fn encode_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            };
            let mut v = json!({ "role": role, "content": m.content });
            if !m.tool_calls.is_empty() {
                v["tool_calls"] = Value::Array(
                    m.tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.call_id, "type": "function",
                                "function": { "name": c.name, "arguments": c.arguments }
                            })
                        })
                        .collect(),
                );
                if m.content.is_empty() {
                    v["content"] = Value::Null;
                }
            }
            if m.role == MessageRole::Tool && !m.tool_call_id.is_empty() {
                v["tool_call_id"] = Value::String(m.tool_call_id.clone());
            }
            v
        })
        .collect()
}

fn decode_usage(v: &Value) -> Usage {
    Usage {
        input_tokens: v.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
        output_tokens: v
            .get("completion_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        total_tokens: v.get("total_tokens").and_then(Value::as_i64).unwrap_or(0),
    }
}

/// Decodes a non-streaming completion.
pub fn decode_completion(v: &Value) -> Result<ProviderResponse, ProviderError> {
    let choice = v
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| {
            ProviderError::provider("malformed_response", "no choices in completion", false)
        })?;
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let output = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(i, c)| ToolCall {
                    call_id: c
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(&format!("call_{i}"))
                        .to_string(),
                    name: c
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ProviderResponse {
        output,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        usage: v.get("usage").map(decode_usage).unwrap_or_default(),
        tool_calls,
    })
}

/// Accumulates streamed `tool_calls` fragments (OpenAI sends the id/name on
/// the first delta and argument text in pieces) into whole calls.
#[derive(Default)]
pub struct StreamAccumulator {
    calls: BTreeMap<usize, ToolCall>,
    pub finish_reason: String,
    pub usage: Usage,
}

impl StreamAccumulator {
    /// Applies one SSE `data:` payload; returns any text delta.
    pub fn apply(&mut self, payload: &Value) -> String {
        let mut text = String::new();
        if let Some(usage) = payload.get("usage").filter(|u| !u.is_null()) {
            self.usage = decode_usage(usage);
        }
        if let Some(choice) = payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = reason.to_string();
            }
            let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                text.push_str(t);
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (i, c) in calls.iter().enumerate() {
                    let index = c
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .unwrap_or(i);
                    let entry = self.calls.entry(index).or_default();
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        entry.call_id = id.to_string();
                    }
                    if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                        entry.name.push_str(name);
                    }
                    if let Some(args) = c.pointer("/function/arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(args);
                    }
                }
            }
        }
        text
    }

    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_values()
            .enumerate()
            .map(|(i, mut c)| {
                if c.call_id.is_empty() {
                    c.call_id = format!("call_{i}");
                }
                if c.arguments.is_empty() {
                    c.arguments = "{}".into();
                }
                c
            })
            .collect()
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

impl kura_llm::Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let response = self.send(&request, false).await?;
            let body: Value = response
                .json()
                .await
                .map_err(|e| ProviderError::provider("malformed_response", e.to_string(), false))?;
            decode_completion(&body)
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            use futures::StreamExt;
            let response = self.send(&request, true).await?;
            let mut acc = StreamAccumulator::default();
            let mut output = String::new();
            let mut buffer = String::new();
            let mut bytes = response.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|e| {
                    ProviderError::provider("transport_error", e.to_string(), false)
                })?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer.drain(..=pos);
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        continue;
                    }
                    let Ok(payload) = serde_json::from_str::<Value>(data) else {
                        continue;
                    };
                    let delta = acc.apply(&payload);
                    if !delta.is_empty() {
                        output.push_str(&delta);
                        emit(StreamChunk {
                            delta,
                            ..StreamChunk::default()
                        })?;
                    }
                }
            }
            let finish_reason = acc.finish_reason.clone();
            let usage = acc.usage;
            Ok(ProviderResponse {
                output,
                finish_reason,
                usage,
                tool_calls: acc.finish(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_and_tool_messages_encode_in_the_openai_shape() {
        let tool = ToolSpec {
            name: "web.search".into(),
            description: "search".into(),
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}}}),
        };
        let encoded = encode_tool(&tool);
        assert_eq!(encoded["type"], "function");
        assert_eq!(encoded["function"]["name"], "web.search");

        let messages = vec![
            Message {
                role: MessageRole::Assistant,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    call_id: "c1".into(),
                    name: "web.search".into(),
                    arguments: "{\"query\":\"x\"}".into(),
                }],
                tool_call_id: String::new(),
            },
            Message {
                role: MessageRole::Tool,
                content: "results".into(),
                tool_calls: vec![],
                tool_call_id: "c1".into(),
            },
        ];
        let wire = encode_messages(&messages);
        assert!(
            wire[0]["content"].is_null(),
            "content-free tool request encodes as null"
        );
        assert_eq!(
            wire[0]["tool_calls"][0]["function"]["arguments"],
            "{\"query\":\"x\"}"
        );
        assert_eq!(wire[1]["tool_call_id"], "c1");
    }

    #[test]
    fn a_completion_with_tool_calls_decodes_them_verbatim() {
        let body = json!({
            "choices": [{ "finish_reason": "tool_calls", "message": { "content": null, "tool_calls": [
                { "id": "call_abc", "type": "function", "function": { "name": "memory.lookup", "arguments": "{\"assetId\":\"mem_1\"}" } }
            ] } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        });
        let r = decode_completion(&body).unwrap();
        assert_eq!(r.output, "");
        assert_eq!(r.finish_reason, "tool_calls");
        assert_eq!(r.tool_calls[0].call_id, "call_abc");
        assert_eq!(r.tool_calls[0].arguments, "{\"assetId\":\"mem_1\"}");
        assert_eq!(r.usage.total_tokens, 15);
    }

    #[test]
    fn streamed_tool_call_fragments_reassemble() {
        let mut acc = StreamAccumulator::default();
        let t = acc.apply(&json!({"choices":[{"delta":{"content":"Let me "}}]}));
        assert_eq!(t, "Let me ");
        acc.apply(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"web.search","arguments":"{\"qu"}}]}}]}));
        acc.apply(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ery\":\"k\"}"}}]}}]}));
        acc.apply(&json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}));
        assert_eq!(acc.finish_reason, "tool_calls");
        assert_eq!(acc.usage.total_tokens, 3);
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web.search");
        assert_eq!(calls[0].arguments, "{\"query\":\"k\"}");
    }
}
