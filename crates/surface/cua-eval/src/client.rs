//! The daemon's HTTP API as the runner uses it: the same routes a person's
//! client calls (chat, devices, approvals, events), nothing private.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

/// One `chat.tool.called` event.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub sequence: i64,
    pub name: String,
    pub arguments: String,
    pub output: String,
    pub is_error: bool,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub approval_id: String,
    pub action: String,
    pub status: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub device_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub platform: String,
    pub online: bool,
    #[serde(default)]
    pub session: Option<String>,
}

/// What a chat query answered, or why it did not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub http_status: u16,
    pub status: String,
    pub reply: String,
    pub finish_reason: String,
    pub error_code: String,
    pub error: String,
    pub turn_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl Client {
    /// `server` is the daemon's (or gateway's) base URL. A loopback server
    /// is reached directly, never through a proxy from the environment.
    pub fn new(server: &str, token: &str) -> Result<Self, String> {
        let base = server.trim_end_matches('/').to_string();
        let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
        if is_loopback(&base) {
            builder = builder.no_proxy();
        }
        Ok(Self {
            base,
            token: token.trim().to_string(),
            http: builder.build().map_err(|err| err.to_string())?,
        })
    }

    /// Local pairing (`mode: local`), as the web UI does on a daemon the
    /// person runs themselves. Returns the API token.
    pub async fn login_local(server: &str, label: &str) -> Result<String, String> {
        let client = Self::new(server, "")?;
        let started = client
            .send(
                reqwest::Method::POST,
                "/v1/auth/pairings/start",
                Some(json!({"mode": "local", "label": label})),
                Duration::from_secs(20),
            )
            .await?;
        let pairing = started["pairing"]["pairingId"].as_str().unwrap_or_default();
        let code = started["pairingCode"].as_str().unwrap_or_default();
        if pairing.is_empty() || code.is_empty() {
            return Err(format!("unexpected pairing answer: {started}"));
        }
        let done = client
            .send(
                reqwest::Method::POST,
                &format!("/v1/auth/pairings/{pairing}/complete"),
                Some(json!({"code": code})),
                Duration::from_secs(20),
            )
            .await?;
        done["accessToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "the pairing answer carries no access token".to_string())
    }

    /// A single-use device pairing code for `kura-device pair`.
    pub async fn device_pairing_code(&self) -> Result<String, String> {
        let answer = self
            .send(
                reqwest::Method::POST,
                "/v1/devices/pairings",
                Some(json!({})),
                Duration::from_secs(20),
            )
            .await?;
        answer["code"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("unexpected answer: {answer}"))
    }

    pub async fn devices(&self) -> Result<Vec<Device>, String> {
        let answer = self
            .send(
                reqwest::Method::GET,
                "/v1/devices",
                None,
                Duration::from_secs(20),
            )
            .await?;
        serde_json::from_value(answer["items"].clone()).map_err(|err| format!("/v1/devices: {err}"))
    }

    /// One chat turn. `request_id` names it, so it can be cancelled.
    pub async fn chat(
        &self,
        request_id: &str,
        query: &str,
        provider: &str,
        model: &str,
        timeout: Duration,
    ) -> Result<Reply, String> {
        let mut body = json!({"query": query});
        if !provider.is_empty() {
            body["provider"] = provider.into();
        }
        if !model.is_empty() {
            body["model"] = model.into();
        }
        let response = self
            .http
            .post(format!("{}/v1/chat/query", self.base))
            .bearer_auth(&self.token)
            .header("x-request-id", request_id)
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|err| format!("/v1/chat/query: {err}"))?;
        let http_status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let field = |name: &str| value[name].as_str().unwrap_or_default().to_string();
        let mut reply = Reply {
            http_status,
            status: field("status"),
            reply: field("reply"),
            finish_reason: field("finishReason"),
            error_code: field("errorCode"),
            error: field("error"),
            turn_id: field("turnId"),
            input_tokens: value["usage"]["inputTokens"].as_i64().unwrap_or(0),
            output_tokens: value["usage"]["outputTokens"].as_i64().unwrap_or(0),
        };
        if value.is_null() {
            reply.error = text.chars().take(300).collect();
        } else if reply.error_code.is_empty() {
            // An API error is `{code, message, error}`, not a dispatch.
            reply.error_code = field("code");
        }
        Ok(reply)
    }

    /// Asks a running turn to stop at its next round.
    pub async fn cancel(&self, request_id: &str) -> Result<(), String> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/chat/turns/{request_id}/cancel"),
            Some(json!({})),
            Duration::from_secs(20),
        )
        .await
        .map(|_| ())
    }

    /// `chat.tool.called` events after `cursor`, and the new cursor.
    pub async fn tool_calls_after(&self, cursor: i64) -> Result<(Vec<ToolCall>, i64), String> {
        let path = format!("/v1/events?category=chat&resourceKind=tool_call&cursor={cursor}");
        let answer = self
            .send(reqwest::Method::GET, &path, None, Duration::from_secs(30))
            .await?;
        let next = answer["nextCursor"].as_i64().unwrap_or(cursor).max(cursor);
        let calls = answer["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|event| event["name"] == "chat.tool.called")
                    .map(|event| {
                        let payload = &event["payload"];
                        let text = |name: &str| payload[name].as_str().unwrap_or("").to_string();
                        ToolCall {
                            sequence: event["sequence"].as_i64().unwrap_or(0),
                            name: text("name"),
                            arguments: text("arguments"),
                            output: text("output"),
                            is_error: payload["isError"].as_bool().unwrap_or(false),
                            duration_ms: payload["durationMs"].as_i64().unwrap_or(0),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((calls, next))
    }

    pub async fn approvals(&self, pending_only: bool) -> Result<Vec<Approval>, String> {
        let path = if pending_only {
            "/v1/policy/approvals?status=pending"
        } else {
            "/v1/policy/approvals"
        };
        let answer = self
            .send(reqwest::Method::GET, path, None, Duration::from_secs(20))
            .await?;
        serde_json::from_value(answer["items"].clone())
            .map_err(|err| format!("/v1/policy/approvals: {err}"))
    }

    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        resolution: &str,
    ) -> Result<(), String> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/policy/approvals/{approval_id}/resolve"),
            Some(json!({
                "resolution": resolution,
                "comment": "kura-cua-eval: the suite's tasks never need this action"
            })),
            Duration::from_secs(20),
        )
        .await
        .map(|_| ())
    }

    /// `kura_device_screenshot_bytes_total`, or `None` when `/metrics` is
    /// not readable with this token (a tenant behind a gateway).
    pub async fn screenshot_bytes_total(&self) -> Option<u64> {
        let response = self
            .http
            .get(format!("{}/metrics", self.base))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let text = response.text().await.ok()?;
        Some(metric_sum(&text, "kura_device_screenshot_bytes_total"))
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .timeout(timeout);
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("{path}: {err}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "{path}: {status}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|err| format!("{path}: {err}"))
    }
}

