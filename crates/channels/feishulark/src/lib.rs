//! Port of the Feishu/Lark provider fault taxonomy and credential envelope
//! (token.go + the fault-mapping core of client.go). The HTTP client and the
//! calendar/mail provider handlers are the next increment.

mod calendar;
mod client;
mod mail;
pub use calendar::*;
pub use client::*;
pub use mail::*;

use serde::{Deserialize, Serialize};

#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// The Feishu Open Platform base. Lark international uses open.larksuite.com; overridable at
/// wiring time.
pub const DEFAULT_BASE_URL: &str = "https://open.feishu.cn";

/// Confirmed-provider failure kind, mapped onto the adapter contract's failure kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultKind {
    Auth,
    Scope,
    RateLimited,
    Unavailable,
    Internal,
}

/// A confirmed provider failure carrying a stable, redacted token. It never carries
/// credential/token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFault {
    pub kind: FaultKind,
    pub code: String,
    pub message: String,
}

impl ProviderFault {
    #[must_use]
    pub fn new(kind: FaultKind, code: impl Into<String>) -> Self {
        ProviderFault {
            kind,
            code: code.into(),
            message: String::new(),
        }
    }

    #[must_use]
    pub fn is_ambiguous(&self) -> bool {
        self.code == AMBIGUOUS_CODE
    }

    #[must_use]
    pub fn to_adapter_fault(&self) -> kura_adapterprovider::Fault {
        kura_adapterprovider::Fault {
            kind: adapter_failure_kind(self.kind),
            code: self.code.clone(),
            message: self.message.clone(),
        }
    }
}

impl std::fmt::Display for ProviderFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            f.write_str(&self.code)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for ProviderFault {}

/// Maps a local fault kind onto the adapter contract's failure kind.
#[must_use]
pub fn adapter_failure_kind(kind: FaultKind) -> kura_adapterrpc::FailureKind {
    match kind {
        FaultKind::Auth => kura_adapterrpc::FailureKind::Auth,
        FaultKind::Scope => kura_adapterrpc::FailureKind::Scope,
        FaultKind::RateLimited => kura_adapterrpc::FailureKind::RateLimited,
        FaultKind::Unavailable => kura_adapterrpc::FailureKind::Unavailable,
        FaultKind::Internal => kura_adapterrpc::FailureKind::Internal,
    }
}

/// Marks a ProviderFault as an unconfirmed write outcome (FR-008).
pub const AMBIGUOUS_CODE: &str = "__ambiguous_commit__";

#[must_use]
pub fn ambiguous_fault(message: impl Into<String>) -> ProviderFault {
    ProviderFault {
        kind: FaultKind::Internal,
        code: AMBIGUOUS_CODE.to_string(),
        message: message.into(),
    }
}

/// Maps a transport-level HTTP status to a fault before the Feishu envelope is considered. 2xx
/// returns None so the envelope code governs.
#[must_use]
pub fn http_status_fault(status: u16, write: bool) -> Option<ProviderFault> {
    match status {
        200..=299 => None,
        401 => Some(ProviderFault {
            kind: FaultKind::Auth,
            code: "token_expired".to_string(),
            message: "provider rejected credentials".to_string(),
        }),
        403 => Some(ProviderFault {
            kind: FaultKind::Scope,
            code: "scope_not_granted".to_string(),
            message: "provider denied permission".to_string(),
        }),
        429 => Some(ProviderFault {
            kind: FaultKind::RateLimited,
            code: "rate_limited".to_string(),
            message: "provider rate limited".to_string(),
        }),
        s if s >= 500 => {
            if write {
                Some(ambiguous_fault(
                    "provider returned a server error after a write was submitted",
                ))
            } else {
                Some(ProviderFault {
                    kind: FaultKind::Unavailable,
                    code: "service_unavailable".to_string(),
                    message: "provider service unavailable".to_string(),
                })
            }
        }
        s => Some(ProviderFault {
            kind: FaultKind::Unavailable,
            code: "provider_unavailable".to_string(),
            message: format!("provider returned status {s}"),
        }),
    }
}

/// Maps a non-zero Feishu envelope code to a stable, redacted token. The numeric code is
/// preserved (non-secret, recognized by the feishu_lark classifier); the provider msg is NOT
/// forwarded to avoid leaking sensitive context.
#[must_use]
pub fn feishu_code_fault(code: i64) -> ProviderFault {
    match code {
        99991663 => ProviderFault {
            kind: FaultKind::Auth,
            code: "tenant_access_token_invalid approval".to_string(),
            message: "tenant authorization pending".to_string(),
        },
        99991664 | 99991665 => ProviderFault {
            kind: FaultKind::Auth,
            code: "app_access_token_invalid".to_string(),
            message: "app authorization missing".to_string(),
        },
        99991668 | 99991661 => ProviderFault {
            kind: FaultKind::Auth,
            code: "user_access_token_invalid".to_string(),
            message: "user authorization invalid".to_string(),
        },
        99991669 => ProviderFault {
            kind: FaultKind::Scope,
            code: "scope_not_granted".to_string(),
            message: "required scope not granted".to_string(),
        },
        99991677 => ProviderFault {
            kind: FaultKind::Auth,
            code: "token_expired".to_string(),
            message: "access token expired".to_string(),
        },
        1062502 | 429 => ProviderFault {
            kind: FaultKind::RateLimited,
            code: "rate_limited".to_string(),
            message: "provider rate limited".to_string(),
        },
        99992001 | 190002 => ProviderFault {
            kind: FaultKind::Internal,
            code: "recurrence_unsupported".to_string(),
            message: "provider does not support the requested recurrence operation".to_string(),
        },
        other => ProviderFault {
            kind: FaultKind::Unavailable,
            code: format!("provider_error_{other}"),
            message: "provider returned an error".to_string(),
        },
    }
}

/// The per-call credential envelope the daemon resolves (Roadmap 37 secret path) and passes to
/// the adapter. It carries only short-lived access material and granted scopes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedToken {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_scopes: Vec<String>,
}

/// Decodes the credential envelope, failing closed when material is absent so a missing/empty
/// credential never reaches the provider as an anonymous call (FR-012).
pub fn parse_token(raw: &[u8]) -> Result<ScopedToken, ProviderFault> {
    if raw.is_empty() {
        return Err(ProviderFault {
            kind: FaultKind::Auth,
            code: "access_token_missing".to_string(),
            message: "no credential material resolved for integration".to_string(),
        });
    }
    let token: ScopedToken = serde_json::from_slice(raw).map_err(|_| ProviderFault {
        kind: FaultKind::Auth,
        code: "access_token_missing".to_string(),
        message: "credential envelope unreadable".to_string(),
    })?;
    if token.access_token.trim().is_empty() {
        return Err(ProviderFault {
            kind: FaultKind::Auth,
            code: "access_token_missing".to_string(),
            message: "credential envelope carried no access token".to_string(),
        });
    }
    Ok(token)
}

impl ScopedToken {
    /// Whether the granted scope set includes want. An empty granted set is treated as
    /// "unknown / not restricted" so read paths are not blocked before the provider rules.
    #[must_use]
    pub fn has_scope(&self, want: &str) -> bool {
        if self.granted_scopes.is_empty() {
            return true;
        }
        self.granted_scopes
            .iter()
            .any(|s| s.trim().eq_ignore_ascii_case(want))
    }
}
