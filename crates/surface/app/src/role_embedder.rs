//! The `embed` model role as the retrieval embedder (G10).
//!
//! When the `embed` role is routed to a provider that speaks the
//! OpenAI-compatible wire, retrieval ranks with that model's vectors instead
//! of the hashed n-gram default. Resolved once at boot: a role change applies
//! on restart, so one process never writes two vector spaces under a
//! fingerprint mid-flight.
//!
//! [`kura_context::Embedder`] is synchronous and runs on the reply path, while
//! the HTTP client is async. Requests run on a small runtime this embedder
//! owns and the caller waits on a channel — never `block_on` inside the
//! caller's runtime, which panics, and never a fresh runtime per call, which
//! would strand the client's pooled connections.

use std::sync::Arc;
use std::time::Duration;

use kura_api::state::AppState;
use kura_context::Embedder;
use kura_model_provider::{Credential, OpenAiCompatibleEmbeddingClient};

/// Uncached corpus docs one retrieval may embed inline. One batch request:
/// enough for the handful of memories written between turns, far short of a
/// corpus re-embed, which is the operator's explicit action.
const INLINE_EMBED_LIMIT: usize = 16;

/// Backstop over the client's own request timeout, so a wedged runtime
/// cannot hold a chat turn forever.
const REPLY_BACKSTOP: Duration = Duration::from_secs(20);

pub(crate) struct RoleEmbedder {
    name: String,
    fingerprint: String,
    client: Arc<OpenAiCompatibleEmbeddingClient>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl RoleEmbedder {
    fn new(
        provider_id: &str,
        client: OpenAiCompatibleEmbeddingClient,
    ) -> Result<RoleEmbedder, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("kura-embedder")
            .enable_all()
            .build()
            .map_err(|err| format!("embedder runtime: {err}"))?;
        Ok(RoleEmbedder {
            name: format!("{provider_id}/{}", client.model()),
            // Provider and model are the vector space as far as the daemon
            // can see; a provider that swaps weights behind a stable model id
            // needs `memory indexes rebuild` (see Embedder::fingerprint).
            fingerprint: format!("role-embed/{provider_id}/{}/v1", client.model()),
            client: Arc::new(client),
            runtime: Some(runtime),
        })
    }

    fn request(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| "embedder is shut down".to_string())?;
        let client = Arc::clone(&self.client);
        let texts = texts.to_vec();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        runtime.spawn(async move {
            let _ = tx.send(client.embed(&texts).await.map_err(|err| err.to_string()));
        });
        rx.recv_timeout(REPLY_BACKSTOP)
            .map_err(|_| "embedding request timed out".to_string())?
    }
}

