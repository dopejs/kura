use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_stream::try_stream;
use kura_protocol::ResponseItem;
use kura_protocol::Role;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
use serde_json::json;

use crate::provider::GeneratedAsset;
use crate::provider::GenerationModality;
use crate::provider::GenerationProvider;
use crate::provider::GenerationRequest;
use crate::provider::GenerationStatus;
use crate::provider::ModelProvider;
use crate::provider::Prompt;
use crate::provider::ProviderError;
use crate::provider::ResponseEvent;

/// Streaming client for OpenAI-compatible `/chat/completions` endpoints
/// (OpenAI, local vLLM/Ollama gateways, openai-compatible providers in the
/// Go daemon's provider manager).
/// A credential that can be replaced while the provider is registered.
///
/// OAuth access tokens last about an hour, and the daemon outlives them by a
/// long way. Reading the credential at boot and holding it would mean either a
/// provider that starts failing mid-session or a daemon restart every time a
/// token rotates -- and a restart drops whatever run was in flight. The handle
/// is cloned into whatever refreshes it, so the swap costs one lock.
#[derive(Clone, Default)]
pub struct Credential(Arc<RwLock<Option<String>>>);

impl Credential {
    #[must_use]
    pub fn new(value: Option<String>) -> Self {
        Self(Arc::new(RwLock::new(value)))
    }

    /// Replace the credential used by every request from here on.
    pub fn set(&self, value: Option<String>) {
        // A poisoned lock means a panic while swapping a token, which says
        // nothing about the new value; recovering keeps a dispatch working.
        let mut guard = match self.0.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = value;
    }

    #[must_use]
    pub fn get(&self) -> Option<String> {
        match self.0.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

pub struct OpenAiCompatibleClient {
    base_url: String,
    model: String,
    api_key: Credential,
    /// Extra headers sent with every request; see [`Self::with_headers`].
    headers: BTreeMap<String, String>,
    /// Sampling parameters; `None` fields are omitted from the body.
    sampling: Sampling,
    http: reqwest::Client,
}

/// Sampling parameters forwarded to the provider. Kept as a local type so this
/// crate does not depend on the config crate.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sampling {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
}

/// Headers a caller may not override: the API key is supplied separately, so
/// letting a header map replace it would silently bypass the configured
/// credential and any redaction applied to it.
const RESERVED_HEADERS: [&str; 1] = ["authorization"];

impl OpenAiCompatibleClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: Credential::new(api_key),
            headers: BTreeMap::new(),
            sampling: Sampling::default(),
            http: reqwest::Client::new(),
        }
    }

    /// Attach gateway headers. Reserved headers are dropped rather than
    /// applied, so a misconfigured map cannot replace the credential.
    #[must_use]
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers
            .into_iter()
            .filter(|(name, _)| !RESERVED_HEADERS.contains(&name.trim().to_lowercase().as_str()))
            .collect();
        self
    }

    #[must_use]
    pub fn with_sampling(mut self, sampling: Sampling) -> Self {
        self.sampling = sampling;
        self
    }

    /// A handle to this client's credential, for whatever keeps it fresh.
    #[must_use]
    pub fn credential(&self) -> Credential {
        self.api_key.clone()
    }
}

/// Partially received tool call; `arguments` arrive as JSON fragments that
/// must be concatenated before the call is complete.
#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

impl ModelProvider for OpenAiCompatibleClient {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
        let body = build_request(&self.model, prompt, self.sampling);
        let mut request = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // Read per request, not captured at construction: a token refreshed
        // since the last dispatch has to be the one that goes out.
        let request = match self.api_key.get() {
            Some(key) => request.bearer_auth(key),
            None => request,
        };
        Box::pin(try_stream! {
            let response = request.send().await?;
            let mut bytes = checked_bytes_stream(response).await?;

            let mut buffer = String::new();
            let mut pending_calls: Vec<ToolCallAcc> = Vec::new();
            let mut done = false;
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer.drain(..=pos);
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        done = true;
                        break;
                    }
                    if data.is_empty() {
                        continue;
                    }
                    for event in accumulate_chunk(data, &mut pending_calls)? {
                        yield event;
                    }
                }
                if done {
                    break;
                }
            }

            for (index, call) in pending_calls.into_iter().enumerate() {
                yield ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.id, format!("call_{index}")),
                    name: call.name,
                    arguments: call.arguments,
                };
            }
            yield ResponseEvent::Completed;
        })
    }
}

fn non_empty_or(value: String, fallback: String) -> String {
    if value.is_empty() { fallback } else { value }
}

