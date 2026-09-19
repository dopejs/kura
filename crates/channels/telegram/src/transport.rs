//! Telegram Bot API transport (port of transport.go): REST long-poll
//! client over ureq plus the fake transport used by tests.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use kura_connectors::RedactionStatus;
use kura_im::ReplySender;
use kura_imtypes::{OutboundReply, ReplyCapabilities, SentReply};
use parking_lot::Mutex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::TelegramError;
use crate::allowment::{ConversationType, InboundUpdate, RouteDecision};
use crate::readiness::{AccountBinding, PermissionState};

/// Telegram channel transport boundary (Go `Transport`). Extends
/// [`ReplySender`] so the runtime can hand the transport directly to the
/// shared message loop.
pub trait Transport: ReplySender {
    /// Starts the long-poll loop feeding inbound updates to `handle`.
    fn start(&self, handle: Arc<dyn Fn(InboundUpdate) + Send + Sync>) -> Result<(), String>;
    /// The reply capabilities the transport can deliver (Go
    /// `ReplyCapabilities`).
    fn reply_capabilities(&self) -> ReplyCapabilities;
    /// Stops the long-poll loop.
    fn close(&self) -> Result<(), String>;
    /// Optional route-outcome recorder (Go
    /// `interface{ RecordRouteOutcome(RouteDecision) }`).
    fn record_route_outcome(&self, _decision: RouteDecision) {}
}

/// Configuration for [`BotApiTransport`] (Go `BotAPITransportConfig`).
#[derive(Debug, Clone)]
pub struct BotApiTransportConfig {
    pub connector_id: String,
    pub bot_token: String,
    pub bot_username: String,
    pub base_url: String,
    pub http_client: Option<ureq::Agent>,
    pub poll_interval: Duration,
    pub poll_timeout: Duration,
}

impl Default for BotApiTransportConfig {
    fn default() -> Self {
        BotApiTransportConfig {
            connector_id: String::new(),
            bot_token: String::new(),
            bot_username: String::new(),
            base_url: String::new(),
            http_client: None,
            poll_interval: Duration::ZERO,
            poll_timeout: Duration::ZERO,
        }
    }
}

/// Bot API REST long-poll transport (Go `BotAPITransport`).
pub struct BotApiTransport {
    connector_id: String,
    bot_token: String,
    // Kept for config parity with Go's BotAPITransportConfig.BotUsername;
    // the bot account label is sourced from the getMe response instead.
    #[allow(dead_code)]
    bot_username: String,
    base_url: String,
    agent: ureq::Agent,
    poll_interval: Duration,
    poll_timeout: Duration,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl BotApiTransport {
    /// Go `NewBotAPITransport`. An empty base URL uses the Telegram API
    /// default; a nil HTTP client uses a 35s-timeout agent; zero poll knobs
    /// default to 1s interval and 25s long-poll timeout.
    pub fn new(cfg: BotApiTransportConfig) -> Result<Self, TelegramError> {
        if cfg.bot_token.trim().is_empty() {
            return Err(TelegramError::BotTokenRequired);
        }
        let base_url = cfg.base_url.trim().trim_end_matches('/').to_string();
        let base_url = if base_url.is_empty() {
            "https://api.telegram.org".to_string()
        } else {
            base_url
        };
        let agent = cfg.http_client.unwrap_or_else(|| {
            ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(35))
                .build()
        });
        let poll_interval = if cfg.poll_interval <= Duration::ZERO {
            Duration::from_secs(1)
        } else {
            cfg.poll_interval
        };
        let poll_timeout = if cfg.poll_timeout <= Duration::ZERO {
            Duration::from_secs(25)
        } else {
            cfg.poll_timeout
        };
        Ok(BotApiTransport {
            connector_id: cfg.connector_id,
            bot_token: cfg.bot_token,
            bot_username: cfg.bot_username,
            base_url,
            agent,
            poll_interval,
            poll_timeout,
            cancel: Mutex::new(None),
        })
    }

