//! Discord gateway receive loop (port of transport.go's discordgo event
//! handlers): a tokio-tungstenite WebSocket client speaking the gateway v10
//! protocol — Identify on connect, opcode-1 heartbeats, READY bookkeeping,
//! and MESSAGE_CREATE normalization into the inbound handler. The loop runs
//! on a dedicated std::thread with a current-thread Tokio runtime (the
//! bridge_runtime pattern from rs/chat/src/service.rs).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use kura_connectors::DiagnosticReasonCode;
use kura_imtypes::InboundMessage;

use crate::DiscordError;
use crate::transport::{GatewayMessage, GatewayTransportInner, TransportLifecycleEvent};

pub(crate) const GATEWAY_URL: &str = "wss://gateway.discord.gg";
pub(crate) const GATEWAY_VERSION: &str = "10";
/// discordgo intents: GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT.
pub(crate) const GATEWAY_INTENTS: u64 = (1 << 9) | (1 << 12) | (1 << 15);

const OP_DISPATCH: u64 = 0;
const OP_HEARTBEAT: u64 = 1;
const OP_IDENTIFY: u64 = 2;
const OP_RECONNECT: u64 = 7;
const OP_HELLO: u64 = 10;
const OP_HEARTBEAT_ACK: u64 = 11;

const MAX_BACKOFF_MS: u64 = 30_000;

/// Spawns the gateway loop on a dedicated std::thread running a current-thread
/// Tokio runtime.
pub(crate) fn spawn_gateway(
    inner: Arc<GatewayTransportInner>,
    handle: Arc<dyn Fn(InboundMessage) + Send + Sync>,
) -> Result<(), DiscordError> {
    std::thread::Builder::new()
        .name("kura-discord-gateway".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(rt) => rt.block_on(run_gateway(inner, handle)),
                Err(err) => {
                    inner.emit_lifecycle(TransportLifecycleEvent {
                        reason_code: Some(DiagnosticReasonCode::NetworkFailed),
                        evidence: [("stage".to_string(), "gateway_runtime".to_string())].into(),
                        degraded: true,
                    });
                    let _ = err;
                }
            }
        })
        .map_err(|err| DiscordError::Other(format!("spawn gateway thread: {err}")))?;
    Ok(())
}

