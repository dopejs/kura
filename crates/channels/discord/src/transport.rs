//! Transport layer (port of transport.go): the transport traits, the
//! classified Discord error type, and the `GatewayTransport` — a REST client
//! (ureq) for send/edit/typing plus an in-memory gateway state used by
//! destination validation, and a WebSocket gateway receive loop (see
//! gateway.rs) that normalizes MESSAGE_CREATE events into inbound messages.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use kura_connectors::{DiagnosticReasonCode, RedactionStatus};
use kura_im::{ReplyProgressor, ReplySender};
use kura_imtypes::{
    InboundMessage, OutboundReply, ReplyCapabilities, ReplyEdit, SentReply, ThinkingSignal,
};
use kura_router::SessionKind;

use crate::config::Config;
use crate::destinations::{DestinationType, DestinationValidation, DestinationValidationState};
use crate::diagnostics::classify_discord_error_message;

/// Discord REST API base for gateway-versioned HTTP calls.
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Discord permission bits referenced by the connector.
pub(crate) const PERMISSION_VIEW_CHANNEL: u64 = 1 << 10;
pub(crate) const PERMISSION_SEND_MESSAGES: u64 = 1 << 11;
pub(crate) const PERMISSION_READ_MESSAGE_HISTORY: u64 = 1 << 16;
pub(crate) const PERMISSION_ADMINISTRATOR: u64 = 1 << 3;
const PERMISSION_ALL: u64 = u64::MAX;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced by the Discord connector. `Classified` carries the stable
/// error class (see `classify_discord_error_message`) like Go's
/// `classifiedDiscordError`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DiscordError {
    #[error("discord bot token is required")]
    BotTokenRequired,
    #[error("create discord session: {0}")]
    CreateSession(String),
    #[error("discord session is not configured")]
    SessionNotConfigured,
    #[error("discord connector id is required")]
    ConnectorIdRequired,
    #[error("discord display name is required")]
    DisplayNameRequired,
    #[error("unsupported discord delivery mode: {0}")]
    UnsupportedDeliveryMode(String),
    #[error("discord connector dependencies are not configured")]
    DependenciesNotConfigured,
    #[error("connector id is required")]
    DiagnosticConnectorRequired,
    #[error("diagnostic reason code is required")]
    DiagnosticReasonRequired,
    #[error("{message}")]
    Classified { class: String, message: String },
    #[error("{0}")]
    Other(String),
}

impl DiscordError {
    /// Go `classifiedDiscordError.ErrorClass()`.
    #[must_use]
    pub fn error_class(&self) -> String {
        match self {
            DiscordError::Classified { class, .. } => class.clone(),
            other => classify_discord_error_message(&other.to_string()),
        }
    }
}

/// Go `wrapDiscordError`: prefixes the source and classifies it.
#[must_use]
pub fn wrap_discord_error(prefix: &str, source: impl std::fmt::Display) -> DiscordError {
    let source_str = source.to_string();
    let class = classify_discord_error_message(&source_str);
    let message = if prefix.is_empty() {
        source_str
    } else {
        format!("{prefix}: {source_str}")
    };
    DiscordError::Classified { class, message }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Go `Transport`: the connector-facing send/start/close surface. Implements
/// `ReplySender` + `ReplyProgressor` so the runtime can pass the transport
/// directly into `MessageLoop::process_single_turn`.
pub trait Transport: ReplySender + ReplyProgressor + Send + Sync {
    /// Go `Start(ctx, handle)`: opens the gateway and routes normalized
    /// inbound messages to `handle`.
    fn start(&self, handle: Arc<dyn Fn(InboundMessage) + Send + Sync>) -> Result<(), DiscordError>;
    /// Go `Close(ctx)`.
    fn close(&self) -> Result<(), DiscordError>;
    /// Go `transport.(DestinationValidator)` type assertion.
    fn destination_validator(&self) -> Option<&dyn DestinationValidator> {
        None
    }
    /// Go `transport.(LifecycleObservableTransport)` type assertion.
    fn lifecycle_observable(&self) -> Option<&dyn LifecycleObservableTransport> {
        None
    }
}

/// Go `DestinationValidator`.
pub trait DestinationValidator: Send + Sync {
    fn validate_destinations(
        &self,
        destinations: Vec<DestinationValidation>,
    ) -> Result<Vec<DestinationValidation>, DiscordError>;
}

/// Go `LifecycleObservableTransport`.
pub trait LifecycleObservableTransport: Send + Sync {
    fn set_lifecycle_observer(
        &self,
        observer: Option<Arc<dyn Fn(TransportLifecycleEvent) + Send + Sync>>,
    );
}

/// Go `TransportLifecycleEvent`. `reason_code` is `Option` to preserve
/// Go's empty-string zero value (no diagnostic persisted for empty reasons).
#[derive(Debug, Clone, Default)]
pub struct TransportLifecycleEvent {
    pub reason_code: Option<DiagnosticReasonCode>,
    pub evidence: HashMap<String, String>,
    pub degraded: bool,
}

// ---------------------------------------------------------------------------
// Gateway message model (discordgo.MessageCreate subset)
// ---------------------------------------------------------------------------

/// Minimal MESSAGE_CREATE payload the connector normalizes (discordgo.Message).
#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayMessage {
    pub(crate) id: String,
    pub(crate) channel_id: String,
    pub(crate) guild_id: String,
    pub(crate) content: String,
    pub(crate) author: Option<GatewayUser>,
    pub(crate) mentions: Vec<String>,
}

