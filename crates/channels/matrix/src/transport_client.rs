//! Port of daemon/internal/connectors/matrix/transport_client.go: the REST
//! Matrix client-server API transport. Sync is a long-polling loop over
//! `/_matrix/client/v3/sync` with a `since` cursor; replies are sent via
//! `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`;
//! binding validation uses `/account/whoami` and room membership state.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::Utc;
use kura_connectors::RedactionStatus;
use kura_imtypes::{OutboundReply, ReplyCapabilities, SentReply};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::routes::normalize_route_policy;
use crate::smoke::SmokeTransport;
use crate::types::{
    AuthorizationState, ConversationType, HomeserverBinding, HomeserverCapabilityState,
    InboundEvent, MessageKind, RoomSelectionState, RoutePolicy, RoutePolicyState,
};

/// Go `AccessTokenProvider`.
pub trait AccessTokenProvider: Send + Sync {
    /// Go `MatrixAccessToken(ctx, connectorID)` (context dropped).
    fn matrix_access_token(&self, connector_id: &str) -> Result<String, String>;
}

/// Go `ClientTransportConfig`. The Go `HTTPClient` injection point maps to
/// `timeout` (default 10s), the base URL being the injection seam in tests.
pub struct ClientTransportConfig {
    pub connector_id: String,
    pub homeserver_url: String,
    pub bot_access_token: String,
    pub access_token_source: Option<Box<dyn AccessTokenProvider>>,
    pub timeout: Duration,
    pub sync_poll_interval: Duration,
    pub sync_timeout: Duration,
    pub max_sync_cycles: i64,
    pub selected_room_ids: Vec<String>,
    pub allowed_direct_user_ids: Vec<String>,
}

impl Default for ClientTransportConfig {
    fn default() -> Self {
        ClientTransportConfig {
            connector_id: String::new(),
            homeserver_url: String::new(),
            bot_access_token: String::new(),
            access_token_source: None,
            timeout: Duration::from_secs(10),
            sync_poll_interval: Duration::ZERO,
            sync_timeout: Duration::ZERO,
            max_sync_cycles: 0,
            selected_room_ids: Vec::new(),
            allowed_direct_user_ids: Vec::new(),
        }
    }
}

/// Go `ClientAPIError`. `Display` matches Go's `Error()` exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientApiError {
    pub status_code: i32,
    pub err_code: String,
    pub message: String,
}

impl std::fmt::Display for ClientApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.message.trim().is_empty() {
            return f.write_str(self.message.trim());
        }
        if !self.err_code.trim().is_empty() {
            return write!(f, "matrix client api error: {}", self.err_code.trim());
        }
        if self.status_code > 0 {
            return write!(f, "matrix client api status {}", self.status_code);
        }
        f.write_str("matrix client api error")
    }
}

impl std::error::Error for ClientApiError {}

/// Errors surfaced by the client transport. Only `Api` carries the
/// structured status/code needed for validation-state classification.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Api(#[from] ClientApiError),
    #[error("json decode: {0}")]
    Json(String),
}

/// Go `ClientTransport`.
pub struct ClientTransport {
    connector_id: String,
    homeserver_url: String,
    bot_access_token: String,
    access_token_source: Option<Box<dyn AccessTokenProvider>>,
    timeout: Duration,
    sync_poll_interval: Duration,
    sync_timeout: Duration,
    max_sync_cycles: i64,
    selected_room_ids: HashSet<String>,
    allowed_direct_user_ids: HashSet<String>,
    bot_user_id: parking_lot::Mutex<String>,
}

/// Go `NewClientTransport`.
pub fn new_client_transport(cfg: ClientTransportConfig) -> Result<ClientTransport, String> {
    let base_url = cfg.homeserver_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err("matrix homeserver URL is required".to_string());
    }
    Ok(ClientTransport {
        connector_id: cfg.connector_id.trim().to_string(),
        homeserver_url: base_url,
        bot_access_token: cfg.bot_access_token.trim().to_string(),
        access_token_source: cfg.access_token_source,
        timeout: cfg.timeout,
        sync_poll_interval: cfg.sync_poll_interval,
        sync_timeout: cfg.sync_timeout,
        max_sync_cycles: cfg.max_sync_cycles,
        selected_room_ids: matrix_string_set(&cfg.selected_room_ids),
        allowed_direct_user_ids: matrix_string_set(&cfg.allowed_direct_user_ids),
        bot_user_id: parking_lot::Mutex::new(String::new()),
    })
}

