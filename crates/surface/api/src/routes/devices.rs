//! The user's own computers, for remote computer use
//! (docs/harness/remote-computer-use.md §4–§5).
//!
//! Signed in (tenant routes):
//! - `POST /v1/devices/pairings` — a single-use code, valid ten minutes.
//! - `GET /v1/devices` — the tenant's devices, with who is online.
//! - `POST /v1/devices/{device_id}/revoke` — revokes, and drops a live
//!   connection at once.
//!
//! Device-authenticated (open routes; the credential is the device's own):
//! - `POST /v1/devices/pair` — trades a code for a device token.
//! - `GET /v1/devices/connect` — the device's WebSocket, authenticated by
//!   `X-Kura-Device-Token` before the upgrade.
//!
//! A device of another tenant is a 404, never a 403.

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use kura_devices::{ConnectionConfig, Hub, RegistryError, registry};
use kura_store::DeviceRecord;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::ActingTenant;
use crate::response::Json;
use crate::state::AppState;

/// The header a device sends its token in. Not `Authorization`: a gateway
/// in front of the cell replaces that with its own credential.
pub const DEVICE_TOKEN_HEADER: &str = "x-kura-device-token";

/// Signed-in routes, mounted behind `protected()`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/pairings", post(start_pairing))
        .route("/v1/devices/{device_id}/revoke", post(revoke_device))
}

