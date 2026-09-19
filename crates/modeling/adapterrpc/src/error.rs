//! Error taxonomy mirroring the Go package's sentinel errors and `AdapterError`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Classifies an adapter failure so the daemon can map it to existing integration
/// diagnostics and live-validation classification (FR-007).
///
/// Like the Go `FailureKind string`, unknown wire values are preserved in
/// [`FailureKind::Other`] instead of failing decode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FailureKind {
    Auth,
    Scope,
    RateLimited,
    Unavailable,
    Malformed,
    Internal,
    /// Non-contract failure kind, preserved verbatim.
    Other(String),
}

impl FailureKind {
    pub fn as_str(&self) -> &str {
        match self {
            FailureKind::Auth => "auth",
            FailureKind::Scope => "scope",
            FailureKind::RateLimited => "rate_limited",
            FailureKind::Unavailable => "unavailable",
            FailureKind::Malformed => "malformed",
            FailureKind::Internal => "internal",
            FailureKind::Other(s) => s,
        }
    }

    fn from_wire(s: &str) -> Self {
        match s {
            "auth" => FailureKind::Auth,
            "scope" => FailureKind::Scope,
            "rate_limited" => FailureKind::RateLimited,
            "unavailable" => FailureKind::Unavailable,
            "malformed" => FailureKind::Malformed,
            "internal" => FailureKind::Internal,
            other => FailureKind::Other(other.to_owned()),
        }
    }
}

impl std::fmt::Display for FailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for FailureKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FailureKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(FailureKind::from_wire(&s))
    }
}

/// Typed adapter failure carried back to the domain shim, which maps it onto existing
/// integration diagnostics / live-validation classification. A clean `Status::Failure`
/// response is a confirmed non-commit and is NOT ambiguous (FR-007a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub kind: FailureKind,
    pub detail: String,
}

impl AdapterError {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        AdapterError {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "integration adapter failure ({})", self.kind)
        } else {
            write!(
                f,
                "integration adapter failure ({}): {}",
                self.kind, self.detail
            )
        }
    }
}

impl std::error::Error for AdapterError {}

/// Errors produced by the client/transport while dispatching an operation.
///
/// `Timeout`, `Unreachable`, and `MalformedResponse` describe outcomes where a
/// side-effecting operation's result cannot be confirmed (deadline expiry, transport
/// break, or an undecodable response). For a write, these are ambiguous-commit conditions
/// (FR-007a): the daemon must not assume committed or failed. See [`Error::is_ambiguous`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The adapter advertises a contract version that is not an exact match for the
    /// daemon's (FR-008).
    #[error("integration adapter contract version mismatch")]
    ContractMismatch,
    /// A response's requestId does not match the request.
    #[error("integration adapter response correlation mismatch")]
    Correlation,
    /// Deadline expiry: the outcome is unconfirmed (hang/timeout, FR-007b).
    #[error("integration adapter operation timed out")]
    Timeout,
    /// The adapter process/stream is gone or the transport was poisoned by a previous
    /// unconfirmed outcome.
    #[error("integration adapter unreachable")]
    Unreachable,
    /// The response frame or an ok payload could not be decoded.
    #[error("integration adapter response malformed: {0}")]
    MalformedResponse(String),
    /// Marshaling the resource or payload failed daemon-side (request never sent).
    #[error("marshal {domain}/{operation} {what}: {source}")]
    Marshal {
        domain: String,
        operation: String,
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// The credential resolver failed; the operation fails closed (request never sent).
    #[error("resolve {domain}/{operation} credentials: {source}")]
    Credential {
        domain: String,
        operation: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Spawning or wiring the adapter process failed.
    #[error("adapter {what}: {source}")]
    Process {
        what: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A confirmed adapter failure (clean `Status::Failure` response).
    #[error(transparent)]
    Adapter(#[from] AdapterError),
}

impl Error {
    /// Whether this error represents an unconfirmed outcome (FR-007a). Mirrors Go
    /// `IsAmbiguous`: deadline expiry, transport break, or an undecodable response.
    pub fn is_ambiguous(&self) -> bool {
        matches!(
            self,
            Error::Timeout | Error::Unreachable | Error::MalformedResponse(_)
        )
    }
}

/// Whether `err` represents an unconfirmed outcome (FR-007a). Free-function mirror of Go
/// `IsAmbiguous`.
pub fn is_ambiguous(err: &Error) -> bool {
    err.is_ambiguous()
}

/// Errors from the newline-delimited JSON codec.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("encode frame: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("frame io: {0}")]
    Io(#[from] std::io::Error),
}
