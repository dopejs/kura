//! Feishu/Lark HTTP client (port of client.go's Client + call): a thin Feishu Open Platform
//! client with an injectable base URL, exercised against synthetic/recorded responses in CI.

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_BASE_URL, FaultKind, ProviderFault, ambiguous_fault, feishu_code_fault,
    http_status_fault,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(25);

pub struct Client {
    base_url: String,
}

impl Client {
    /// Builds a client. An empty base URL uses the Feishu default; a trailing slash is trimmed.
    pub fn new(base_url: &str) -> Self {
        let base_url = if base_url.trim().is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base_url.trim().trim_end_matches('/').to_string()
        };
        Client { base_url }
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Performs one request and decodes data into out. The write flag marks side-effecting calls
    /// so an unconfirmed outcome (mid-response transport break) is reported as ambiguous-commit.
    pub fn call<R: Serialize, O: DeserializeOwned>(
        &self,
        deadline: Option<Duration>,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&R>,
        out: Option<&mut O>,
        write: bool,
    ) -> Result<(), ProviderFault> {
        let url = format!("{}{}", self.base_url, path);
        let request = ureq::request(method, &url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json; charset=utf-8")
            .timeout(deadline.unwrap_or(DEFAULT_TIMEOUT));

        let response = match body {
            Some(body) => {
                let bytes = serde_json::to_vec(body).map_err(|_| ProviderFault {
                    kind: FaultKind::Internal,
                    code: "request_encode_failed".to_string(),
                    message: "request body encode failed".to_string(),
                })?;
                request.send(bytes.as_slice())
            }
            None => request.call(),
        };

        let (status, raw) = match response {
            Ok(resp) => {
                let code = resp.status();
                let mut buf = Vec::new();
                let mut reader = resp.into_reader();
                if reader.read_to_end(&mut buf).is_err() {
                    if write {
                        return Err(ambiguous_fault(
                            "provider response truncated after acknowledgement",
                        ));
                    }
                    return Err(ProviderFault {
                        kind: FaultKind::Unavailable,
                        code: "provider_unavailable".to_string(),
                        message: "provider response read failed".to_string(),
                    });
                }
                (code, buf)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let mut buf = Vec::new();
                let _ = resp.into_reader().read_to_end(&mut buf);
                (code, buf)
            }
            Err(ureq::Error::Transport(_)) => {
                if write {
                    return Err(ambiguous_fault(
                        "provider connection broke before acknowledgement",
                    ));
                }
                return Err(ProviderFault {
                    kind: FaultKind::Unavailable,
                    code: "provider_unavailable".to_string(),
                    message: "provider connection failed".to_string(),
                });
            }
        };

        if let Some(fault) = http_status_fault(status, write) {
            return Err(fault);
        }

        let envelope: Envelope = serde_json::from_slice(&raw).map_err(|_| {
            if write {
                ambiguous_fault("provider acknowledgement unparseable")
            } else {
                ProviderFault {
                    kind: FaultKind::Unavailable,
                    code: "provider_unavailable".to_string(),
                    message: "provider response unparseable".to_string(),
                }
            }
        })?;
        if envelope.code != 0 {
            return Err(feishu_code_fault(envelope.code));
        }
        if let (Some(out), Some(data)) = (out, envelope.data) {
            if !data.is_null() {
                let value: O = serde_json::from_value(data).map_err(|_| {
                    if write {
                        ambiguous_fault("provider acknowledgement payload unparseable")
                    } else {
                        ProviderFault {
                            kind: FaultKind::Unavailable,
                            code: "provider_unavailable".to_string(),
                            message: "provider data unparseable".to_string(),
                        }
                    }
                })?;
                *out = value;
            }
        }
        Ok(())
    }
}

/// The standard Feishu response wrapper.
#[derive(Debug, Clone, Default, Deserialize)]
struct Envelope {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    #[allow(dead_code)]
    msg: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}
