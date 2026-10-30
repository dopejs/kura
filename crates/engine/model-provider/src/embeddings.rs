//! Text embeddings over the OpenAI-compatible `/embeddings` endpoint.
//!
//! The wire most embedding providers speak (OpenAI, Ollama's `/v1`, vLLM,
//! most gateways). A provider routed to the `embed` role that speaks
//! something else is not served here; the caller keeps its deterministic
//! fallback instead.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;

use crate::openai::{Credential, RESERVED_HEADERS};
use crate::provider::ProviderError;

/// Upper bound on one `/embeddings` request. Embedding is on the reply path
/// of a chat turn, so a hung endpoint must fail fast rather than hold it.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct OpenAiCompatibleEmbeddingClient {
    base_url: String,
    model: String,
    /// Shared with the chat client for the same account, so a refreshed
    /// token reaches embeddings without re-registering anything.
    api_key: Credential,
    headers: BTreeMap<String, String>,
    http: reqwest::Client,
}

impl OpenAiCompatibleEmbeddingClient {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, api_key: Credential) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            headers: BTreeMap::new(),
            http,
        }
    }

    /// See [`crate::OpenAiCompatibleClient::with_headers`]; reserved headers
    /// are dropped.
    #[must_use]
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers
            .into_iter()
            .filter(|(name, _)| !RESERVED_HEADERS.contains(&name.trim().to_lowercase().as_str()))
            .collect();
        self
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed `inputs` in one request. The result is positionally aligned
    /// with `inputs`; anything short of one non-empty vector per input, all
    /// of one dimension, is an error rather than a partial answer, because a
    /// caller caches these by position.
    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({ "model": self.model, "input": inputs });
        let mut http = self
            .http
            .post(format!("{}/embeddings", self.base_url))
            .json(&body);
        for (name, value) in &self.headers {
            http = http.header(name, value);
        }
        if let Some(key) = self.api_key.get() {
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
            .map_err(|err| ProviderError::Malformed(format!("invalid embeddings json: {err}")))?;
        parse_embeddings(&parsed, inputs.len())
    }
}

/// Order `data[]` by its `index` (the spec does not promise response order)
/// and check the shape a positional cache depends on.
fn parse_embeddings(parsed: &Value, expected: usize) -> Result<Vec<Vec<f32>>, ProviderError> {
    let entries = parsed
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Malformed("embeddings response has no data array".into()))?;
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (position, entry) in entries.iter().enumerate() {
        let index = entry
            .get("index")
            .and_then(Value::as_u64)
            .map_or(position, |i| i as usize);
        let vector: Vec<f32> = entry
            .get("embedding")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|v| v as f32)
                    .collect()
            })
            .unwrap_or_default();
        let Some(slot) = slots.get_mut(index) else {
            return Err(ProviderError::Malformed(format!(
                "embedding index {index} out of range for {expected} inputs"
            )));
        };
        *slot = Some(vector);
    }
    let vectors: Vec<Vec<f32>> = slots
        .into_iter()
        .enumerate()
        .map(|(i, slot)| match slot {
            Some(vector) if !vector.is_empty() => Ok(vector),
            _ => Err(ProviderError::Malformed(format!(
                "embeddings response has no vector for input {i}"
            ))),
        })
        .collect::<Result<_, _>>()?;
    let dim = vectors[0].len();
    if vectors.iter().any(|v| v.len() != dim) {
        return Err(ProviderError::Malformed(
            "embeddings response mixes vector dimensions".into(),
        ));
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::spawn_scripted_endpoint;

    #[test]
    fn vectors_are_ordered_by_index_not_response_order() {
        let parsed = json!({"data": [
            {"index": 1, "embedding": [0.0, 1.0]},
            {"index": 0, "embedding": [1.0, 0.0]},
        ]});
        let vectors = parse_embeddings(&parsed, 2).expect("parsed");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn a_short_or_ragged_answer_is_an_error_not_a_partial_result() {
        let missing = json!({"data": [{"index": 0, "embedding": [1.0]}]});
        assert!(parse_embeddings(&missing, 2).is_err());
        let ragged = json!({"data": [
            {"index": 0, "embedding": [1.0, 0.0]},
            {"index": 1, "embedding": [1.0]},
        ]});
        assert!(parse_embeddings(&ragged, 2).is_err());
        let out_of_range = json!({"data": [{"index": 5, "embedding": [1.0]}]});
        assert!(parse_embeddings(&out_of_range, 1).is_err());
    }

    #[tokio::test]
    async fn embeds_a_batch_with_the_shared_credential() {
        let (base_url, seen) = spawn_scripted_endpoint(vec![(
            200,
            r#"{"data":[{"index":0,"embedding":[0.6,0.8]},{"index":1,"embedding":[1.0,0.0]}]}"#,
        )]);
        let credential = Credential::new(Some("stale".into()));
        let client = OpenAiCompatibleEmbeddingClient::new(base_url, "embed-m", credential.clone());
        // Rotated after construction: the request must carry the new token.
        credential.set(Some("fresh".into()));
        let vectors = client
            .embed(&["a".to_string(), "b".to_string()])
            .await
            .expect("embedded");
        assert_eq!(vectors, vec![vec![0.6, 0.8], vec![1.0, 0.0]]);
        let request = seen.lock().expect("lock")[0].clone();
        assert!(request.starts_with("POST /embeddings "), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer fresh")
        );
        assert!(request.contains(r#""input":["a","b"]"#), "{request}");
        assert!(request.contains(r#""model":"embed-m""#), "{request}");
    }

    #[tokio::test]
    async fn a_rejected_request_surfaces_the_status() {
        let (base_url, _) = spawn_scripted_endpoint(vec![(400, r#"{"error":"no such model"}"#)]);
        let client = OpenAiCompatibleEmbeddingClient::new(base_url, "nope", Credential::new(None));
        let err = client
            .embed(&["a".to_string()])
            .await
            .expect_err("rejected");
        assert!(
            matches!(err, ProviderError::Status { status: 400, .. }),
            "{err}"
        );
    }
}