impl ClientTransport {
    /// Go `ValidateHomeserverBinding`: `GET /account/whoami` and reconcile
    /// the returned user/device ids with the requested binding. On failure the
    /// binding's authorization/capability states are classified and returned
    /// alongside the error, exactly like Go.
    pub fn validate_homeserver_binding(
        &self,
        mut binding: HomeserverBinding,
    ) -> (HomeserverBinding, Result<(), String>) {
        let token = match self.access_token() {
            Ok(token) => token,
            Err(err) => {
                binding.authorization_state = AuthorizationState::Missing;
                return (binding, Err(err));
            }
        };
        #[derive(Deserialize)]
        struct WhoAmI {
            #[serde(default)]
            user_id: String,
            #[serde(default)]
            device_id: String,
        }
        let response: WhoAmI =
            match self.call("GET", "/_matrix/client/v3/account/whoami", &token, None) {
                Ok(response) => response,
                Err(err) => {
                    let (auth, capability) = matrix_validation_states_for_error(&err);
                    binding.authorization_state = auth;
                    binding.homeserver_capability_state = capability;
                    return (binding, Err(err.to_string()));
                }
            };
        let user_id = response.user_id.trim().to_string();
        if !binding.bot_user_id.trim().is_empty() && user_id != binding.bot_user_id.trim() {
            binding.authorization_state = AuthorizationState::OwnershipMismatch;
            binding.homeserver_capability_state = HomeserverCapabilityState::Valid;
            return (binding, Err(ERR_HOMESERVER_BINDING_INVALID.to_string()));
        }
        if !user_id.is_empty() {
            binding.bot_user_id = user_id;
        }
        binding.bot_device_id = response.device_id.trim().to_string();
        binding.authorization_state = AuthorizationState::Valid;
        binding.homeserver_capability_state = HomeserverCapabilityState::Valid;
        binding.validated_at = Utc::now();
        binding.redaction_status = RedactionStatus::Redacted;
        if binding.safe_evidence.is_empty() {
            binding.safe_evidence.insert(
                "provider".to_string(),
                "matrix_client_server_api".to_string(),
            );
        }
        *self.bot_user_id.lock() = binding.bot_user_id.clone();
        (binding, Ok(()))
    }

    /// Go `ValidateRoutePolicy`: normalizes the policy and verifies the bot
    /// has joined each selected room via the room membership state endpoint.
    pub fn validate_route_policy(
        &self,
        mut policy: RoutePolicy,
    ) -> (RoutePolicy, Result<(), String>) {
        let token = match self.access_token() {
            Ok(token) => token,
            Err(err) => return (policy, Err(err)),
        };
        policy = normalize_route_policy(policy, Utc::now());
        let bot_user_id = self.current_bot_user_id();
        for i in 0..policy.selected_rooms.len() {
            if policy.selected_rooms[i].conversation_type != ConversationType::Room
                || policy.selected_rooms[i].conversation_id.trim().is_empty()
            {
                continue;
            }
            if bot_user_id.is_empty() {
                policy.selected_rooms[i].validation_state = RoutePolicyState::Blocked;
                policy.selected_rooms[i].reason_code = "matrix_bot_identity_missing".to_string();
                policy.validation_state = RoutePolicyState::Blocked;
                return (policy, Err(ERR_HOMESERVER_BINDING_INVALID.to_string()));
            }
            let path = format!(
                "/_matrix/client/v3/rooms/{}/state/m.room.member/{}",
                path_escape(&policy.selected_rooms[i].conversation_id),
                path_escape(&bot_user_id)
            );
            #[derive(Deserialize)]
            struct Membership {
                #[serde(default)]
                membership: String,
            }
            let response: Membership = match self.call("GET", &path, &token, None) {
                Ok(response) => response,
                Err(err) => {
                    policy.selected_rooms[i].validation_state = RoutePolicyState::Blocked;
                    policy.selected_rooms[i].room_selection_state =
                        RoomSelectionState::MissingMembership;
                    policy.selected_rooms[i].reason_code =
                        "matrix_room_permission_missing".to_string();
                    policy.validation_state = RoutePolicyState::Blocked;
                    policy.reason_code = policy.selected_rooms[i].reason_code.clone();
                    return (policy, Err(err.to_string()));
                }
            };
            if response.membership != "join" {
                policy.selected_rooms[i].validation_state = RoutePolicyState::Blocked;
                policy.selected_rooms[i].room_selection_state =
                    RoomSelectionState::MissingMembership;
                policy.selected_rooms[i].reason_code = "matrix_room_membership_missing".to_string();
                policy.validation_state = RoutePolicyState::Blocked;
                policy.reason_code = policy.selected_rooms[i].reason_code.clone();
                return (policy, Err(ERR_HOMESERVER_BINDING_INVALID.to_string()));
            }
            policy.selected_rooms[i].validation_state = RoutePolicyState::Valid;
            policy.selected_rooms[i].room_selection_state = RoomSelectionState::Selected;
        }
        policy.validation_state = RoutePolicyState::Valid;
        policy.validated_at = Utc::now();
        policy.redaction_status = RedactionStatus::Redacted;
        if policy.safe_evidence.is_empty() {
            policy.safe_evidence.insert(
                "provider".to_string(),
                "matrix_client_server_api".to_string(),
            );
        }
        (policy, Ok(()))
    }