/// Minimal author payload (discordgo.User).
#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayUser {
    pub(crate) id: String,
    pub(crate) bot: bool,
}

// ---------------------------------------------------------------------------
// Gateway state (discordgo.State subset for destination validation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayState {
    guilds: HashMap<String, Guild>,
}

impl GatewayState {
    pub(crate) fn guild(&self, id: &str) -> Option<&Guild> {
        self.guilds.get(id)
    }

    pub(crate) fn channel(&self, id: &str) -> Option<&Channel> {
        self.guilds
            .values()
            .find_map(|guild| guild.channels.get(id))
    }

    /// Test/setup helper mirroring discordgo State.GuildAdd.
    #[allow(dead_code)]
    pub(crate) fn guild_add(&mut self, guild: Guild) {
        self.guilds.insert(guild.id.clone(), guild);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Guild {
    pub(crate) id: String,
    pub(crate) owner_id: String,
    pub(crate) roles: Vec<Role>,
    pub(crate) members: HashMap<String, Member>,
    pub(crate) channels: HashMap<String, Channel>,
}

impl Guild {
    fn everyone_role_permissions(&self) -> u64 {
        self.roles
            .iter()
            .find(|role| role.id == self.id)
            .map(|role| role.permissions)
            .unwrap_or(0)
    }

    fn role(&self, id: &str) -> Option<&Role> {
        self.roles.iter().find(|role| role.id == id)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Role {
    pub(crate) id: String,
    pub(crate) permissions: u64,
    #[allow(dead_code)]
    pub(crate) position: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct Member {
    #[allow(dead_code)]
    pub(crate) user_id: String,
    pub(crate) roles: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Channel {
    pub(crate) id: String,
    pub(crate) guild_id: String,
    pub(crate) channel_type: ChannelType,
    pub(crate) permission_overwrites: Vec<Overwrite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ChannelType {
    GuildText,
    Dm,
    GroupDm,
}

#[derive(Debug, Clone)]
pub(crate) struct Overwrite {
    pub(crate) kind: OverwriteKind,
    pub(crate) id: String,
    pub(crate) allow: u64,
    pub(crate) deny: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverwriteKind {
    Role,
    Member,
}

/// discordgo `Guild.MemberPermissions`: @everyone + member roles, then
/// channel role/member overwrites, with Administrator overriding everything.
#[must_use]
fn member_permissions(guild: &Guild, channel: &Channel, user_id: &str) -> u64 {
    if guild.owner_id == user_id {
        return PERMISSION_ALL;
    }
    let Some(member) = guild.members.get(user_id) else {
        return 0;
    };
    let mut permissions = guild.everyone_role_permissions();
    for role_id in &member.roles {
        if let Some(role) = guild.role(role_id) {
            permissions |= role.permissions;
        }
    }
    if permissions & PERMISSION_ADMINISTRATOR != 0 {
        return PERMISSION_ALL;
    }
    for overwrite in &channel.permission_overwrites {
        if overwrite.kind == OverwriteKind::Role
            && (overwrite.id == guild.id || member.roles.contains(&overwrite.id))
        {
            permissions = (permissions & !overwrite.deny) | overwrite.allow;
        }
    }
    for overwrite in &channel.permission_overwrites {
        if overwrite.kind == OverwriteKind::Member && overwrite.id == user_id {
            permissions = (permissions & !overwrite.deny) | overwrite.allow;
        }
    }
    if permissions & PERMISSION_ADMINISTRATOR != 0 {
        return PERMISSION_ALL;
    }
    permissions
}

/// discordgo `State.UserChannelPermissions` mapped onto the local state model.
fn user_channel_permissions(
    state: &GatewayState,
    channel: &Channel,
    user_id: &str,
) -> Result<u64, ()> {
    match channel.channel_type {
        ChannelType::Dm | ChannelType::GroupDm => return Ok(PERMISSION_ALL),
        ChannelType::GuildText => {}
    }
    let guild = state.guild(&channel.guild_id).ok_or(())?;
    Ok(member_permissions(guild, channel, user_id))
}

// ---------------------------------------------------------------------------
// REST client
// ---------------------------------------------------------------------------

/// REST operations the transport performs (discordgo.ChannelMessageSendComplex,
/// ChannelTyping, ChannelMessageEditComplex).
pub(crate) trait DiscordRestClient: Send + Sync {
    fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: &str,
    ) -> Result<String, DiscordError>;
    fn send_typing(&self, channel_id: &str) -> Result<(), DiscordError>;
    fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<(), DiscordError>;
    /// Downcast escape hatch for tests (Go's injectable package vars).
    #[allow(dead_code)]
    fn as_any(&self) -> &dyn std::any::Any;
}

/// ureq-backed REST client following the rs/feishulark/src/client.rs pattern.
pub(crate) struct UreqRestClient {
    base_url: String,
    authorization: String,
}

impl UreqRestClient {
    pub(crate) fn new(token: &str) -> Self {
        UreqRestClient {
            base_url: DISCORD_API_BASE.to_string(),
            authorization: format!("Bot {}", token.trim()),
        }
    }
}

impl DiscordRestClient for UreqRestClient {
    fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: &str,
    ) -> Result<String, DiscordError> {
        let url = format!("{}/channels/{channel_id}/messages", self.base_url);
        let body = serde_json::json!({
            "content": content,
            "message_reference": { "message_id": reply_to, "channel_id": channel_id },
        });
        let response = ureq::post(&url)
            .set("Authorization", &self.authorization)
            .send_json(body);
        match response {
            Ok(resp) => match resp.into_json::<serde_json::Value>() {
                Ok(value) => {
                    let id = value
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if id.is_empty() {
                        Err(wrap_discord_error(
                            "send discord reply",
                            "missing message id in response",
                        ))
                    } else {
                        Ok(id)
                    }
                }
                Err(err) => Err(wrap_discord_error(
                    "send discord reply",
                    format!("decode response: {err}"),
                )),
            },
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(wrap_discord_error(
                    "send discord reply",
                    format!("HTTP {code}: {body}"),
                ))
            }
            Err(ureq::Error::Transport(err)) => Err(wrap_discord_error(
                "send discord reply",
                format!("transport: {err}"),
            )),
        }
    }

    fn send_typing(&self, channel_id: &str) -> Result<(), DiscordError> {
        let url = format!("{}/channels/{channel_id}/typing", self.base_url);
        let response = ureq::post(&url)
            .set("Authorization", &self.authorization)
            .call();
        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(wrap_discord_error(
                    "send discord typing",
                    format!("HTTP {code}: {body}"),
                ))
            }
            Err(ureq::Error::Transport(err)) => Err(wrap_discord_error(
                "send discord typing",
                format!("transport: {err}"),
            )),
        }
    }

    fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<(), DiscordError> {
        let url = format!(
            "{}/channels/{channel_id}/messages/{message_id}",
            self.base_url
        );
        let body = serde_json::json!({ "content": content });
        let response = ureq::patch(&url)
            .set("Authorization", &self.authorization)
            .send_json(body);
        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(wrap_discord_error(
                    "edit discord reply",
                    format!("HTTP {code}: {body}"),
                ))
            }
            Err(ureq::Error::Transport(err)) => Err(wrap_discord_error(
                "edit discord reply",
                format!("transport: {err}"),
            )),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// GatewayTransport
// ---------------------------------------------------------------------------

/// Go `GatewayTransport`. The shared inner state is wrapped in an `Arc` so
/// the gateway thread (see gateway.rs) and the REST surface can share it.
pub struct GatewayTransport {
    inner: Arc<GatewayTransportInner>,
}

impl GatewayTransportInner {
    #[cfg(test)]
    pub(crate) fn rest_sent(&self) -> Vec<(String, String, String)> {
        if let Some(fake) = self.rest.as_any().downcast_ref::<tests::FakeRestClient>() {
            return fake.sent.lock().clone();
        }
        Vec::new()
    }

    #[cfg(test)]
    pub(crate) fn rest_typing(&self) -> Vec<String> {
        if let Some(fake) = self.rest.as_any().downcast_ref::<tests::FakeRestClient>() {
            return fake.typing.lock().clone();
        }
        Vec::new()
    }

    #[cfg(test)]
    pub(crate) fn rest_edited(&self) -> Vec<(String, String, String)> {
        if let Some(fake) = self.rest.as_any().downcast_ref::<tests::FakeRestClient>() {
            return fake.edited.lock().clone();
        }
        Vec::new()
    }
}

pub(crate) struct GatewayTransportInner {
    pub(crate) cfg: Config,
    pub(crate) state: parking_lot::Mutex<GatewayState>,
    pub(crate) bot_user_id: parking_lot::Mutex<String>,
    pub(crate) lifecycle:
        parking_lot::Mutex<Option<Arc<dyn Fn(TransportLifecycleEvent) + Send + Sync>>>,
    pub(crate) closed: AtomicBool,
    pub(crate) rest: Arc<dyn DiscordRestClient>,
}

impl GatewayTransport {
    /// Go `NewGatewayTransport`: validates the bot token and builds the
    /// session configuration (Identify intents are applied in gateway.rs).
    pub fn new(cfg: Config) -> Result<Self, DiscordError> {
        let token = cfg.bot_token.trim().to_string();
        if token.is_empty() {
            return Err(DiscordError::BotTokenRequired);
        }
        Ok(GatewayTransport {
            inner: Arc::new(GatewayTransportInner {
                cfg,
                state: parking_lot::Mutex::new(GatewayState::default()),
                bot_user_id: parking_lot::Mutex::new(String::new()),
                lifecycle: parking_lot::Mutex::new(None),
                closed: AtomicBool::new(false),
                rest: Arc::new(UreqRestClient::new(&token)),
            }),
        })
    }

    /// The bot user id recorded from the gateway READY payload.
    #[must_use]
    pub fn current_bot_user_id(&self) -> String {
        self.inner.current_bot_user_id()
    }

    /// Go `SendReply` returning the classified error.
    pub fn send_reply_classified(&self, reply: OutboundReply) -> Result<SentReply, DiscordError> {
        let message_id = self.inner.rest.send_message(
            &reply.channel_id,
            &reply.content,
            &reply.reply_to_external_message_id,
        )?;
        Ok(SentReply {
            external_message_id: message_id,
        })
    }

    /// Go `SendThinking` returning the classified error.
    pub fn send_thinking_classified(&self, signal: ThinkingSignal) -> Result<(), DiscordError> {
        self.inner.rest.send_typing(&signal.channel_id)
    }

    /// Go `EditReply` returning the classified error.
    pub fn edit_reply_classified(&self, edit: ReplyEdit) -> Result<(), DiscordError> {
        self.inner
            .rest
            .edit_message(&edit.channel_id, &edit.external_message_id, &edit.content)
    }

    /// Go `ReplyCapabilities`.
    #[must_use]
    pub fn reply_capabilities(&self) -> ReplyCapabilities {
        ReplyCapabilities {
            supports_thinking: true,
            supports_streaming: true,
            max_message_length: 2000,
        }
    }
}

impl GatewayTransportInner {
    #[must_use]
    pub(crate) fn current_bot_user_id(&self) -> String {
        self.bot_user_id.lock().clone()
    }

    /// Go `normalizeMessage`: skips bot/empty messages, strips the bot
    /// mention in guild channels, and derives the session kind and identity
    /// fields.
    pub(crate) fn normalize_message(&self, message: &GatewayMessage) -> Option<InboundMessage> {
        let author = message.author.as_ref()?;
        if author.bot {
            return None;
        }
        let mut content = message.content.trim().to_string();
        if content.is_empty() {
            return None;
        }
        let bot_user_id = self.current_bot_user_id();
        let mentioned = mentioned_user(&message.mentions, &bot_user_id);
        let direct = message.guild_id.trim().is_empty();
        if !direct && !bot_user_id.is_empty() {
            content = strip_bot_mention(&content, &bot_user_id);
            content = content.trim().to_string();
            if content.is_empty() {
                return None;
            }
        }
        let (kind, thread_id, peer_id) = if direct {
            (SessionKind::Direct, String::new(), author.id.clone())
        } else {
            (
                SessionKind::Group,
                message.channel_id.clone(),
                message.channel_id.clone(),
            )
        };
        Some(InboundMessage {
            connector_id: self.cfg.connector_id.clone(),
            connector_kind: "discord".to_string(),
            external_message_id: message.id.clone(),
            account_id: bot_user_id.clone(),
            connector_account_id: bot_user_id.clone(),
            channel_or_conversation_id: message.channel_id.clone(),
            provider_message_id: message.id.clone(),
            equivalent_rule_id: "discord_message_id".to_string(),
            channel_id: message.channel_id.clone(),
            guild_id: message.guild_id.clone(),
            peer_id,
            thread_id,
            author_id: author.id.clone(),
            content,
            kind,
            direct,
            mentioned: direct || mentioned,
            received_at: Utc::now(),
            ..InboundMessage::default()
        })
    }

    /// Go `emitLifecycle`.
    pub(crate) fn emit_lifecycle(&self, event: TransportLifecycleEvent) {
        let observer = self.lifecycle.lock().clone();
        if let Some(observer) = observer {
            observer(event);
        }
    }
}

impl ReplySender for GatewayTransport {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.send_reply_classified(reply)
            .map_err(|err| err.to_string())
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        Some(self)
    }
}

impl ReplyProgressor for GatewayTransport {
    fn reply_capabilities(&self) -> ReplyCapabilities {
        GatewayTransport::reply_capabilities(self)
    }

    fn send_thinking(&self, signal: ThinkingSignal) -> Result<(), String> {
        self.send_thinking_classified(signal)
            .map_err(|err| err.to_string())
    }

    fn edit_reply(&self, edit: ReplyEdit) -> Result<(), String> {
        self.edit_reply_classified(edit)
            .map_err(|err| err.to_string())
    }
}

impl Transport for GatewayTransport {
    fn start(&self, handle: Arc<dyn Fn(InboundMessage) + Send + Sync>) -> Result<(), DiscordError> {
        crate::gateway::spawn_gateway(self.inner.clone(), handle)
    }

    fn close(&self) -> Result<(), DiscordError> {
        self.inner.closed.store(true, Ordering::SeqCst);
        *self.inner.lifecycle.lock() = None;
        Ok(())
    }

    fn destination_validator(&self) -> Option<&dyn DestinationValidator> {
        Some(self)
    }

    fn lifecycle_observable(&self) -> Option<&dyn LifecycleObservableTransport> {
        Some(self)
    }
}

impl DestinationValidator for GatewayTransport {
    fn validate_destinations(
        &self,
        destinations: Vec<DestinationValidation>,
    ) -> Result<Vec<DestinationValidation>, DiscordError> {
        let now = Utc::now();
        let state = self.inner.state.lock();
        let bot_user_id = self.inner.current_bot_user_id();
        let mut validated = Vec::with_capacity(destinations.len());
        for mut destination in destinations {
            destination.validated_at = now;
            destination.redaction_status = RedactionStatus::Redacted;
            destination.safe_evidence =
                HashMap::from([("source".to_string(), "gateway_state".to_string())]);
            match destination.destination_type {
                DestinationType::Guild => {
                    if state.guild(&destination.destination_id).is_some() {
                        destination.validation_state = DestinationValidationState::Valid;
                        destination.reason_code = "healthy".to_string();
                        destination.provider_label =
                            redacted_discord_label(&destination.destination_id);
                    } else {
                        destination.validation_state = DestinationValidationState::BotNotMember;
                        destination.reason_code = "bot_not_member".to_string();
                    }
                }
                DestinationType::Channel => {
                    if let Some(channel) = state.channel(&destination.destination_id) {
                        match user_channel_permissions(&state, channel, &bot_user_id) {
                            Ok(permissions) => {
                                let missing = missing_discord_channel_permissions(permissions);
                                if !missing.is_empty() {
                                    destination.validation_state =
                                        DestinationValidationState::MissingPermission;
                                    destination.reason_code = "permission_missing".to_string();
                                    destination.provider_label =
                                        redacted_discord_label(&channel.id);
                                    destination
                                        .safe_evidence
                                        .insert("missingPermissions".to_string(), missing);
                                } else {
                                    destination.validation_state =
                                        DestinationValidationState::Valid;
                                    destination.reason_code = "healthy".to_string();
                                    destination.provider_label =
                                        redacted_discord_label(&channel.id);
                                    destination.safe_evidence.insert(
                                        "permissionCheck".to_string(),
                                        "send_read".to_string(),
                                    );
                                }
                            }
                            Err(()) => {
                                destination.validation_state =
                                    DestinationValidationState::MissingPermission;
                                destination.reason_code = "permission_missing".to_string();
                                destination.provider_label = redacted_discord_label(&channel.id);
                                destination
                                    .safe_evidence
                                    .insert("permissionCheck".to_string(), "failed".to_string());
                                destination.safe_evidence.insert(
                                    "errorClass".to_string(),
                                    classify_discord_error_message(
                                        "discord gateway state: guild not found",
                                    ),
                                );
                            }
                        }
                    } else {
                        destination.validation_state = DestinationValidationState::NotFound;
                        destination.reason_code = "not_found".to_string();
                    }
                }
                DestinationType::DirectMessage => {
                    destination.validation_state = DestinationValidationState::Invalid;
                    destination.reason_code = "unsupported_destination".to_string();
                }
            }
            validated.push(destination);
        }
        Ok(validated)
    }
}

impl LifecycleObservableTransport for GatewayTransport {
    fn set_lifecycle_observer(
        &self,
        observer: Option<Arc<dyn Fn(TransportLifecycleEvent) + Send + Sync>>,
    ) {
        *self.inner.lifecycle.lock() = observer;
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (transport.go)
// ---------------------------------------------------------------------------

/// Go `mentionedUser`: whether any mention matches the bot user id.
#[must_use]
pub fn mentioned_user(mentions: &[String], user_id: &str) -> bool {
    if user_id.is_empty() {
        return false;
    }
    mentions.iter().any(|mention| mention == user_id)
}

/// Go `stripBotMention`: removes <@id> and <@!id> forms and trims.
#[must_use]
pub fn strip_bot_mention(content: &str, user_id: &str) -> String {
    content
        .replace(&format!("<@{user_id}>"), "")
        .replace(&format!("<@!{user_id}>"), "")
        .trim()
        .to_string()
}

/// Go `redactDiscordRoute`: replaces route segments that look like Discord
/// snowflake ids with `redacted_id`.
#[must_use]
pub fn redact_discord_route(route: &str) -> String {
    let route = route.trim();
    if route.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = route.split('/').collect();
    let redacted: Vec<String> = parts
        .iter()
        .map(|part| {
            if looks_like_discord_id(part) {
                "redacted_id".to_string()
            } else {
                (*part).to_string()
            }
        })
        .collect();
    redacted.join("/")
}

/// Go `redactedDiscordLabel`.
#[must_use]
pub fn redacted_discord_label(id: &str) -> String {
    if id.trim().is_empty() {
        String::new()
    } else {
        "discord_resource_redacted".to_string()
    }
}

/// Go `looksLikeDiscordID`: at least 12 characters, all digits.
#[must_use]
pub fn looks_like_discord_id(value: &str) -> bool {
    if value.chars().count() < 12 {
        return false;
    }
    value.chars().all(|ch| ch.is_ascii_digit())
}

/// Go `missingDiscordChannelPermissions`: comma-joined names of the missing
/// view/send/read-history bits.
#[must_use]
pub fn missing_discord_channel_permissions(permissions: u64) -> String {
    let required = [
        ("view_channel", PERMISSION_VIEW_CHANNEL),
        ("send_messages", PERMISSION_SEND_MESSAGES),
        ("read_message_history", PERMISSION_READ_MESSAGE_HISTORY),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|(_, bit)| permissions & bit == 0)
        .map(|(name, _)| *name)
        .collect();
    missing.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::diagnostic_reason_for_error;

    fn transport_with(
        cfg: Config,
        state: GatewayState,
        bot_user_id: &str,
        rest: Arc<dyn DiscordRestClient>,
    ) -> GatewayTransport {
        GatewayTransport {
            inner: Arc::new(GatewayTransportInner {
                cfg,
                state: parking_lot::Mutex::new(state),
                bot_user_id: parking_lot::Mutex::new(bot_user_id.to_string()),
                lifecycle: parking_lot::Mutex::new(None),
                closed: AtomicBool::new(false),
                rest,
            }),
        }
    }

    fn gateway_transport(cfg: Config) -> GatewayTransport {
        transport_with(
            cfg,
            GatewayState::default(),
            "bot_1",
            Arc::new(FakeRestClient::default()),
        )
    }

    #[derive(Default)]
    pub(crate) struct FakeRestClient {
        pub(crate) sent: parking_lot::Mutex<Vec<(String, String, String)>>,
        pub(crate) typing: parking_lot::Mutex<Vec<String>>,
        pub(crate) edited: parking_lot::Mutex<Vec<(String, String, String)>>,
        pub(crate) send_err: Option<String>,
    }

    impl DiscordRestClient for FakeRestClient {
        fn send_message(
            &self,
            channel_id: &str,
            content: &str,
            reply_to: &str,
        ) -> Result<String, DiscordError> {
            if let Some(err) = &self.send_err {
                return Err(wrap_discord_error("send discord reply", err));
            }
            self.sent.lock().push((
                channel_id.to_string(),
                content.to_string(),
                reply_to.to_string(),
            ));
            Ok("reply_1".to_string())
        }

        fn send_typing(&self, channel_id: &str) -> Result<(), DiscordError> {
            self.typing.lock().push(channel_id.to_string());
            Ok(())
        }

        fn edit_message(
            &self,
            channel_id: &str,
            message_id: &str,
            content: &str,
        ) -> Result<(), DiscordError> {
            self.edited.lock().push((
                channel_id.to_string(),
                message_id.to_string(),
                content.to_string(),
            ));
            Ok(())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn direct_message(id: &str, channel_id: &str, content: &str) -> GatewayMessage {
        GatewayMessage {
            id: id.to_string(),
            channel_id: channel_id.to_string(),
            guild_id: String::new(),
            content: content.to_string(),
            author: Some(GatewayUser {
                id: "user_1".to_string(),
                bot: false,
            }),
            mentions: Vec::new(),
        }
    }

    // Go TestGatewayTransportNormalizeDirectMessage
    #[test]
    fn normalize_direct_message() {
        let transport = gateway_transport(Config {
            connector_id: "discord-main".to_string(),
            ..Config::default()
        });
        let inbound = transport
            .inner
            .normalize_message(&direct_message("msg_1", "dm_1", "hello from dm"))
            .expect("direct message normalized");
        assert_eq!(inbound.kind, SessionKind::Direct);
        assert!(inbound.direct);
        assert!(inbound.mentioned);
        assert_eq!(inbound.content, "hello from dm");
        assert_eq!(inbound.connector_account_id, "bot_1");
        assert_eq!(inbound.channel_or_conversation_id, "dm_1");
        assert_eq!(inbound.provider_message_id, "msg_1");
    }

    // Go TestGatewayTransportNormalizeGuildMentionStripsBotMention
    #[test]
    fn normalize_guild_mention_strips_bot_mention() {
        let transport = gateway_transport(Config {
            connector_id: "discord-main".to_string(),
            ..Config::default()
        });
        let message = GatewayMessage {
            id: "msg_2".to_string(),
            channel_id: "channel_1".to_string(),
            guild_id: "guild_1".to_string(),
            content: "<@bot_1> hello guild".to_string(),
            author: Some(GatewayUser {
                id: "user_1".to_string(),
                bot: false,
            }),
            mentions: vec!["bot_1".to_string()],
        };
        let inbound = transport
            .inner
            .normalize_message(&message)
            .expect("guild message normalized");
        assert_eq!(inbound.kind, SessionKind::Group);
        assert!(!inbound.direct);
        assert!(inbound.mentioned);
        assert_eq!(inbound.content, "hello guild");
        assert_eq!(inbound.peer_id, "channel_1");
        assert_eq!(inbound.thread_id, "channel_1");
        assert_eq!(inbound.connector_account_id, "bot_1");
        assert_eq!(inbound.channel_or_conversation_id, "channel_1");
        assert_eq!(inbound.provider_message_id, "msg_2");
        assert_eq!(inbound.equivalent_rule_id, "discord_message_id");
    }

    #[test]
    fn normalize_skips_bots_and_empty_content() {
        let transport = gateway_transport(Config {
            connector_id: "discord-main".to_string(),
            ..Config::default()
        });
        let bot_message = GatewayMessage {
            author: Some(GatewayUser {
                id: "bot_other".to_string(),
                bot: true,
            }),
            ..direct_message("msg_x", "dm_1", "hello")
        };
        assert!(transport.inner.normalize_message(&bot_message).is_none());
        assert!(
            transport
                .inner
                .normalize_message(&direct_message("msg_y", "dm_1", "   "))
                .is_none()
        );
        let mention_only = GatewayMessage {
            guild_id: "guild_1".to_string(),
            channel_id: "channel_1".to_string(),
            content: "<@bot_1>".to_string(),
            mentions: vec!["bot_1".to_string()],
            ..direct_message("msg_z", "channel_1", "")
        };
        assert!(transport.inner.normalize_message(&mention_only).is_none());
    }

    // Go TestGatewayTransportSendReplyShapesDiscordRequest
    #[test]
    fn send_reply_shapes_discord_request() {
        let rest = FakeRestClient::default();
        let transport = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            GatewayState::default(),
            "bot_1",
            Arc::new(rest),
        );
        let reply = transport
            .send_reply_classified(OutboundReply {
                connector_id: "discord-main".to_string(),
                channel_id: "channel_1".to_string(),
                content: "assistant reply".to_string(),
                reply_to_external_message_id: "msg_1".to_string(),
            })
            .expect("send reply");
        assert_eq!(reply.external_message_id, "reply_1");
        let sent = transport.inner.rest_sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0],
            (
                "channel_1".to_string(),
                "assistant reply".to_string(),
                "msg_1".to_string()
            )
        );
    }

    // Go TestGatewayTransportWrapsAuthFailure
    #[test]
    fn wraps_auth_failure() {
        let rest = FakeRestClient {
            send_err: Some("401 Unauthorized: invalid token".to_string()),
            ..FakeRestClient::default()
        };
        let transport = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            GatewayState::default(),
            "bot_1",
            Arc::new(rest),
        );
        let err = transport
            .send_reply_classified(OutboundReply {
                connector_id: "discord-main".to_string(),
                channel_id: "channel_1".to_string(),
                content: "assistant reply".to_string(),
                reply_to_external_message_id: "msg_1".to_string(),
            })
            .expect_err("expected auth failure");
        assert_eq!(err.error_class(), "auth_error");
        assert_eq!(
            diagnostic_reason_for_error(&err),
            DiagnosticReasonCode::AuthMissing
        );
    }

    // Go TestGatewayTransportSendThinkingUsesChannelTyping
    #[test]
    fn send_thinking_uses_channel_typing() {
        let rest = FakeRestClient::default();
        let transport = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            GatewayState::default(),
            "bot_1",
            Arc::new(rest),
        );
        transport
            .send_thinking_classified(ThinkingSignal {
                connector_id: "discord-main".to_string(),
                channel_id: "channel_1".to_string(),
            })
            .expect("send thinking");
        let typing = transport.inner.rest_typing();
        assert_eq!(typing, vec!["channel_1".to_string()]);
    }

    // Go TestGatewayTransportEditReplyShapesDiscordRequest
    #[test]
    fn edit_reply_shapes_discord_request() {
        let rest = FakeRestClient::default();
        let transport = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            GatewayState::default(),
            "bot_1",
            Arc::new(rest),
        );
        transport
            .edit_reply_classified(ReplyEdit {
                connector_id: "discord-main".to_string(),
                channel_id: "channel_1".to_string(),
                external_message_id: "reply_1".to_string(),
                content: "updated reply".to_string(),
            })
            .expect("edit reply");
        let edited = transport.inner.rest_edited();
        assert_eq!(edited.len(), 1);
        assert_eq!(
            edited[0],
            (
                "channel_1".to_string(),
                "reply_1".to_string(),
                "updated reply".to_string()
            )
        );
    }

    // Go TestGatewayTransportValidateDestinationsRequiresChannelPermissions
    #[test]
    fn validate_destinations_requires_channel_permissions() {
        let mut state = GatewayState::default();
        add_guild_with_bot_channel_permissions(
            &mut state,
            "guild_1",
            "channel_1",
            PERMISSION_VIEW_CHANNEL | PERMISSION_SEND_MESSAGES | PERMISSION_READ_MESSAGE_HISTORY,
        );
        let valid = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            state,
            "bot_1",
            Arc::new(FakeRestClient::default()),
        );
        let validated = valid
            .validate_destinations(vec![DestinationValidation {
                connector_id: "discord-main".to_string(),
                destination_id: "channel_1".to_string(),
                destination_type: DestinationType::Channel,
                selected: true,
                ..DestinationValidation::default()
            }])
            .expect("validate valid channel");
        assert_eq!(validated.len(), 1);
        assert_eq!(
            validated[0].validation_state,
            DestinationValidationState::Valid
        );

        let mut blocked_state = GatewayState::default();
        add_guild_with_bot_channel_permissions(
            &mut blocked_state,
            "guild_1",
            "channel_1",
            PERMISSION_VIEW_CHANNEL,
        );
        let blocked = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            blocked_state,
            "bot_1",
            Arc::new(FakeRestClient::default()),
        );
        let degraded = blocked
            .validate_destinations(vec![DestinationValidation {
                connector_id: "discord-main".to_string(),
                destination_id: "channel_1".to_string(),
                destination_type: DestinationType::Channel,
                selected: true,
                ..DestinationValidation::default()
            }])
            .expect("validate blocked channel");
        assert_eq!(degraded.len(), 1);
        assert_eq!(
            degraded[0].validation_state,
            DestinationValidationState::MissingPermission
        );
        assert_eq!(degraded[0].reason_code, "permission_missing");
        assert_eq!(
            degraded[0]
                .safe_evidence
                .get("missingPermissions")
                .map(String::as_str),
            Some("send_messages,read_message_history")
        );
    }

    #[test]
    fn validate_destinations_guild_and_not_found() {
        let mut state = GatewayState::default();
        add_guild_with_bot_channel_permissions(&mut state, "guild_1", "channel_1", 0);
        let transport = transport_with(
            Config {
                connector_id: "discord-main".to_string(),
                ..Config::default()
            },
            state,
            "bot_1",
            Arc::new(FakeRestClient::default()),
        );
        let validated = transport
            .validate_destinations(vec![
                DestinationValidation {
                    destination_id: "guild_1".to_string(),
                    destination_type: DestinationType::Guild,
                    selected: true,
                    ..DestinationValidation::default()
                },
                DestinationValidation {
                    destination_id: "missing_channel".to_string(),
                    destination_type: DestinationType::Channel,
                    selected: true,
                    ..DestinationValidation::default()
                },
                DestinationValidation {
                    destination_id: "some_guild".to_string(),
                    destination_type: DestinationType::Guild,
                    selected: true,
                    ..DestinationValidation::default()
                },
            ])
            .expect("validate");
        assert_eq!(
            validated[0].validation_state,
            DestinationValidationState::Valid
        );
        assert_eq!(validated[0].provider_label, "discord_resource_redacted");
        assert_eq!(
            validated[1].validation_state,
            DestinationValidationState::NotFound
        );
        assert_eq!(validated[1].reason_code, "not_found");
        assert_eq!(
            validated[2].validation_state,
            DestinationValidationState::BotNotMember
        );
        assert_eq!(validated[2].reason_code, "bot_not_member");
    }

    // Go TestGatewayTransportLifecycleObserverRecordsGatewayAndRateLimitEvidence
    #[test]
    fn lifecycle_observer_records_gateway_and_rate_limit_evidence() {
        let transport = gateway_transport(Config {
            connector_id: "discord-main".to_string(),
            ..Config::default()
        });
        let events: Arc<parking_lot::Mutex<Vec<TransportLifecycleEvent>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        transport.inner.lifecycle.lock().replace(Arc::new(
            move |event: TransportLifecycleEvent| {
                recorded.lock().push(event);
            },
        ));
        transport.inner.emit_lifecycle(TransportLifecycleEvent {
            reason_code: Some(DiagnosticReasonCode::NetworkFailed),
            evidence: HashMap::from([("stage".to_string(), "gateway_disconnect".to_string())]),
            degraded: true,
        });
        transport.inner.emit_lifecycle(TransportLifecycleEvent {
            reason_code: Some(DiagnosticReasonCode::NetworkFailed),
            evidence: HashMap::from([("stage".to_string(), "gateway_resumed".to_string())]),
            degraded: false,
        });
        transport.inner.emit_lifecycle(TransportLifecycleEvent {
            reason_code: Some(DiagnosticReasonCode::RateLimited),
            evidence: HashMap::from([
                ("stage".to_string(), "rate_limit".to_string()),
                ("bucket".to_string(), "messages".to_string()),
                ("retryAfter".to_string(), "5s".to_string()),
            ]),
            degraded: true,
        });
        let events = events.lock().clone();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0].reason_code,
            Some(DiagnosticReasonCode::NetworkFailed)
        );
        assert_eq!(
            events[0].evidence.get("stage").map(String::as_str),
            Some("gateway_disconnect")
        );
        assert!(events[0].degraded);
        assert_eq!(
            events[1].reason_code,
            Some(DiagnosticReasonCode::NetworkFailed)
        );
        assert_eq!(
            events[1].evidence.get("stage").map(String::as_str),
            Some("gateway_resumed")
        );
        assert!(!events[1].degraded);
        assert_eq!(
            events[2].reason_code,
            Some(DiagnosticReasonCode::RateLimited)
        );
        assert!(
            !events[2]
                .evidence
                .get("retryAfter")
                .unwrap_or(&String::new())
                .is_empty()
        );
        assert!(events[2].degraded);
    }

    // Redaction pure helpers (transport.go)
    #[test]
    fn redaction_helpers() {
        assert_eq!(redact_discord_route(""), "");
        assert_eq!(
            redact_discord_route("/channels/123456789012345678/messages/987654321098765432"),
            "/channels/redacted_id/messages/redacted_id"
        );
        assert_eq!(
            redact_discord_route("/guilds/guild_name"),
            "/guilds/guild_name"
        );
        assert!(looks_like_discord_id("123456789012345678"));
        assert!(!looks_like_discord_id("short"));
        assert!(!looks_like_discord_id("1234567890ab"));
        assert_eq!(
            redacted_discord_label("123456789012345678"),
            "discord_resource_redacted"
        );
        assert_eq!(redacted_discord_label("  "), "");
        assert_eq!(
            missing_discord_channel_permissions(0),
            "view_channel,send_messages,read_message_history"
        );
        assert_eq!(
            missing_discord_channel_permissions(PERMISSION_VIEW_CHANNEL),
            "send_messages,read_message_history"
        );
        assert_eq!(
            missing_discord_channel_permissions(
                PERMISSION_VIEW_CHANNEL
                    | PERMISSION_SEND_MESSAGES
                    | PERMISSION_READ_MESSAGE_HISTORY
            ),
            ""
        );
        assert_eq!(strip_bot_mention("<@bot_1> hello", "bot_1"), "hello");
        assert_eq!(strip_bot_mention("<@!bot_1> hello", "bot_1"), "hello");
        assert_eq!(
            strip_bot_mention("<@other> hello", "bot_1"),
            "<@other> hello"
        );
        assert!(mentioned_user(&["bot_1".to_string()], "bot_1"));
        assert!(!mentioned_user(&["bot_2".to_string()], "bot_1"));
        assert!(!mentioned_user(&["bot_1".to_string()], ""));
    }

    fn add_guild_with_bot_channel_permissions(
        state: &mut GatewayState,
        guild_id: &str,
        channel_id: &str,
        permissions: u64,
    ) {
        state.guild_add(Guild {
            id: guild_id.to_string(),
            owner_id: String::new(),
            roles: vec![
                Role {
                    id: guild_id.to_string(),
                    permissions: 0,
                    position: -1,
                },
                Role {
                    id: "role_bot".to_string(),
                    permissions,
                    position: 0,
                },
            ],
            members: HashMap::from([(
                "bot_1".to_string(),
                Member {
                    user_id: "bot_1".to_string(),
                    roles: vec!["role_bot".to_string()],
                },
            )]),
            channels: HashMap::from([(
                channel_id.to_string(),
                Channel {
                    id: channel_id.to_string(),
                    guild_id: guild_id.to_string(),
                    channel_type: ChannelType::GuildText,
                    permission_overwrites: Vec::new(),
                },
            )]),
        });
    }
}