    /// Go `ValidateCredential`: resolves the bot account via `getMe`.
    pub fn validate_credential(&self) -> Result<AccountBinding, TelegramApiError> {
        let mut response = TelegramApiResponse::<TelegramUser>::default();
        self.call("GET", "getMe", None, &mut response)?;
        if !response.ok {
            return Err(TelegramApiError {
                class: None,
                status_code: response.error_code,
                description: response.description,
            });
        }
        let account_id = format!("telegram_bot_{}", response.result.id);
        let label = response.result.username.trim().to_string();
        let label = if label.is_empty() {
            response.result.first_name.trim().to_string()
        } else {
            label
        };
        Ok(AccountBinding {
            connector_id: self.connector_id.clone(),
            connector_account_id: account_id,
            provider_account_label: label,
            permission_state: PermissionState::Valid,
            validated_at: Utc::now(),
            redaction_status: RedactionStatus::Redacted,
            safe_evidence: [("provider".to_string(), "telegram_bot_api".to_string())]
                .into_iter()
                .collect(),
            ..AccountBinding::default()
        })
    }

    /// Go `methodURL`.
    fn method_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.bot_token.trim(), method)
    }

    /// Go `call`.
    fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        api_method: &str,
        payload: Option<&Value>,
        out: &mut T,
    ) -> Result<(), TelegramApiError> {
        call_inner(
            &self.agent,
            &self.method_url(api_method),
            method,
            payload,
            out,
        )
    }
}

impl ReplySender for BotApiTransport {
    /// Go `SendReply`: sends the reply via `sendMessage`.
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        let mut payload = json!({ "chat_id": reply.channel_id, "text": reply.content });
        if !reply.reply_to_external_message_id.trim().is_empty() {
            payload["reply_to_message_id"] = json!(reply.reply_to_external_message_id);
        }
        let mut response = TelegramApiResponse::<TelegramMessage>::default();
        self.call("POST", "sendMessage", Some(&payload), &mut response)
            .map_err(|e| e.to_string())?;
        if !response.ok {
            return Err(TelegramApiError {
                class: None,
                status_code: response.error_code,
                description: response.description,
            }
            .to_string());
        }
        Ok(SentReply {
            external_message_id: response.result.message_id.to_string(),
        })
    }
}
impl Transport for BotApiTransport {
    /// Go `Start`: cancels any previous poll loop and spawns a new one.
    fn start(&self, handle: Arc<dyn Fn(InboundUpdate) + Send + Sync>) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let previous = self.cancel.lock().replace(cancel.clone());
        if let Some(previous) = previous {
            previous.store(true, Ordering::SeqCst);
        }
        let agent = self.agent.clone();
        let base_url = self.base_url.clone();
        let token = self.bot_token.clone();
        let poll_interval = self.poll_interval;
        let poll_timeout = self.poll_timeout;
        std::thread::spawn(move || {
            let mut offset: i64 = 0;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return;
                }
                let mut payload = json!({ "timeout": poll_timeout.as_secs() as i64 });
                if offset > 0 {
                    payload["offset"] = json!(offset);
                }
                let url = format!("{}/bot{}/{}", base_url, token.trim(), "getUpdates");
                let mut response = TelegramApiResponse::<Vec<TelegramUpdate>>::default();
                let err = call_inner(&agent, &url, "POST", Some(&payload), &mut response);
                if err.is_ok() && response.ok {
                    for update in &response.result {
                        if update.update_id >= offset {
                            offset = update.update_id + 1;
                        }
                        if let Some(inbound) = inbound_from_telegram_update(update) {
                            handle(inbound);
                        }
                    }
                }
                std::thread::sleep(poll_interval);
            }
        });
        Ok(())
    }

    fn reply_capabilities(&self) -> ReplyCapabilities {
        ReplyCapabilities {
            supports_thinking: false,
            supports_streaming: false,
            max_message_length: 4096,
        }
    }

    /// Go `Close`: cancels the current poll loop.
    fn close(&self) -> Result<(), String> {
        if let Some(cancel) = self.cancel.lock().take() {
            cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Go `call`: one Bot API request with the Go error-class semantics
/// (network failures are `network_failed`; decode failures are
/// `provider_unavailable`; HTTP >= 400 surfaces the status code).
fn call_inner<T: DeserializeOwned>(
    agent: &ureq::Agent,
    url: &str,
    method: &str,
    payload: Option<&Value>,
    out: &mut T,
) -> Result<(), TelegramApiError> {
    let request = agent.request(method, url);
    let response = match payload {
        Some(body) => request.send_json(body.clone()),
        None => request.call(),
    };
    let (status, raw) = match response {
        Ok(resp) => {
            let status = resp.status();
            let mut buf = Vec::new();
            if resp.into_reader().read_to_end(&mut buf).is_err() {
                return Err(TelegramApiError {
                    class: Some("provider_unavailable".to_string()),
                    status_code: status as i32,
                    description: "provider response read failed".to_string(),
                });
            }
            (status, buf)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let mut buf = Vec::new();
            let _ = resp.into_reader().read_to_end(&mut buf);
            (status, buf)
        }
        Err(ureq::Error::Transport(transport)) => {
            return Err(TelegramApiError {
                class: Some("network_failed".to_string()),
                status_code: 0,
                description: transport.to_string(),
            });
        }
    };
    match serde_json::from_slice::<T>(&raw) {
        Ok(value) => {
            *out = value;
            if status >= 400 {
                return Err(TelegramApiError {
                    class: None,
                    status_code: status as i32,
                    description: String::new(),
                });
            }
            Ok(())
        }
        Err(err) => Err(TelegramApiError {
            class: Some("provider_unavailable".to_string()),
            status_code: status as i32,
            description: err.to_string(),
        }),
    }
}
/// Standard Telegram Bot API response wrapper (Go `telegramAPIResponse[T]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramApiResponse<T> {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub result: T,
    #[serde(default)]
    pub error_code: i32,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub first_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: TelegramMessage,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    #[serde(default)]
    pub from: TelegramUser,
    #[serde(default)]
    pub chat: TelegramChat,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub voice: Option<Value>,
    #[serde(default)]
    pub document: Option<Value>,
    #[serde(default)]
    pub photo: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub type_: String,
}

/// Classified Telegram Bot API failure (Go `telegramAPIError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramApiError {
    /// Explicit error class when one was assigned at construction; `None`
    /// falls back to the status-code mapping in [`TelegramApiError::error_class`].
    pub class: Option<String>,
    pub status_code: i32,
    pub description: String,
}