/// Return the response byte stream, or a `Status` error carrying the body.
pub(crate) async fn checked_bytes_stream(
    response: reqwest::Response,
) -> Result<impl futures::Stream<Item = reqwest::Result<bytes::Bytes>>, ProviderError> {
    if response.status().is_success() {
        Ok(response.bytes_stream())
    } else {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(ProviderError::Status { status, body })
    }
}

/// Parse one SSE `data:` payload into events, accumulating tool-call
/// fragments in `pending_calls`. Returns only immediately emittable events
/// (text deltas); completed calls are flushed by the caller at stream end.
fn accumulate_chunk(
    data: &str,
    pending_calls: &mut Vec<ToolCallAcc>,
) -> Result<Vec<ResponseEvent>, ProviderError> {
    let chunk: Value = serde_json::from_str(data)
        .map_err(|err| ProviderError::Malformed(format!("invalid chunk json: {err}")))?;
    let mut events = Vec::new();
    let Some(choices) = chunk["choices"].as_array() else {
        return Ok(events);
    };
    for choice in choices {
        let delta = &choice["delta"];
        if let Some(content) = delta["content"].as_str()
            && !content.is_empty()
        {
            events.push(ResponseEvent::OutputTextDelta(content.to_string()));
        }
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for (position, call) in tool_calls.iter().enumerate() {
                let index = call["index"].as_u64().unwrap_or(position as u64) as usize;
                if pending_calls.len() <= index {
                    pending_calls.resize_with(index + 1, ToolCallAcc::default);
                }
                let acc = &mut pending_calls[index];
                if let Some(id) = call["id"].as_str() {
                    acc.id.push_str(id);
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    acc.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    acc.arguments.push_str(arguments);
                }
            }
        }
    }
    Ok(events)
}

/// Build the `/chat/completions` request body, mapping conversation history
/// into OpenAI message shapes.
fn build_request(model: &str, prompt: &Prompt, sampling: Sampling) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) = &prompt.instructions {
        messages.push(json!({"role": "system", "content": instructions}));
    }
    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content } => {
                let role = match role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                messages.push(json!({"role": role, "content": content}));
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                messages.push(json!({
                    "role": "assistant",
                    "tool_calls": [{
                        "id": call_id,
                        "type": "function",
                        "function": {"name": name, "arguments": arguments},
                    }],
                }));
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output,
                }));
            }
        }
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    if !prompt.tools.is_empty() {
        let tools: Vec<Value> = prompt
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }
    // Omitted when unset: some gateways reject an explicit temperature on
    // reasoning models, so "unset" must not become an explicit default.
    if let Some(temperature) = sampling.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = sampling.top_p {
        body["top_p"] = json!(top_p);
    }
    body
}

// ---------------------------------------------------------------------------
// Image generation
// ---------------------------------------------------------------------------

/// Image generation over the OpenAI-compatible `/images/generations` endpoint.
///
/// Separate from [`OpenAiCompatibleClient`] because the two speak different
/// endpoints and response shapes; sharing one type would mean a client whose
/// `modality()` depends on which method you call.
pub struct OpenAiCompatibleImageClient {
    base_url: String,
    api_key: Option<String>,
    headers: BTreeMap<String, String>,
    http: reqwest::Client,
}

impl OpenAiCompatibleImageClient {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            headers: BTreeMap::new(),
            http: reqwest::Client::new(),
        }
    }

    /// See [`OpenAiCompatibleClient::with_headers`]; reserved headers are dropped.
    #[must_use]
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers
            .into_iter()
            .filter(|(name, _)| !RESERVED_HEADERS.contains(&name.trim().to_lowercase().as_str()))
            .collect();
        self
    }
}

/// Build the `/images/generations` body. Caller options are merged first so a
/// provider-specific knob (size, quality, seed) can be passed through, but
/// `model` and `prompt` always win: they are the request's identity.
fn build_image_request(request: &GenerationRequest) -> Value {
    let mut body = match &request.options {
        Value::Object(map) => Value::Object(map.clone()),
        _ => json!({}),
    };
    body["model"] = json!(request.model);
    body["prompt"] = json!(request.prompt);
    body
}

/// Map one `data` entry to an asset. Providers return either inline base64 or
/// a URL, and which one arrived matters to the caller, so it is preserved
/// rather than normalized.
fn parse_image_entry(entry: &Value) -> Option<GeneratedAsset> {
    if let Some(b64) = entry.get("b64_json").and_then(Value::as_str) {
        return Some(GeneratedAsset::Bytes {
            media_type: "image/png".to_string(),
            data: bytes::Bytes::from(b64.to_string()),
        });
    }
    entry
        .get("url")
        .and_then(Value::as_str)
        .map(|url| GeneratedAsset::Url {
            media_type: "image/png".to_string(),
            url: url.to_string(),
        })
}