    /// Go `accessToken`: the configured static token wins; otherwise the
    /// access-token provider is consulted.
    fn access_token(&self) -> Result<String, String> {
        if !self.bot_access_token.trim().is_empty() {
            return Ok(self.bot_access_token.trim().to_string());
        }
        let Some(source) = &self.access_token_source else {
            return Err("matrix bot access token is not configured".to_string());
        };
        let token = source.matrix_access_token(&self.connector_id)?;
        if token.trim().is_empty() {
            return Err("matrix bot access token is not configured".to_string());
        }
        Ok(token.trim().to_string())
    }

    /// Go `currentBotUserID`.
    fn current_bot_user_id(&self) -> String {
        self.bot_user_id.lock().trim().to_string()
    }

    /// Go `syncOnce`: one long-polled sync request.
    fn sync_once(
        &self,
        token: &str,
        since: &str,
        timeout: Duration,
    ) -> Result<MatrixSyncResponse, String> {
        let mut query = format!("timeout={}", timeout.as_millis());
        if !since.trim().is_empty() {
            let encoded: String =
                url::form_urlencoded::byte_serialize(since.trim().as_bytes()).collect();
            query = format!("since={encoded}&{query}");
        }
        let path = format!("/_matrix/client/v3/sync?{query}");
        self.call("GET", &path, token, None)
            .map_err(|e| e.to_string())
    }

    /// Go `call`: one HTTP request with a Bearer token, JSON payload, and
    /// decoded response. Non-2xx responses are surfaced as [ClientApiError]
    /// with the Matrix `errcode`/`error` body; transport failures become
    /// `network_failed` client api errors.
    fn call<O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        token: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<O, ClientError> {
        let url = format!("{}{}", self.homeserver_url, path);
        let mut request = ureq::request(method, &url)
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(self.timeout);
        if payload.is_some() {
            request = request.set("Content-Type", "application/json");
        }
        let response = match payload {
            Some(payload) => {
                let bytes = serde_json::to_vec(payload).map_err(|e| {
                    ClientError::Message(format!("request body encode failed: {e}"))
                })?;
                request.send_bytes(&bytes)
            }
            None => request.call(),
        };
        let (status, raw) = match response {
            Ok(resp) => {
                let status = resp.status();
                let raw = resp.into_string().unwrap_or_default();
                (status, raw)
            }
            Err(ureq::Error::Status(status, resp)) => {
                let raw = resp.into_string().unwrap_or_default();
                (status, raw)
            }
            Err(ureq::Error::Transport(err)) => {
                return Err(ClientError::Api(ClientApiError {
                    status_code: 0,
                    err_code: "network_failed".to_string(),
                    message: err.to_string(),
                }));
            }
        };
        if (200..300).contains(&status) {
            let out =
                serde_json::from_str::<O>(&raw).map_err(|e| ClientError::Json(e.to_string()))?;
            return Ok(out);
        }
        #[derive(Deserialize, Default)]
        struct MatrixApiErrorBody {
            #[serde(default)]
            errcode: String,
            #[serde(default)]
            error: String,
        }
        let body: MatrixApiErrorBody = serde_json::from_str(&raw).unwrap_or_default();
        Err(ClientError::Api(ClientApiError {
            status_code: status as i32,
            err_code: body.errcode,
            message: body.error,
        }))
    }
}