impl TelegramApiError {
    /// Go `ErrorClass()`.
    #[must_use]
    pub fn error_class(&self) -> String {
        if let Some(class) = &self.class {
            if !class.trim().is_empty() {
                return class.clone();
            }
        }
        match self.status_code {
            401 => "auth_error".to_string(),
            403 => "permission_missing".to_string(),
            429 => "rate_limited".to_string(),
            s if s >= 500 => "provider_unavailable".to_string(),
            _ => "unknown_connector_failure".to_string(),
        }
    }
}

impl std::fmt::Display for TelegramApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.description.is_empty() {
            write!(f, "telegram bot api error: {}", self.description)
        } else {
            write!(f, "telegram bot api error: status {}", self.status_code)
        }
    }
}

impl std::error::Error for TelegramApiError {}

/// Go `inboundFromTelegramUpdate`: normalizes a Telegram update into an
/// [`InboundUpdate`], rejecting updates without message/chat identity.
#[must_use]
pub fn inbound_from_telegram_update(update: &TelegramUpdate) -> Option<InboundUpdate> {
    if update.message.message_id == 0 || update.message.chat.id == 0 {
        return None;
    }
    let conversation =
        if update.message.chat.type_ == "group" || update.message.chat.type_ == "supergroup" {
            ConversationType::Group
        } else {
            ConversationType::Direct
        };
    let unsupported = if update.message.voice.is_some() {
        "voice"
    } else if update.message.document.is_some() {
        "attachment"
    } else if !update.message.photo.is_empty() {
        "attachment"
    } else {
        ""
    };
    let text = update.message.text.trim().to_string();
    Some(InboundUpdate {
        update_id: update.update_id.to_string(),
        message_id: update.message.message_id.to_string(),
        chat_id: update.message.chat.id.to_string(),
        sender_id: update.message.from.id.to_string(),
        text: text.clone(),
        conversation_type: conversation,
        command: text.starts_with('/'),
        unsupported_surface: unsupported.to_string(),
        received_at: Utc::now(),
        ..InboundUpdate::default()
    })
}