impl Drop for RoleEmbedder {
    fn drop(&mut self) {
        // Dropping a runtime blocks, which panics inside an async context;
        // AppState can be dropped from one.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Unit length, so cosine is a dot product as the ranker assumes. OpenAI
/// normalizes; many local models do not.
fn normalized(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vector {
            *v /= norm;
        }
    }
    vector
}

impl Embedder for RoleEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        self.embed_batch(&[text.to_string()])
            .pop()
            .unwrap_or_default()
    }

    fn embed_batch(&self, texts: &[String]) -> Vec<Vec<f32>> {
        use kura_telemetry::metrics::{EMBEDDER_REQUESTS_TOTAL, registry};
        if texts.is_empty() {
            return Vec::new();
        }
        match self.request(texts) {
            Ok(vectors) => {
                registry().inc(
                    EMBEDDER_REQUESTS_TOTAL,
                    &[("embedder", &self.name), ("outcome", "ok")],
                );
                vectors.into_iter().map(normalized).collect()
            }
            Err(err) => {
                registry().inc(
                    EMBEDDER_REQUESTS_TOTAL,
                    &[("embedder", &self.name), ("outcome", "error")],
                );
                // Empty, never a fallback vector: see Embedder::embed_batch.
                eprintln!(
                    "[kura] embedder {} failed ({err}); ranking without vectors",
                    self.name
                );
                vec![Vec::new(); texts.len()]
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    fn inline_embed_limit(&self) -> Option<usize> {
        Some(INLINE_EMBED_LIMIT)
    }
}

/// The embedder for the routed `embed` role, or `None` (hashed n-gram stays
/// in charge). A role that is routed but cannot be served is logged, not
/// fatal: retrieval still works, only with the default vectors.
pub(crate) fn from_role(state: &AppState) -> Option<Arc<dyn Embedder>> {
    let binding =
        match kura_api::routes::providers::resolve_role(state, kura_config::ModelRole::Embed) {
            Ok(binding) => binding?,
            Err(err) => {
                eprintln!(
                    "[kura] embed role: cannot resolve the binding ({err}); using hashed n-gram"
                );
                return None;
            }
        };
    match build(state, &binding.0, &binding.1) {
        Ok(embedder) => {
            eprintln!(
                "[kura] embed role: retrieval vectors from {} (fingerprint {})",
                embedder.name(),
                embedder.fingerprint()
            );
            Some(Arc::new(embedder))
        }
        Err(reason) => {
            eprintln!(
                "[kura] embed role routed to {} but not served ({reason}); using hashed n-gram",
                binding.0
            );
            None
        }
    }
}

fn build(state: &AppState, provider_id: &str, model: &str) -> Result<RoleEmbedder, String> {
    let model = model.trim();
    // An empty model would mean "the provider's default", which for a chat
    // account is a chat model: no embeddings endpoint will accept it.
    if model.is_empty() {
        return Err("the binding names no embedding model".to_string());
    }
    let llm = &state.config.llm;
    let (base_url, headers, credential) = if provider_id == "openai_compatible" {
        let openai = &llm.openai_compatible;
        let credential = state.openai_credential.clone().unwrap_or_else(|| {
            let key = openai.api_key.trim();
            Credential::new((!key.is_empty()).then(|| key.to_string()))
        });
        (openai.base_url.trim(), openai.headers.clone(), credential)
    } else {
        let account = llm
            .accounts
            .iter()
            .find(|account| account.id.trim() == provider_id)
            .ok_or_else(|| "no configured account with that id".to_string())?;
        if account.protocol != kura_config::AccountProtocol::OpenAiCompatible {
            return Err(format!(
                "account protocol {:?} has no embeddings endpoint here",
                account.protocol
            ));
        }
        let credential = state
            .account_credentials
            .get(provider_id)
            .cloned()
            .unwrap_or_else(|| {
                let token = account.access_token.trim();
                Credential::new((!token.is_empty()).then(|| token.to_string()))
            });
        (account.base_url.trim(), account.headers.clone(), credential)
    };
    if base_url.is_empty() {
        return Err("the provider has no base URL".to_string());
    }
    let client =
        OpenAiCompatibleEmbeddingClient::new(base_url, model, credential).with_headers(headers);
    RoleEmbedder::new(provider_id, client)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves one canned 200 body per connection and records request bodies.
    fn spawn_endpoint(bodies: Vec<&'static str>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for (stream, body) in listener.incoming().zip(bodies) {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let length = text[..head_end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if request.len() >= head_end + 4 + length {
                            break;
                        }
                    }
                }
                recorded
                    .lock()
                    .expect("lock")
                    .push(String::from_utf8_lossy(&request).to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn embedder(base_url: &str) -> RoleEmbedder {
        RoleEmbedder::new(
            "acct",
            OpenAiCompatibleEmbeddingClient::new(base_url, "embed-m", Credential::new(None)),
        )
        .expect("embedder")
    }

    /// Called from inside a current-thread runtime — the chat reply path —
    /// it must neither panic nor deadlock, and it returns unit vectors.
    #[tokio::test]
    async fn embeds_from_inside_an_async_context_and_normalizes() {
        let (base_url, seen) = spawn_endpoint(vec![
            r#"{"data":[{"index":0,"embedding":[3.0,4.0]},{"index":1,"embedding":[0.0,2.0]}]}"#,
        ]);
        let embedder = embedder(&base_url);
        let vectors = embedder.embed_batch(&["a".to_string(), "b".to_string()]);
        assert_eq!(vectors, vec![vec![0.6, 0.8], vec![0.0, 1.0]]);
        assert_eq!(seen.lock().expect("lock").len(), 1, "one batched request");
        assert_eq!(embedder.fingerprint(), "role-embed/acct/embed-m/v1");
        assert_eq!(embedder.inline_embed_limit(), Some(INLINE_EMBED_LIMIT));
        drop(embedder);
    }

    #[test]
    fn an_unreachable_endpoint_yields_empty_vectors_not_fallback_ones() {
        let embedder = embedder("http://127.0.0.1:9");
        assert_eq!(
            embedder.embed_batch(&["a".to_string(), "b".to_string()]),
            vec![Vec::<f32>::new(), Vec::new()]
        );
        assert!(embedder.embed("a").is_empty());
    }
}