impl crate::transport::Transport for ClientTransport {
    /// Go `Start`: long-poll `/_matrix/client/v3/sync`, mapping each joined
    /// room's timeline events into [InboundEvent]s until `max_sync_cycles`
    /// (when positive) is exhausted, sleeping `sync_poll_interval` between
    /// cycles.
    fn start(&self, handle: &dyn Fn(InboundEvent)) -> Result<(), String> {
        let token = self.access_token()?;
        let poll_interval = if self.sync_poll_interval <= Duration::ZERO {
            Duration::from_secs(1)
        } else {
            self.sync_poll_interval
        };
        let sync_timeout = if self.sync_timeout <= Duration::ZERO {
            Duration::from_secs(30)
        } else {
            self.sync_timeout
        };
        let mut since = String::new();
        let mut cycle: i64 = 0;
        loop {
            if self.max_sync_cycles > 0 && cycle >= self.max_sync_cycles {
                return Ok(());
            }
            let response = self.sync_once(&token, &since, sync_timeout)?;
            since = response.next_batch.trim().to_string();
            for event in response.inbound_events(
                &self.connector_id,
                &since,
                &self.homeserver_url,
                &self.selected_room_ids,
                &self.allowed_direct_user_ids,
            ) {
                handle(event);
            }
            if self.max_sync_cycles > 0 && cycle + 1 >= self.max_sync_cycles {
                return Ok(());
            }
            std::thread::sleep(poll_interval);
            cycle += 1;
        }
    }

    /// Go `SendReply`: sends an `m.text` message to the room with a stable
    /// `kura_<event-id>` transaction id and returns the assigned event id.
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        let room_id = reply.channel_id.trim().to_string();
        if room_id.is_empty() {
            return Err("matrix room id is required".to_string());
        }
        let token = self.access_token()?;
        let mut transaction_id = format!(
            "kura_{}",
            reply.reply_to_external_message_id.trim().replace('$', "")
        );
        if transaction_id == "kura_" {
            transaction_id = format!("kura_{}", Utc::now().timestamp_nanos_opt().unwrap_or(0));
        }
        let payload = serde_json::json!({
            "msgtype": "m.text",
            "body": reply.content.trim(),
        });
        #[derive(Deserialize)]
        struct SendResponse {
            #[serde(default)]
            event_id: String,
            #[serde(default)]
            errcode: String,
            #[serde(default)]
            error: String,
        }
        let path = format!(
            "/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            path_escape(&room_id),
            path_escape(&transaction_id)
        );
        let response: SendResponse = self
            .call("PUT", &path, &token, Some(&payload))
            .map_err(|e| e.to_string())?;
        if response.event_id.trim().is_empty() {
            return Err(ClientApiError {
                status_code: 0,
                err_code: response.errcode,
                message: response.error,
            }
            .to_string());
        }
        Ok(SentReply {
            external_message_id: response.event_id.trim().to_string(),
        })
    }

    fn reply_capabilities(&self) -> ReplyCapabilities {
        ReplyCapabilities {
            max_message_length: 40000,
            ..ReplyCapabilities::default()
        }
    }

    fn close(&self) -> Result<(), String> {
        Ok(())
    }
}

impl SmokeTransport for ClientTransport {
    fn validate_homeserver_binding(
        &self,
        binding: HomeserverBinding,
    ) -> (HomeserverBinding, Result<(), String>) {
        ClientTransport::validate_homeserver_binding(self, binding)
    }

    fn validate_route_policy(&self, policy: RoutePolicy) -> (RoutePolicy, Result<(), String>) {
        ClientTransport::validate_route_policy(self, policy)
    }

    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        crate::transport::Transport::send_reply(self, reply)
    }
}