/// Device-authenticated routes, mounted open.
pub fn open_router() -> Router<AppState> {
    Router::new()
        .route("/v1/devices/pair", post(pair_device))
        .route("/v1/devices/connect", get(connect_device))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingResponse {
    code: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRequest {
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairResponse {
    device: DeviceView,
    /// Shown once; only its hash is kept.
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceView {
    device_id: String,
    name: String,
    platform: String,
    created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revoked_at: Option<DateTime<Utc>>,
    online: bool,
    /// A control session the user granted and that is still live.
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeviceList {
    items: Vec<DeviceView>,
}

fn view(device: DeviceRecord, hub: Option<&Hub>) -> DeviceView {
    let live = hub.and_then(|hub| {
        hub.online(&device.tenant_id)
            .into_iter()
            .find(|online| online.device_id == device.device_id)
    });
    DeviceView {
        online: live.is_some(),
        session: live.and_then(|live| live.session),
        device_id: device.device_id,
        name: device.name,
        platform: device.platform,
        created_at: device.created_at,
        last_seen_at: device.last_seen_at,
        revoked_at: device.revoked_at,
    }
}

/// The tenant and principal a signed-in request acts for; pairing needs
/// both, so the device has an owner to answer to.
fn owner(acting: &ActingTenant) -> Result<(String, String), ApiError> {
    let Some(context) = acting.context() else {
        return Err(ApiError::Forbidden(
            "managing devices needs a signed-in tenant".into(),
        ));
    };
    let tenant_id = context.0.tenant_id.trim().to_string();
    let principal_id = context.0.principal_id.trim().to_string();
    if tenant_id.is_empty() || principal_id.is_empty() {
        return Err(ApiError::Forbidden(
            "managing devices needs a signed-in tenant".into(),
        ));
    }
    Ok((tenant_id, principal_id))
}

fn registry_error(err: RegistryError) -> ApiError {
    match err {
        RegistryError::InvalidCode | RegistryError::InvalidToken | RegistryError::Revoked => {
            ApiError::Unauthorized(err.to_string())
        }
        RegistryError::Invalid(message) => ApiError::BadRequest(message),
        RegistryError::Store(message) => ApiError::from_store(message),
    }
}

#[allow(clippy::unused_async)]
async fn start_pairing(
    State(state): State<AppState>,
    acting: ActingTenant,
) -> Result<Response, ApiError> {
    let (tenant_id, principal_id) = owner(&acting)?;
    let now = Utc::now();
    let store = state.store.lock();
    let code =
        registry::start_pairing(&store, &tenant_id, &principal_id, now).map_err(registry_error)?;
    // Spent and expired codes are worthless; clearing them here keeps the
    // table to the codes of the last day without a maintenance job.
    if let Err(err) = store.prune_device_pairings(&(now - chrono::Duration::days(1))) {
        eprintln!("[kura] devices: pruning pairing codes failed: {err}");
    }
    drop(store);
    eprintln!("[kura] devices: tenant={tenant_id} principal={principal_id} pairing code issued");
    Ok((
        StatusCode::CREATED,
        Json(PairingResponse {
            code: code.code,
            expires_at: code.expires_at,
        }),
    )
        .into_response())
}

#[allow(clippy::unused_async)]
async fn list_devices(
    State(state): State<AppState>,
    acting: ActingTenant,
) -> Result<Json<DeviceList>, ApiError> {
    let (tenant_id, _) = owner(&acting)?;
    let devices = state
        .store
        .lock()
        .list_devices(&tenant_id)
        .map_err(ApiError::from_store)?;
    let hub = state.device_hub.as_deref();
    Ok(Json(DeviceList {
        items: devices
            .into_iter()
            .map(|device| view(device, hub))
            .collect(),
    }))
}

#[allow(clippy::unused_async)]
async fn revoke_device(
    State(state): State<AppState>,
    acting: ActingTenant,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceView>, ApiError> {
    let (tenant_id, _) = owner(&acting)?;
    let device_id = device_id.trim();
    let now = Utc::now();
    let device = {
        let store = state.store.lock();
        let Some(device) = store
            .get_device(&tenant_id, device_id)
            .map_err(ApiError::from_store)?
        else {
            return Err(ApiError::NotFound("device not found".into()));
        };
        if device.revoked_at.is_none() {
            store
                .revoke_device(&tenant_id, device_id, &now)
                .map_err(ApiError::from_store)?;
        }
        store
            .get_device(&tenant_id, device_id)
            .map_err(ApiError::from_store)?
            .unwrap_or(device)
    };
    // After the row: a connection that comes back in between is refused
    // by `authenticate`.
    if let Some(hub) = &state.device_hub {
        hub.revoke(device_id);
    }
    eprintln!("[kura] devices: tenant={tenant_id} device={device_id} revoked");
    Ok(Json(view(device, state.device_hub.as_deref())))
}

#[allow(clippy::unused_async)]
async fn pair_device(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let request: PairRequest = serde_json::from_slice(&body)
        .map_err(|err| ApiError::BadRequest(format!("invalid pairing request: {err}")))?;
    let paired = registry::redeem(
        &state.store.lock(),
        &request.code,
        &request.name,
        &request.platform,
        Utc::now(),
    )
    .map_err(|err| {
        if matches!(err, RegistryError::InvalidCode) {
            eprintln!("[kura] devices: pairing refused: invalid or expired code");
        }
        registry_error(err)
    })?;
    eprintln!(
        "[kura] devices: tenant={} device={} paired ({}, {})",
        paired.device.tenant_id,
        paired.device.device_id,
        paired.device.platform,
        paired.device.name
    );
    Ok((
        StatusCode::CREATED,
        Json(PairResponse {
            device: view(paired.device, None),
            token: paired.token,
        }),
    )
        .into_response())
}

async fn connect_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let Some(hub) = state.device_hub.clone() else {
        return Err(ApiError::NotFound("devices are not enabled".into()));
    };
    let token = headers
        .get(DEVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let device = registry::authenticate(&state.store.lock(), token).map_err(registry_error)?;
    let config = ConnectionConfig::default();
    let store = state.store.clone();
    Ok(upgrade
        .max_message_size(config.max_frame_bytes)
        .on_upgrade(move |socket| run_connection(hub, store, device, socket, config)))
}

async fn run_connection(
    hub: Arc<Hub>,
    store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
    device: DeviceRecord,
    socket: WebSocket,
    config: ConnectionConfig,
) {
    let started = Instant::now();
    touch(&store, &device);
    eprintln!(
        "[kura] devices: tenant={} device={} connected",
        device.tenant_id, device.device_id
    );
    let (sink, stream) = socket.split();
    // Text frames are the protocol; pings are answered by the server
    // library, and anything else ends the stream.
    let incoming = stream
        .take_while(|message| {
            futures::future::ready(matches!(
                message,
                Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_))
            ))
        })
        .filter_map(|message| {
            futures::future::ready(match message {
                Ok(Message::Text(text)) => Some(text.to_string()),
                _ => None,
            })
        });
    let outgoing = sink.with(|text: String| {
        futures::future::ready(Ok::<_, axum::Error>(Message::Text(text.into())))
    });
    let closed = kura_devices::serve(
        hub,
        &device,
        Box::pin(incoming),
        Box::pin(outgoing),
        &config,
    )
    .await;
    touch(&store, &device);
    eprintln!(
        "[kura] devices: tenant={} device={} disconnected after {}s: {closed:?}",
        device.tenant_id,
        device.device_id,
        started.elapsed().as_secs()
    );
}

fn touch(store: &parking_lot::Mutex<kura_store::SQLiteStore>, device: &DeviceRecord) {
    if let Err(err) = store.lock().touch_device(&device.device_id, &Utc::now()) {
        eprintln!(
            "[kura] devices: device={} recording last seen failed: {err}",
            device.device_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::tests_support::{request_json, request_json_as_tenant, test_state};
    use kura_device_protocol::{CellFrame, DeviceFrame, Hello, PROTOCOL_VERSION, Screen};
    use serde_json::json;
    use tokio_tungstenite::tungstenite;

    /// Checks a real response body against its schema in `schemas/api`.
    fn conforms(schema: &str, body: &serde_json::Value) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        kura_contracts::Validator::new(root)
            .validate_relative(
                format!("schemas/api/{schema}.schema.json"),
                &serde_json::to_vec(body).unwrap(),
            )
            .unwrap_or_else(|err| panic!("{schema}: {err:?}\n{body}"));
    }

    async fn pair(state: &AppState, tenant: &str) -> (String, String) {
        let (status, body) =
            request_json_as_tenant(state.clone(), tenant, "POST", "/v1/devices/pairings", None)
                .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        conforms("create-device-pairing.response", &body);
        let code = body["code"].as_str().unwrap().to_string();
        let request = json!({"code": code, "name": "work mac", "platform": "macos"});
        conforms("pair-device.request", &request);
        let (status, body) =
            request_json(state.clone(), "POST", "/v1/devices/pair", Some(request)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        conforms("pair-device.response", &body);
        (
            body["device"]["deviceId"].as_str().unwrap().to_string(),
            body["token"].as_str().unwrap().to_string(),
        )
    }

    #[tokio::test]
    async fn a_tenant_pairs_lists_and_revokes_only_its_own_devices() {
        let state = test_state();
        let (device_id, token) = pair(&state, "ten_a").await;
        assert!(token.starts_with("kdev.ten_a."));

        let (status, body) =
            request_json_as_tenant(state.clone(), "ten_a", "GET", "/v1/devices", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"][0]["deviceId"], json!(device_id));
        assert_eq!(body["items"][0]["online"], json!(false));
        assert!(body["items"][0].get("tokenHash").is_none());

        let (_, body) =
            request_json_as_tenant(state.clone(), "ten_b", "GET", "/v1/devices", None).await;
        assert_eq!(body["items"], json!([]));
        let revoke = format!("/v1/devices/{device_id}/revoke");
        let (status, _) =
            request_json_as_tenant(state.clone(), "ten_b", "POST", &revoke, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) =
            request_json_as_tenant(state.clone(), "ten_a", "POST", &revoke, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["revokedAt"].is_string());
        conforms("device-resource", &body);
        // Idempotent.
        let (status, _) =
            request_json_as_tenant(state.clone(), "ten_a", "POST", &revoke, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(registry::authenticate(&state.store.lock(), &token).is_err());
    }

    #[tokio::test]
    async fn pairing_needs_a_tenant_and_a_good_code() {
        let state = test_state();
        let (status, _) = request_json(state.clone(), "POST", "/v1/devices/pairings", None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = request_json(
            state.clone(),
            "POST",
            "/v1/devices/pair",
            Some(json!({"code": "kpair.ten_a.00", "name": "mac", "platform": "macos"})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/devices/pair",
            Some(json!({"code": 7})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_device_connects_over_a_websocket_and_is_cut_off_when_revoked() {
        let mut state = test_state();
        let hub = Arc::new(Hub::default());
        state.device_hub = Some(hub.clone());
        let (device_id, token) = pair(&state, "ten_a").await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::routes::router(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let url = format!("ws://{addr}/v1/devices/connect");

        // A wrong token is refused before the upgrade.
        let mut bad =
            tungstenite::client::IntoClientRequest::into_client_request(url.as_str()).unwrap();
        bad.headers_mut()
            .insert(DEVICE_TOKEN_HEADER, "kdev.ten_a.00".parse().unwrap());
        match tokio_tungstenite::connect_async(bad).await {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected a 401, got {other:?}"),
        }

        let mut request =
            tungstenite::client::IntoClientRequest::into_client_request(url.as_str()).unwrap();
        request
            .headers_mut()
            .insert(DEVICE_TOKEN_HEADER, token.parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let hello = DeviceFrame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            agent: "test".into(),
            platform: "macos".into(),
            screen: Screen {
                width: 1440,
                height: 900,
                scale: 2,
            },
        });
        socket
            .send(tungstenite::Message::Text(
                serde_json::to_string(&hello).unwrap(),
            ))
            .await
            .unwrap();
        let welcome = socket.next().await.unwrap().unwrap();
        let welcome: CellFrame = serde_json::from_str(welcome.to_text().unwrap()).unwrap();
        assert_eq!(
            welcome,
            CellFrame::Welcome {
                protocol: PROTOCOL_VERSION,
                device_id: device_id.clone()
            }
        );

        let (_, body) =
            request_json_as_tenant(state.clone(), "ten_a", "GET", "/v1/devices", None).await;
        assert_eq!(body["items"][0]["online"], json!(true));
        assert!(body["items"][0]["lastSeenAt"].is_string());
        conforms("device-list.response", &body);

        let revoke = format!("/v1/devices/{device_id}/revoke");
        let (status, _) =
            request_json_as_tenant(state.clone(), "ten_a", "POST", &revoke, None).await;
        assert_eq!(status, StatusCode::OK);
        // The cell tells the device and closes the connection.
        let mut frames = Vec::new();
        while let Some(Ok(message)) = socket.next().await {
            if let tungstenite::Message::Text(text) = message {
                frames.push(serde_json::from_str::<CellFrame>(&text).unwrap());
            }
        }
        assert!(
            frames.iter().any(
                |frame| matches!(frame, CellFrame::SessionEnd { reason, .. } if reason == "revoked")
            ),
            "{frames:?}"
        );
        assert!(hub.online("ten_a").is_empty());
    }
}
