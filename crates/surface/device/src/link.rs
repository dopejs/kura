//! The one outbound connection to the cell, and reconnecting it
//! (docs/harness/remote-computer-use.md §3, §5, §11).
//!
//! [`session`] speaks the protocol over any stream of text frames, so it is
//! tested against the cell's own connection loop without a network;
//! [`run`] adds the WebSocket, the token header and the backoff.

use std::sync::Arc;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use kura_device_protocol::{CellFrame, DeviceFrame, PROTOCOL_VERSION};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest};

use crate::agent::Agent;
use crate::log;

pub const TOKEN_HEADER: &str = "X-Kura-Device-Token";

#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub welcome_timeout: Duration,
    pub ping_every: Duration,
    /// Silence from the cell after which the connection counts as dead.
    pub idle_timeout: Duration,
    /// The largest frame accepted from the cell; its frames are small.
    pub max_frame_bytes: usize,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// A connection that lasted this long resets the backoff.
    pub stable_after: Duration,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            welcome_timeout: Duration::from_secs(10),
            ping_every: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(60),
            max_frame_bytes: 256 * 1024,
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            stable_after: Duration::from_secs(30),
        }
    }
}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The cell refused the token: revoked, unknown, or another tenant's.
    /// Reconnecting cannot help; the device must be paired again.
    Unauthorized,
    /// Anything else; worth reconnecting.
    Lost(String),
    /// Asked to stop.
    Shutdown,
}

/// Connects, serves, and reconnects with backoff until the token is
/// refused or `shutdown` fires.
pub async fn run(
    agent: Arc<Agent>,
    url: String,
    token: String,
    config: LinkConfig,
    kills: &mut mpsc::Receiver<()>,
    shutdown: &mut mpsc::Receiver<()>,
) -> Ended {
    let mut backoff = config.backoff_min;
    loop {
        let started = Instant::now();
        let ended = tokio::select! {
            ended = connect(&agent, &url, &token, &config, kills) => ended,
            _ = shutdown.recv() => Ended::Shutdown,
        };
        match &ended {
            Ended::Unauthorized | Ended::Shutdown => return ended,
            Ended::Lost(why) => log::warn(&format!("connection lost: {why}")),
        }
        if started.elapsed() >= config.stable_after {
            backoff = config.backoff_min;
        }
        let wait = jitter(backoff);
        log::info(&format!("reconnecting in {}ms", wait.as_millis()));
        // The kill switch works while disconnected too: it blocks the next
        // sessions even though there is no session to end.
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                Some(()) = kills.recv() => { agent.kill(); }
                _ = shutdown.recv() => return Ended::Shutdown,
            }
        }
        backoff = (backoff * 2).min(config.backoff_max);
    }
}

/// Up to half again of `base`, so devices dropped together by a cell
/// restart do not reconnect together.
fn jitter(base: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    base + base.mul_f64(f64::from(nanos % 1000) / 2000.0)
}

/// `https://host/base` → `wss://host/base/v1/devices/connect`.
pub fn connect_url(server: &str) -> Result<String, String> {
    let server = server.trim_end_matches('/');
    let rest = if let Some(rest) = server.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = server.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(format!(
            "server must start with https:// or http://: {server}"
        ));
    };
    Ok(format!("{rest}/v1/devices/connect"))
}

async fn connect(
    agent: &Arc<Agent>,
    url: &str,
    token: &str,
    config: &LinkConfig,
    kills: &mut mpsc::Receiver<()>,
) -> Ended {
    let mut request = match url.into_client_request() {
        Ok(request) => request,
        Err(err) => return Ended::Lost(format!("bad url: {err}")),
    };
    match token.parse() {
        Ok(value) => {
            request.headers_mut().insert(TOKEN_HEADER, value);
        }
        Err(_) => return Ended::Unauthorized,
    }
    let ws_config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(config.max_frame_bytes),
        max_frame_size: Some(config.max_frame_bytes),
        ..Default::default()
    };
    let socket =
        tokio_tungstenite::connect_async_with_config(request, Some(ws_config), false).await;
    let (socket, _) = match socket {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(response)) if response.status().as_u16() == 401 => {
            return Ended::Unauthorized;
        }
        Err(err) => return Ended::Lost(format!("connect: {err}")),
    };
    log::info("connected");
    let (sink, stream) = socket.split();
    // Text frames only; pings at the WebSocket level are answered by
    // tungstenite, and protocol pings are JSON.
    let incoming = stream.filter_map(|message| async move {
        match message {
            Ok(Message::Text(text)) => Some(Ok(text)),
            Ok(Message::Close(_)) | Err(_) => Some(Err(())),
            Ok(_) => None,
        }
    });
    let incoming = incoming.take_while(|item| futures::future::ready(item.is_ok()));
    let incoming = incoming.filter_map(|item| async move { item.ok() });
    let outgoing =
        sink.with(|text: String| async move { Ok::<_, tungstenite::Error>(Message::Text(text)) });
    session(agent, Box::pin(incoming), Box::pin(outgoing), config, kills).await
}

