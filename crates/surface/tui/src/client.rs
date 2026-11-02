use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryInput {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryResponse {
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub status: String,
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct StreamDelta {
    /// The current round's text so far. A turn that calls tools streams
    /// several rounds, and each starts over: what came before a tool call is
    /// interim, as in a non-streamed query.
    #[serde(default)]
    reply: String,
}

#[derive(Clone)]
pub struct Client {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Client {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// One-shot (non-streaming) chat query.
    #[allow(dead_code)]
    pub async fn query_chat(&self, input: &ChatQueryInput) -> Result<ChatQueryResponse, String> {
        let url = format!("{}/v1/chat/query", self.base_url);
        let resp = self
            .auth(self.http.post(&url).json(input))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        resp.json::<ChatQueryResponse>()
            .await
            .map_err(|e| e.to_string())
    }

    /// Streaming chat query via SSE. Invokes `on_reply` with the current
    /// round's text on every delta; returns the terminal response.
    pub async fn stream_chat(
        &self,
        input: &ChatQueryInput,
        mut on_reply: impl FnMut(String),
    ) -> Result<ChatQueryResponse, String> {
        let url = format!("{}/v1/chat/query/stream", self.base_url);
        let resp = self
            .auth(self.http.post(&url).json(input))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut terminal: Option<ChatQueryResponse> = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf.drain(..idx + 2);
                let mut event = String::new();
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                match event.as_str() {
                    "chat.query.delta" => {
                        if let Ok(d) = serde_json::from_str::<StreamDelta>(&data) {
                            on_reply(d.reply);
                        }
                    }
                    "chat.query.completed" | "chat.query.failed" | "chat.query.cancelled" => {
                        terminal = serde_json::from_str::<ChatQueryResponse>(&data).ok();
                    }
                    _ => {}
                }
            }
        }
        terminal.ok_or_else(|| "stream ended without a terminal event".to_string())
    }

    /// Stream the daemon event ledger via SSE (`/v1/events/stream`). Invokes
    /// `on_event(event_name, json)` for each event frame; returning `false`
    /// stops the subscription. Runs until the stream closes or `on_event`
    /// returns `false`.
    pub async fn stream_events(
        &self,
        mut on_event: impl FnMut(String, serde_json::Value) -> bool,
    ) -> Result<(), String> {
        let url = format!("{}/v1/events/stream", self.base_url);
        let resp = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf.drain(..idx + 2);
                let mut event = String::new();
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if data.is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
                    if !on_event(event, value) {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// GET an API path and return the JSON body.
    pub async fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    }

    /// POST a JSON body to an API path and return the JSON body.
    #[allow(dead_code)]
    pub async fn post_json(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .auth(self.http.post(&url).json(&body))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    }
}