impl GenerationProvider for OpenAiCompatibleImageClient {
    fn modality(&self) -> GenerationModality {
        GenerationModality::Image
    }

    fn generate<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> BoxFuture<'a, Result<GenerationStatus, ProviderError>> {
        Box::pin(async move {
            let body = build_image_request(request);
            let mut http = self
                .http
                .post(format!("{}/images/generations", self.base_url))
                .json(&body);
            for (name, value) in &self.headers {
                http = http.header(name, value);
            }
            if let Some(key) = &self.api_key {
                http = http.bearer_auth(key);
            }
            let response = http.send().await?;
            let status = response.status();
            let payload = response.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(ProviderError::Status {
                    status: status.as_u16(),
                    body: payload,
                });
            }
            let parsed: Value = serde_json::from_str(&payload)
                .map_err(|err| ProviderError::Malformed(format!("invalid image json: {err}")))?;
            let entries = parsed
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| ProviderError::Malformed("image response has no data array".into()))?;
            let assets: Vec<GeneratedAsset> = entries.iter().filter_map(parse_image_entry).collect();
            if assets.is_empty() {
                // A success status with nothing usable is a provider bug; do
                // not report it as a completed generation.
                return Err(ProviderError::Malformed(
                    "image response contained no usable asset".into(),
                ));
            }
            Ok(GenerationStatus::Ready(assets))
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sampling_is_omitted_when_unset_and_sent_when_set() {
        let prompt = Prompt {
            instructions: None,
            input: vec![],
            tools: vec![],
        };

        let bare = build_request("m", &prompt, Sampling::default());
        assert!(bare.get("temperature").is_none());
        assert!(bare.get("top_p").is_none());

        // Zero must survive: it is a meaningful value, not "unset".
        let tuned = build_request(
            "m",
            &prompt,
            Sampling {
                temperature: Some(0.0),
                top_p: Some(0.95),
            },
        );
        assert_eq!(tuned["temperature"], json!(0.0));
        assert_eq!(tuned["top_p"], json!(0.95));
    }

    #[test]
    fn image_request_merges_options_but_identity_wins() {
        let request = GenerationRequest {
            model: "sd3-medium".into(),
            prompt: "a forge guardian".into(),
            // A caller passing `model` in options must not be able to redirect
            // the request to a different model.
            options: json!({"size": "1024x1024", "model": "smuggled"}),
        };
        let body = build_image_request(&request);
        assert_eq!(body["model"], json!("sd3-medium"));
        assert_eq!(body["prompt"], json!("a forge guardian"));
        assert_eq!(body["size"], json!("1024x1024"));
    }

    #[test]
    fn image_request_tolerates_non_object_options() {
        let request = GenerationRequest {
            model: "m".into(),
            prompt: "p".into(),
            options: json!("not-an-object"),
        };
        let body = build_image_request(&request);
        assert_eq!(body["model"], json!("m"));
    }

    #[test]
    fn image_entries_preserve_how_the_asset_arrived() {
        // Inline bytes and an expiring URL have different handling costs, so
        // the distinction is kept rather than normalized away.
        let inline = parse_image_entry(&json!({"b64_json": "AAAA"})).expect("inline asset");
        assert!(matches!(inline, GeneratedAsset::Bytes { .. }));

        let remote = parse_image_entry(&json!({"url": "https://cdn/img.png"})).expect("url asset");
        assert!(matches!(remote, GeneratedAsset::Url { .. }));

        assert!(parse_image_entry(&json!({"unexpected": 1})).is_none());
    }

    #[test]
    fn image_client_reports_its_modality() {
        let client = OpenAiCompatibleImageClient::new("http://h/v1", None);
        assert_eq!(client.modality(), GenerationModality::Image);
    }

    #[test]
    fn a_synchronous_provider_reports_unknown_jobs_rather_than_success() {
        // The default `poll` must not imply a generation finished.
        let client = OpenAiCompatibleImageClient::new("http://h/v1", None);
        let result = futures::executor::block_on(client.poll("job_1"));
        assert!(matches!(result, Err(ProviderError::Malformed(_))));
    }

    #[test]
    fn reserved_headers_cannot_replace_the_credential() {
        let client = OpenAiCompatibleClient::new("http://h/v1", "m", Some("key".into()))
            .with_headers(BTreeMap::from([
                ("X-Studio-Team".to_string(), "engine".to_string()),
                ("Authorization".to_string(), "Bearer attacker".to_string()),
                ("authorization".to_string(), "Bearer attacker".to_string()),
            ]));

        assert_eq!(client.headers.get("X-Studio-Team").map(String::as_str), Some("engine"));
        assert!(!client.headers.keys().any(|k| k.eq_ignore_ascii_case("authorization")));
    }