/// One connection: `hello`, `welcome`, then frames until it ends.
pub async fn session<In, Out>(
    agent: &Arc<Agent>,
    mut incoming: In,
    mut outgoing: Out,
    config: &LinkConfig,
    kills: &mut mpsc::Receiver<()>,
) -> Ended
where
    In: Stream<Item = String> + Unpin,
    Out: Sink<String> + Unpin,
{
    let hello = match agent.hello().await {
        Ok(hello) => hello,
        Err(err) => return Ended::Lost(format!("display: {err}")),
    };
    if !send(&mut outgoing, &hello).await {
        return Ended::Lost("send hello".into());
    }
    match tokio::time::timeout(config.welcome_timeout, incoming.next()).await {
        Ok(Some(text)) => match parse(&text, config) {
            Ok(CellFrame::Welcome {
                protocol,
                device_id,
            }) if protocol == PROTOCOL_VERSION => {
                log::info(&format!("welcomed as {device_id}"));
            }
            Ok(CellFrame::Welcome { protocol, .. }) => {
                return Ended::Lost(format!("cell speaks protocol {protocol}"));
            }
            Ok(_) => return Ended::Lost("expected welcome".into()),
            Err(err) => return Ended::Lost(err),
        },
        Ok(None) => return Ended::Lost("closed before welcome".into()),
        Err(_) => return Ended::Lost("no welcome".into()),
    }

    // Consent and actions take seconds; they run as tasks and answer through
    // this channel so pings and the kill switch are never stuck behind them.
    let (answers_tx, mut answers) = mpsc::channel::<DeviceFrame>(16);
    let mut tasks = tokio::task::JoinSet::new();
    let mut ping = tokio::time::interval(config.ping_every);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await;
    let mut heard = Instant::now();
    let ended = loop {
        tokio::select! {
            text = incoming.next() => {
                let Some(text) = text else {
                    break Ended::Lost("closed by the cell".into());
                };
                heard = Instant::now();
                let frame = match parse(&text, config) {
                    Ok(frame) => frame,
                    Err(err) => break Ended::Lost(err),
                };
                match frame {
                    CellFrame::Ping => {
                        if !send(&mut outgoing, &DeviceFrame::Pong).await {
                            break Ended::Lost("send pong".into());
                        }
                    }
                    CellFrame::Pong | CellFrame::Welcome { .. } => {}
                    CellFrame::SessionRequest { session, reason, ttl_s } => {
                        let (agent, answers) = (agent.clone(), answers_tx.clone());
                        tasks.spawn(async move {
                            let _ = answers.send(agent.request(session, reason, ttl_s).await).await;
                        });
                    }
                    CellFrame::Action { id, session, action } => {
                        let (agent, answers) = (agent.clone(), answers_tx.clone());
                        tasks.spawn(async move {
                            for frame in agent.act(id, session, action).await {
                                let _ = answers.send(frame).await;
                            }
                        });
                    }
                    CellFrame::SessionEnd { session, reason } => {
                        log::info(&format!("cell ended session {session}: {reason}"));
                        agent.end(&session);
                    }
                }
            }
            Some(frame) = answers.recv() => {
                if !send(&mut outgoing, &frame).await {
                    break Ended::Lost("send".into());
                }
            }
            Some(()) = kills.recv() => {
                if let Some(frame) = agent.kill() {
                    if !send(&mut outgoing, &frame).await {
                        break Ended::Lost("send".into());
                    }
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() >= config.idle_timeout {
                    break Ended::Lost("cell silent".into());
                }
                if !send(&mut outgoing, &DeviceFrame::Ping).await {
                    break Ended::Lost("send ping".into());
                }
            }
        }
    };
    // A consent dialog still open belongs to a connection that is gone; its
    // answer must not grant a session the next connection never asked for.
    tasks.abort_all();
    agent.disconnected();
    ended
}

fn parse(text: &str, config: &LinkConfig) -> Result<CellFrame, String> {
    if text.len() > config.max_frame_bytes {
        return Err(format!("frame of {} bytes", text.len()));
    }
    serde_json::from_str(text).map_err(|err| format!("malformed frame: {err}"))
}

async fn send<Out: Sink<String> + Unpin>(outgoing: &mut Out, frame: &DeviceFrame) -> bool {
    match serde_json::to_string(frame) {
        Ok(text) => outgoing.send(text).await.is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_become_connect_urls() {
        assert_eq!(
            connect_url("https://kura.example.com/").unwrap(),
            "wss://kura.example.com/v1/devices/connect"
        );
        assert_eq!(
            connect_url("http://127.0.0.1:19192").unwrap(),
            "ws://127.0.0.1:19192/v1/devices/connect"
        );
        assert!(connect_url("kura.example.com").is_err());
    }

    #[test]
    fn jitter_stays_within_half_again() {
        let base = Duration::from_secs(4);
        let wait = jitter(base);
        assert!(wait >= base && wait <= base + base / 2);
    }
}
