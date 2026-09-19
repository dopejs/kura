//! release route family (port of daemon/internal/api/release.go, Roadmap 72).
//!
//! Route: POST /v1/release/launch-gate. The handler is a pure validator over a
//! caller-supplied public-beta launch-gate evidence index and returns the
//! ship/no-ship decision; it owns no runtime truth.

use axum::body::Bytes;
use axum::routing::post;
use axum::{Json, Router};

use kura_opsreadiness as opsreadiness;

use crate::error::ApiError;
use crate::state::AppState;

use super::decode_json_required;

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/release/launch-gate", post(validate_launch_gate))
}

/// POST /v1/release/launch-gate (Go handleLaunchGateValidate) — 200 with the
/// decision; a missing or malformed body is 400.
async fn validate_launch_gate(body: Bytes) -> Result<Json<opsreadiness::LaunchDecision>, ApiError> {
    let evidence: opsreadiness::LaunchGateEvidence = decode_json_required(&body)?;
    Ok(Json(opsreadiness::validate_launch_gate(&evidence)))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn empty_evidence_is_a_no_ship_decision() {
        let (status, decision) = request_json(
            test_state(),
            "POST",
            "/v1/release/launch-gate",
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{decision}");
        assert_eq!(decision["result"], "no_ship", "{decision}");
    }

    #[tokio::test]
    async fn missing_body_is_400() {
        let (status, body) =
            request_json(test_state(), "POST", "/v1/release/launch-gate", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "request body is required");
    }
}