    use super::*;
    use crate::provider::ToolSpec;

    #[test]
    fn request_maps_history_to_openai_messages() {
        let prompt = Prompt {
            instructions: Some("be brief".into()),
            input: vec![
                ResponseItem::Message {
                    role: Role::User,
                    content: "hi".into(),
                },
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "shell".into(),
                    arguments: "{\"cmd\":\"ls\"}".into(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".into(),
                    output: "file.txt".into(),
                },
            ],
            tools: vec![ToolSpec {
                name: "shell".into(),
                description: "run a command".into(),
                parameters: json!({"type": "object"}),
            }],
        };
        let body = build_request("test-model", &prompt, Sampling::default());
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[0],
            json!({"role": "system", "content": "be brief"})
        );
        assert_eq!(messages[1], json!({"role": "user", "content": "hi"}));
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            messages[3],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "file.txt"})
        );
        assert_eq!(body["tools"][0]["function"]["name"], "shell");
    }

    #[test]
    fn chunk_without_tools_yields_text_deltas() {
        let mut pending = Vec::new();
        let events = accumulate_chunk(
            r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
            &mut pending,
        )
        .unwrap();
        assert_eq!(events, vec![ResponseEvent::OutputTextDelta("hello".into())]);
        assert!(pending.is_empty());
    }

    #[test]
    fn tool_call_fragments_accumulate_across_chunks() {
        let mut pending = Vec::new();
        accumulate_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"shell","arguments":"{\"cm"}}]}}]}"#,
            &mut pending,
        )
        .unwrap();
        let events = accumulate_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"d\":\"ls\"}"}}]}}]}"#,
            &mut pending,
        )
        .unwrap();
        assert!(events.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "call_9");
        assert_eq!(pending[0].name, "shell");
        assert_eq!(pending[0].arguments, "{\"cmd\":\"ls\"}");
    }

    #[test]
    fn usage_only_chunks_are_ignored() {
        let mut pending = Vec::new();
        let events =
            accumulate_chunk(r#"{"choices":[],"usage":{"total_tokens":3}}"#, &mut pending).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn invalid_json_is_malformed_error() {
        let mut pending = Vec::new();
        let err = accumulate_chunk("{oops", &mut pending).unwrap_err();
        assert!(matches!(err, ProviderError::Malformed(_)));
    }

    /// An endpoint that records the `Authorization` of each request it serves.
    ///
    /// Two connections, because the point of the test is what the *second*
    /// request carries after the credential was replaced between them.
    fn spawn_recording_endpoint(
        connections: usize,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&seen);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                let authorization = text
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                recorded.lock().expect("lock").push(authorization);
                let body = "data: [DONE]\n\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}"), seen)
    }

    async fn drain(client: &OpenAiCompatibleClient) {
        use futures::StreamExt;
        let prompt = Prompt::default();
        let mut stream = client.stream(&prompt);
        while stream.next().await.is_some() {}
    }

    #[tokio::test]
    async fn a_replaced_credential_is_used_by_the_next_request() {
        // An OAuth access token expires roughly hourly while this daemon runs
        // for far longer. Reading the credential once at construction would
        // mean every dispatch after the first refresh goes out with a dead
        // token -- and arrives back as a 401 that reads as misconfiguration.
        let (base_url, seen) = spawn_recording_endpoint(2);
        let client = OpenAiCompatibleClient::new(base_url, "m", Some("first-token".to_string()));

        drain(&client).await;
        client.credential().set(Some("second-token".to_string()));
        drain(&client).await;

        let headers = seen.lock().expect("lock").clone();
        assert_eq!(headers.len(), 2, "both requests were served");
        assert!(headers[0].ends_with("Bearer first-token"), "got {}", headers[0]);
        assert!(headers[1].ends_with("Bearer second-token"), "got {}", headers[1]);
    }

    #[tokio::test]
    async fn a_cleared_credential_stops_the_header_being_sent() {
        // Signing an account out has to actually stop the token going out,
        // not leave the last one in place until the daemon restarts.
        let (base_url, seen) = spawn_recording_endpoint(1);
        let client = OpenAiCompatibleClient::new(base_url, "m", Some("token".to_string()));

        client.credential().set(None);
        drain(&client).await;

        assert_eq!(seen.lock().expect("lock").as_slice(), &["".to_string()]);
    }

    #[test]
    fn a_credential_handle_is_shared_rather_than_copied() {
        // The handle is cloned into whatever refreshes it; a copy would swap a
        // value nothing reads.
        let client = OpenAiCompatibleClient::new("http://example.test", "m", None);
        let handle = client.credential();

        handle.set(Some("live".to_string()));

        assert_eq!(client.credential().get().as_deref(), Some("live"));
    }
}