/// Go `NormalizeCommandText`: strips a bot mention from the text and reports
/// mention/command flags.
#[must_use]
pub fn normalize_command_text(text: &str, bot_username: &str) -> (String, bool, bool) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (String::new(), false, false);
    }
    let command = trimmed.starts_with('/');
    let mut mentioned = false;
    let mut result = trimmed.to_string();
    let username = bot_username.trim();
    if !username.is_empty() {
        let stripped = username.strip_prefix('@').unwrap_or(username);
        let at = format!("@{stripped}");
        if result.to_lowercase().contains(&at.to_lowercase()) {
            mentioned = true;
            result = result.replace(&at, "").trim().to_string();
        }
    }
    (result, mentioned, command)
}
/// In-memory fake transport for tests (Go `FakeTransport`).
pub struct FakeTransport {
    handler: Mutex<Option<Arc<dyn Fn(InboundUpdate) + Send + Sync>>>,
    replies: Mutex<Vec<OutboundReply>>,
    route_outcomes: Mutex<Vec<RouteDecision>>,
    fail_send: Mutex<Option<String>>,
}

impl FakeTransport {
    /// Go `NewFakeTransport`.
    #[must_use]
    pub fn new() -> Self {
        FakeTransport {
            handler: Mutex::new(None),
            replies: Mutex::new(Vec::new()),
            route_outcomes: Mutex::new(Vec::new()),
            fail_send: Mutex::new(None),
        }
    }

    /// Go `Emit`: dispatches an update to the registered handler.
    pub fn emit(&self, update: InboundUpdate) {
        if let Some(handler) = self.handler.lock().clone() {
            handler(update);
        }
    }

    /// Replies recorded by [`ReplySender::send_reply`].
    #[must_use]
    pub fn replies(&self) -> Vec<OutboundReply> {
        self.replies.lock().clone()
    }

    /// Go `LastRouteOutcome`.
    #[must_use]
    pub fn last_route_outcome(&self) -> RouteDecision {
        self.route_outcomes
            .lock()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// All recorded route outcomes.
    #[must_use]
    pub fn route_outcomes(&self) -> Vec<RouteDecision> {
        self.route_outcomes.lock().clone()
    }

    /// Makes the next `send_reply` fail with the given error (test helper).
    pub fn set_fail_send(&self, error: impl Into<String>) {
        *self.fail_send.lock() = Some(error.into());
    }
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplySender for FakeTransport {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        if let Some(error) = self.fail_send.lock().clone() {
            return Err(error);
        }
        let mut replies = self.replies.lock();
        replies.push(reply);
        Ok(SentReply {
            external_message_id: format!("telegram_reply_{}", replies.len()),
        })
    }
}

impl Transport for FakeTransport {
    fn start(&self, handle: Arc<dyn Fn(InboundUpdate) + Send + Sync>) -> Result<(), String> {
        *self.handler.lock() = Some(handle);
        Ok(())
    }

    fn reply_capabilities(&self) -> ReplyCapabilities {
        ReplyCapabilities {
            supports_thinking: false,
            supports_streaming: false,
            max_message_length: 4096,
        }
    }

    fn close(&self) -> Result<(), String> {
        Ok(())
    }