/// Sums every sample of a Prometheus counter (any labels).
#[must_use]
pub fn metric_sum(text: &str, name: &str) -> u64 {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let rest = line.strip_prefix(name)?;
            if !(rest.starts_with(' ') || rest.starts_with('{')) {
                return None;
            }
            let value = rest.rsplit(' ').next()?;
            value.parse::<f64>().ok()
        })
        .map(|value| value.max(0.0) as u64)
        .sum()
}

fn is_loopback(base: &str) -> bool {
    let rest = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))
        .unwrap_or(base);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    let host = if rest.starts_with('[') {
        rest.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_samples_are_summed() {
        let text = "# TYPE kura_device_screenshot_bytes_total counter\n\
                    kura_device_screenshot_bytes_total 1200\n\
                    kura_device_screenshot_bytes_total_other 5\n\
                    kura_device_screenshot_bytes_total{x=\"y\"} 34\n";
        assert_eq!(metric_sum(text, "kura_device_screenshot_bytes_total"), 1234);
        assert_eq!(metric_sum("", "kura_device_screenshot_bytes_total"), 0);
    }

    #[test]
    fn loopback_servers_skip_the_proxy() {
        assert!(is_loopback("http://127.0.0.1:19193"));
        assert!(is_loopback("http://localhost"));
        assert!(is_loopback("http://[::1]:80"));
        assert!(!is_loopback("https://cell.example.com"));
    }
}