/// Go `matrixSyncResponse` — only the joined-room timelines are consumed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MatrixSyncResponse {
    next_batch: String,
    rooms: MatrixSyncRooms,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MatrixSyncRooms {
    join: HashMap<String, MatrixSyncRoom>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MatrixSyncRoom {
    timeline: MatrixSyncTimeline,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MatrixSyncTimeline {
    events: Vec<MatrixSyncEvent>,
}

#[derive(Debug, Clone, Deserialize)]
struct MatrixSyncEvent {
    #[serde(rename = "type", default)]
    type_: String,
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    sender: String,
    #[serde(default)]
    origin_server_ts: i64,
    #[serde(default)]
    content: serde_json::Map<String, serde_json::Value>,
}

impl MatrixSyncResponse {
    /// Go `inboundEvents`: classifies each timeline event into an
    /// [InboundEvent] with the message-kind/text rules (m.text unencrypted,
    /// m.room.encrypted unsupported, everything else unsupported), room vs
    /// direct conversation typing, and the sender-derived homeserver id.
    fn inbound_events(
        &self,
        connector_id: &str,
        sync_batch_id: &str,
        homeserver_url: &str,
        selected_room_ids: &HashSet<String>,
        allowed_direct_user_ids: &HashSet<String>,
    ) -> Vec<InboundEvent> {
        let mut items = Vec::new();
        for (room_id, room) in &self.rooms.join {
            for event in &room.timeline.events {
                let mut message_kind = MessageKind::Unsupported;
                let mut text = String::new();
                if event.type_ == "m.room.message" {
                    let msg_type = event.content.get("msgtype").and_then(|v| v.as_str());
                    if msg_type == Some("m.text") {
                        message_kind = MessageKind::UnencryptedText;
                        text = event
                            .content
                            .get("body")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                    }
                }
                if event.type_ == "m.room.encrypted" {
                    message_kind = MessageKind::EncryptedUnsupported;
                }
                if event.event_id.trim().is_empty() {
                    continue;
                }
                let received_at = if event.origin_server_ts > 0 {
                    chrono::DateTime::<Utc>::from_timestamp_millis(event.origin_server_ts)
                        .unwrap_or_else(Utc::now)
                } else {
                    Utc::now()
                };
                let mut conversation_type = ConversationType::Room;
                if !selected_room_ids.contains(room_id.trim())
                    && allowed_direct_user_ids.contains(event.sender.trim())
                {
                    conversation_type = ConversationType::DirectMessage;
                }
                items.push(InboundEvent {
                    connector_id: connector_id.to_string(),
                    homeserver_id: matrix_homeserver_id(&event.sender, homeserver_url),
                    conversation_id: room_id.clone(),
                    matrix_event_id: event.event_id.clone(),
                    sync_batch_id: sync_batch_id.to_string(),
                    sender_id: event.sender.clone(),
                    conversation_type,
                    message_kind,
                    text,
                    received_at,
                    ..InboundEvent::default()
                });
            }
        }
        items
    }
}

/// Go `matrixHomeserverID`: the domain after the first ':' in the sender
/// mxid, falling back to the configured homeserver URL hostname.
#[must_use]
pub fn matrix_homeserver_id(sender_id: &str, homeserver_url: &str) -> String {
    if let Some((_, domain)) = sender_id.trim().split_once(':') {
        if !domain.trim().is_empty() {
            return domain.trim().to_string();
        }
    }
    if let Ok(parsed) = url::Url::parse(homeserver_url) {
        if let Some(host) = parsed.host_str() {
            if !host.trim().is_empty() {
                return host.trim().to_string();
            }
        }
    }
    String::new()
}

/// Go `matrixStringSet`: trimmed, non-empty values as a set.
fn matrix_string_set(values: &[String]) -> HashSet<String> {
    values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .collect()
}

/// Go `matrixValidationStatesForError`: maps a client API error to the
/// binding authorization/capability states.
#[must_use]
pub fn matrix_validation_states_for_error(
    err: &ClientError,
) -> (AuthorizationState, HomeserverCapabilityState) {
    if let ClientError::Api(api) = err {
        match api.status_code {
            401 | 403 => {
                return (
                    AuthorizationState::Revoked,
                    HomeserverCapabilityState::Unknown,
                );
            }
            429 => {
                return (
                    AuthorizationState::Valid,
                    HomeserverCapabilityState::RateLimited,
                );
            }
            _ => {}
        }
        if api.status_code >= 500 {
            return (
                AuthorizationState::ProviderUnavailable,
                HomeserverCapabilityState::Unknown,
            );
        }
        if api.err_code == "network_failed" {
            return (
                AuthorizationState::NetworkFailed,
                HomeserverCapabilityState::Unknown,
            );
        }
    }
    (
        AuthorizationState::Unknown,
        HomeserverCapabilityState::Unknown,
    )
}

/// Go `url.PathEscape` for path segments (Go 1.24 semantics): escapes every
/// byte except unreserved characters and the `$ & + : @ =` sub-delims the
/// RFC allows unescaped in a path segment (so `!` becomes `%21` while
/// `@` stays).
#[must_use]
pub fn path_escape(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match *byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'$'
            | b'&'
            | b'+'
            | b':'
            | b'@'
            | b'=' => {
                out.push(*byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

use crate::readiness::ERR_HOMESERVER_BINDING_INVALID;