    fn record_route_outcome(&self, decision: RouteDecision) {
        self.route_outcomes.lock().push(decision);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::diagnostic_reason_for_error;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::thread::{self, JoinHandle};

    struct HttpRequest {
        path: String,
        body: Vec<u8>,
    }

    /// Minimal single-request-per-connection HTTP server for transport tests.
    struct TestHttpServer {
        addr: SocketAddr,
        shutdown: StdArc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl TestHttpServer {
        fn new<F>(handler: F) -> TestHttpServer
        where
            F: Fn(HttpRequest) -> (u16, Vec<u8>) + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            listener.set_nonblocking(true).expect("nonblocking");
            let addr = listener.local_addr().expect("test server addr");
            let shutdown = StdArc::new(AtomicBool::new(false));
            let flag = StdArc::clone(&shutdown);
            let thread = thread::spawn(move || {
                while !flag.load(AtomicOrdering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // On macOS/BSD the accepted stream inherits the
                            // listener's non-blocking mode; restore blocking
                            // IO so the request read never fails WouldBlock
                            // while the client's bytes are in flight (the
                            // CI-only status-line flake). A read timeout
                            // keeps the test bounded.
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            handle_connection(stream, &handler);
                        }
                        Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            TestHttpServer {
                addr,
                shutdown,
                thread: Some(thread),
            }
        }

        fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for TestHttpServer {
        fn drop(&mut self) {
            self.shutdown.store(true, AtomicOrdering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn handle_connection<F>(mut stream: TcpStream, handler: &F)
    where
        F: Fn(HttpRequest) -> (u16, Vec<u8>),
    {
        let request_line = {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                return;
            }
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((key, value)) = trimmed.split_once(':') {
                    if key.trim().eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 && reader.read_exact(&mut body).is_err() {
                return;
            }
            let parts: Vec<&str> = request_line.split_whitespace().collect();
            HttpRequest {
                path: parts.get(1).copied().unwrap_or("").to_string(),
                body,
            }
        };
        let (status, response_body) = handler(request_line);
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            _ => "Status",
        };
        let header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        let mut response = header.into_bytes();
        response.extend_from_slice(&response_body);
        let _ = stream.write_all(&response);
    }

    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()
    }

    // Go TestBotAPITransportValidatesCredentialAndSendsReply.
    #[test]
    fn bot_api_transport_validates_credential_and_sends_reply() {
        let send_payload = StdArc::new(Mutex::new(None::<Value>));
        let captured = StdArc::clone(&send_payload);
        let server = TestHttpServer::new(move |req| match req.path.as_str() {
            "/bot123:token/getMe" => (
                200,
                serde_json::to_vec(&json!({
                    "ok": true,
                    "result": { "id": 42, "username": "kura_test_bot", "first_name": "Kura Test" },
                }))
                .expect("encode getMe"),
            ),
            "/bot123:token/sendMessage" => {
                *captured.lock() = serde_json::from_slice(&req.body).ok();
                (
                    200,
                    serde_json::to_vec(&json!({ "ok": true, "result": { "message_id": 77 } }))
                        .expect("encode sendMessage"),
                )
            }
            other => panic!("unexpected Telegram Bot API path {other}"),
        });

        let transport = BotApiTransport::new(BotApiTransportConfig {
            connector_id: "telegram-main".to_string(),
            bot_token: "123:token".to_string(),
            base_url: server.base_url(),
            http_client: Some(agent()),
            ..BotApiTransportConfig::default()
        })
        .expect("new transport");

        let binding = transport
            .validate_credential()
            .expect("validate credential");
        assert_eq!(binding.connector_account_id, "telegram_bot_42");
        assert_eq!(binding.provider_account_label, "kura_test_bot");
        assert_eq!(binding.permission_state, PermissionState::Valid);

        let sent = transport
            .send_reply(OutboundReply {
                connector_id: "telegram-main".to_string(),
                channel_id: "chat_1".to_string(),
                content: "hello".to_string(),
                reply_to_external_message_id: "message_1".to_string(),
            })
            .expect("send reply");
        assert_eq!(sent.external_message_id, "77");

        let payload = send_payload.lock().clone().expect("sendMessage payload");
        assert_eq!(payload["chat_id"], "chat_1");
        assert_eq!(payload["text"], "hello");
        assert_eq!(payload["reply_to_message_id"], "message_1");
    }

    // Go TestBotAPITransportClassifiesProviderErrors.
    #[test]
    fn bot_api_transport_classifies_provider_errors() {
        let server = TestHttpServer::new(|_req| {
            (
                401,
                serde_json::to_vec(&json!({ "ok": false, "description": "Unauthorized" }))
                    .expect("encode error"),
            )
        });
        let transport = BotApiTransport::new(BotApiTransportConfig {
            connector_id: "telegram-main".to_string(),
            bot_token: "bad:token".to_string(),
            base_url: server.base_url(),
            http_client: Some(agent()),
            ..BotApiTransportConfig::default()
        })
        .expect("new transport");

        let err = transport
            .validate_credential()
            .expect_err("validation must fail");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            kura_connectors::DiagnosticReasonCode::AuthMissing
        );
    }

    #[test]
    fn new_bot_api_transport_requires_token() {
        match BotApiTransport::new(BotApiTransportConfig::default()) {
            Err(err) => assert_eq!(err, TelegramError::BotTokenRequired),
            Ok(_) => panic!("bot token must be required"),
        }
    }
}