async fn run_gateway(
    inner: Arc<GatewayTransportInner>,
    handle: Arc<dyn Fn(InboundMessage) + Send + Sync>,
) {
    let mut backoff_ms: u64 = 1_000;
    while !inner.closed.load(Ordering::SeqCst) {
        match connect_and_listen(&inner, &handle).await {
            Ok(()) => return,
            Err(()) => {
                if inner.closed.load(Ordering::SeqCst) {
                    return;
                }
                inner.emit_lifecycle(TransportLifecycleEvent {
                    reason_code: Some(DiagnosticReasonCode::NetworkFailed),
                    evidence: [("stage".to_string(), "gateway_disconnect".to_string())].into(),
                    degraded: true,
                });
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }
}

/// One connected session: Identify, heartbeat, dispatch handling, and
/// reconnect-on-demand. Ok(()) means a clean stop (closed flag); Err(())
/// means the session dropped and should be re-established.
async fn connect_and_listen(
    inner: &Arc<GatewayTransportInner>,
    handle: &Arc<dyn Fn(InboundMessage) + Send + Sync>,
) -> Result<(), ()> {
    let url = format!("{GATEWAY_URL}/?v={GATEWAY_VERSION}&encoding=json");
    let (mut ws, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|_| ())?;

    let identify = serde_json::json!({
        "op": OP_IDENTIFY,
        "d": {
            "token": format!("Bot {}", inner.cfg.bot_token.trim()),
            "intents": GATEWAY_INTENTS,
            "properties": {
                "os": "linux",
                "browser": "kura-agent",
                "device": "kura-agent",
            },
        },
    });
    ws.send(Message::Text(identify.to_string().into()))
        .await
        .map_err(|_| ())?;

    let mut sequence: Option<u64> = None;
    let mut heartbeat_interval: Option<Duration> = None;
    let mut last_heartbeat = Instant::now();

    loop {
        if inner.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(interval) = heartbeat_interval {
            if last_heartbeat.elapsed() >= interval {
                let heartbeat = serde_json::json!({ "op": OP_HEARTBEAT, "d": sequence });
                if ws
                    .send(Message::Text(heartbeat.to_string().into()))
                    .await
                    .is_err()
                {
                    return Err(());
                }
                last_heartbeat = Instant::now();
            }
        }
        let tick = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
        match tick {
            Err(_) => {}
            Ok(None) | Ok(Some(Err(_))) => return Err(()),
            Ok(Some(Ok(Message::Close(_)))) => return Err(()),
            Ok(Some(Ok(Message::Text(text)))) => {
                match handle_payload(inner, handle, &text, &mut sequence, &mut heartbeat_interval) {
                    PayloadAction::Continue => {}
                    PayloadAction::Reconnect => return Err(()),
                }
            }
            _ => {}
        }
    }
}

enum PayloadAction {
    Continue,
    Reconnect,
}

/// Handles one gateway payload (dispatch/heartbeat/hello/reconnect). READY
/// records the bot user id; MESSAGE_CREATE normalizes and forwards to the
/// inbound handler.
fn handle_payload(
    inner: &GatewayTransportInner,
    handle: &Arc<dyn Fn(InboundMessage) + Send + Sync>,
    text: &str,
    sequence: &mut Option<u64>,
    heartbeat_interval: &mut Option<Duration>,
) -> PayloadAction {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        return PayloadAction::Continue;
    };
    let op = payload.get("op").and_then(Value::as_u64).unwrap_or(0);
    match op {
        OP_DISPATCH => {
            if let Some(s) = payload.get("s").and_then(Value::as_u64) {
                *sequence = Some(s);
            }
            let event = payload.get("t").and_then(Value::as_str).unwrap_or("");
            match event {
                "READY" => {
                    if let Some(user_id) = payload
                        .get("d")
                        .and_then(|d| d.get("user"))
                        .and_then(|user| user.get("id"))
                        .and_then(Value::as_str)
                    {
                        *inner.bot_user_id.lock() = user_id.to_string();
                    }
                }
                "MESSAGE_CREATE" => {
                    if let Some(message) = parse_gateway_message(payload.get("d")) {
                        if let Some(inbound) = inner.normalize_message(&message) {
                            handle(inbound);
                        }
                    }
                }
                _ => {}
            }
            PayloadAction::Continue
        }
        OP_RECONNECT => PayloadAction::Reconnect,
        OP_HELLO => {
            if let Some(ms) = payload
                .get("d")
                .and_then(|d| d.get("heartbeat_interval"))
                .and_then(Value::as_u64)
            {
                *heartbeat_interval = Some(Duration::from_millis(ms));
            }
            PayloadAction::Continue
        }
        OP_HEARTBEAT_ACK => PayloadAction::Continue,
        _ => PayloadAction::Continue,
    }
}

/// Parses a MESSAGE_CREATE payload's `d` object into the connector's message
/// model.
fn parse_gateway_message(data: Option<&Value>) -> Option<GatewayMessage> {
    let d = data?;
    let author = d.get("author")?;
    Some(GatewayMessage {
        id: d.get("id")?.as_str()?.to_string(),
        channel_id: d
            .get("channel_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        guild_id: d
            .get("guild_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content: d
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        author: Some(crate::transport::GatewayUser {
            id: author.get("id")?.as_str()?.to_string(),
            bot: author.get("bot").and_then(Value::as_bool).unwrap_or(false),
        }),
        mentions: d
            .get("mentions")
            .and_then(Value::as_array)
            .map(|mentions| {
                mentions
                    .iter()
                    .filter_map(|mention| {
                        mention
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_intents_match_discordgo() {
        assert_eq!(GATEWAY_INTENTS, 37376);
    }

    #[test]
    fn parses_message_create_payload() {
        let payload = serde_json::json!({
            "op": 0,
            "t": "MESSAGE_CREATE",
            "s": 42,
            "d": {
                "id": "msg_1",
                "channel_id": "channel_1",
                "guild_id": "guild_1",
                "content": "<@bot_1> hello",
                "author": { "id": "user_1", "bot": false },
                "mentions": [ { "id": "bot_1" } ],
            },
        });
        let message = parse_gateway_message(payload.get("d")).expect("parse");
        assert_eq!(message.id, "msg_1");
        assert_eq!(message.guild_id, "guild_1");
        assert_eq!(message.content, "<@bot_1> hello");
        assert_eq!(
            message.author.as_ref().map(|a| a.id.as_str()),
            Some("user_1")
        );
        assert_eq!(message.mentions, vec!["bot_1".to_string()]);
    }

    #[test]
    fn parses_direct_message_payload_without_guild() {
        let payload = serde_json::json!({
            "d": {
                "id": "msg_2",
                "channel_id": "dm_1",
                "content": "hi",
                "author": { "id": "user_2", "bot": false },
            },
        });
        let message = parse_gateway_message(payload.get("d")).expect("parse");
        assert!(message.guild_id.is_empty());
        assert!(message.mentions.is_empty());
    }

    #[test]
    fn rejects_payloads_without_author() {
        let payload = serde_json::json!({ "d": { "id": "msg_3", "content": "hi" } });
        assert!(parse_gateway_message(payload.get("d")).is_none());
    }
}
