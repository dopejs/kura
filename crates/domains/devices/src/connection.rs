//! One device connection, from `hello` to close.
//!
//! Transport-agnostic: the API layer hands over the socket as a stream of
//! text frames in and a sink of text frames out, so this loop is tested
//! without a server. A protocol violation closes the connection; the
//! device reconnects and starts clean.

use std::sync::Arc;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use kura_device_protocol::{CellFrame, DeviceFrame, PROTOCOL_VERSION};
use kura_store::DeviceRecord;
use tokio::time::{Instant, MissedTickBehavior};

use crate::hub::Hub;

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// How long a new connection has to say `hello`.
    pub hello_timeout: Duration,
    /// How often the cell pings.
    pub ping_every: Duration,
    /// Silence after which the device counts as gone.
    pub idle_timeout: Duration,
    /// The largest frame accepted, which bounds a screenshot.
    pub max_frame_bytes: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            hello_timeout: Duration::from_secs(10),
            ping_every: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(60),
            max_frame_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Why a connection ended, for the log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// The device closed it, or the socket failed.
    Peer,
    NoHello,
    WrongProtocol(u32),
    Malformed(String),
    TooLarge(usize),
    Idle,
    /// The hub dropped it: revoked, or replaced by a newer connection.
    Dropped,
}

/// Serves one authenticated device until the connection ends.
pub async fn serve<In, Out>(
    hub: Arc<Hub>,
    device: &DeviceRecord,
    mut incoming: In,
    mut outgoing: Out,
    config: &ConnectionConfig,
) -> Closed
where
    In: Stream<Item = String> + Unpin,
    Out: Sink<String> + Unpin,
{
    let hello = match tokio::time::timeout(config.hello_timeout, incoming.next()).await {
        Ok(Some(text)) => match parse(&text, config) {
            Ok(DeviceFrame::Hello(hello)) => hello,
            Ok(_) => return Closed::NoHello,
            Err(closed) => return closed,
        },
        Ok(None) => return Closed::Peer,
        Err(_) => return Closed::NoHello,
    };
    if hello.protocol != PROTOCOL_VERSION {
        return Closed::WrongProtocol(hello.protocol);
    }
    let (conn, mut rx) = hub.connect(&device.device_id, &device.tenant_id, hello);
    let welcome = CellFrame::Welcome {
        protocol: PROTOCOL_VERSION,
        device_id: device.device_id.clone(),
    };
    let closed = if send(&mut outgoing, &welcome).await {
        pump(
            &hub,
            device,
            conn,
            &mut incoming,
            &mut outgoing,
            &mut rx,
            config,
        )
        .await
    } else {
        Closed::Peer
    };
    hub.disconnect(&device.device_id, conn);
    closed
}

async fn pump<In, Out>(
    hub: &Hub,
    device: &DeviceRecord,
    conn: u64,
    incoming: &mut In,
    outgoing: &mut Out,
    rx: &mut tokio::sync::mpsc::Receiver<CellFrame>,
    config: &ConnectionConfig,
) -> Closed
where
    In: Stream<Item = String> + Unpin,
    Out: Sink<String> + Unpin,
{
    let mut ping = tokio::time::interval(config.ping_every);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            text = incoming.next() => {
                let Some(text) = text else {
                    return Closed::Peer;
                };
                heard = Instant::now();
                match parse(&text, config) {
                    Ok(DeviceFrame::Ping) => {
                        if !send(outgoing, &CellFrame::Pong).await {
                            return Closed::Peer;
                        }
                    }
                    Ok(frame) => hub.on_frame(&device.device_id, conn, frame),
                    Err(closed) => return closed,
                }
            }
            frame = rx.recv() => {
                let Some(frame) = frame else {
                    return Closed::Dropped;
                };
                if !send(outgoing, &frame).await {
                    return Closed::Peer;
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() >= config.idle_timeout {
                    return Closed::Idle;
                }
                if !send(outgoing, &CellFrame::Ping).await {
                    return Closed::Peer;
                }
            }
        }
    }
}

fn parse(text: &str, config: &ConnectionConfig) -> Result<DeviceFrame, Closed> {
    if text.len() > config.max_frame_bytes {
        return Err(Closed::TooLarge(text.len()));
    }
    serde_json::from_str(text).map_err(|err| Closed::Malformed(err.to_string()))
}

async fn send<Out: Sink<String> + Unpin>(outgoing: &mut Out, frame: &CellFrame) -> bool {
    match serde_json::to_string(frame) {
        Ok(text) => outgoing.send(text).await.is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{DeviceError, HubConfig};
    use chrono::Utc;
    use futures::channel::mpsc;
    use kura_device_protocol::{Action, ActionResult, Hello, Screen};

    fn device() -> DeviceRecord {
        DeviceRecord {
            device_id: "dev_1".into(),
            tenant_id: "ten_a".into(),
            principal_id: "prn_1".into(),
            name: "mac".into(),
            platform: "macos".into(),
            token_hash: "h".into(),
            created_at: Utc::now(),
            last_seen_at: None,
            revoked_at: None,
        }
    }

    fn hello(protocol: u32) -> String {
        serde_json::to_string(&DeviceFrame::Hello(Hello {
            protocol,
            agent: "test".into(),
            platform: "macos".into(),
            screen: Screen {
                width: 1440,
                height: 900,
                scale: 2,
            },
        }))
        .unwrap()
    }

    fn quick() -> ConnectionConfig {
        ConnectionConfig {
            hello_timeout: Duration::from_millis(100),
            ping_every: Duration::from_millis(50),
            idle_timeout: Duration::from_millis(200),
            max_frame_bytes: 1024,
        }
    }

    struct Fake {
        to_cell: mpsc::UnboundedSender<String>,
        from_cell: mpsc::UnboundedReceiver<String>,
    }

    impl Fake {
        fn send(&self, frame: &DeviceFrame) {
            self.to_cell
                .unbounded_send(serde_json::to_string(frame).unwrap())
                .unwrap();
        }

        /// The next frame that is not a ping.
        async fn next(&mut self) -> CellFrame {
            loop {
                let text = self.from_cell.next().await.expect("cell closed");
                let frame: CellFrame = serde_json::from_str(&text).unwrap();
                if frame != CellFrame::Ping {
                    return frame;
                }
            }
        }
    }

    fn start(hub: &Arc<Hub>, config: ConnectionConfig) -> (Fake, tokio::task::JoinHandle<Closed>) {
        let (to_cell, incoming) = mpsc::unbounded::<String>();
        let (outgoing, from_cell) = mpsc::unbounded::<String>();
        let hub = hub.clone();
        let task =
            tokio::spawn(async move { serve(hub, &device(), incoming, outgoing, &config).await });
        (Fake { to_cell, from_cell }, task)
    }

    #[tokio::test]
    async fn a_device_says_hello_is_welcomed_and_answers_an_action() {
        let hub = Arc::new(Hub::new(HubConfig::default()));
        let (mut fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION))
            .unwrap();
        assert_eq!(
            fake.next().await,
            CellFrame::Welcome {
                protocol: PROTOCOL_VERSION,
                device_id: "dev_1".into()
            }
        );
        assert_eq!(hub.online("ten_a").len(), 1);

        let asker = {
            let hub = hub.clone();
            tokio::spawn(async move {
                let session = hub
                    .start_session("ten_a", "dev_1", "look", Duration::from_secs(60))
                    .await?;
                hub.act("ten_a", "dev_1", &session, Action::Screenshot)
                    .await
            })
        };
        let CellFrame::SessionRequest { session, .. } = fake.next().await else {
            panic!("expected a session request");
        };
        fake.send(&DeviceFrame::SessionGranted {
            session: session.clone(),
            until_ms: u64::MAX,
        });
        let CellFrame::Action { id, action, .. } = fake.next().await else {
            panic!("expected an action");
        };
        assert_eq!(action, Action::Screenshot);
        fake.send(&DeviceFrame::Result(ActionResult {
            id,
            ok: true,
            error: None,
            screenshot: None,
        }));
        assert!(asker.await.unwrap().unwrap().ok);

        // The device pings, the cell answers.
        fake.send(&DeviceFrame::Ping);
        assert_eq!(fake.next().await, CellFrame::Pong);

        drop(fake);
        assert_eq!(task.await.unwrap(), Closed::Peer);
        assert!(hub.online("ten_a").is_empty());
    }

    #[tokio::test]
    async fn the_wrong_first_frame_or_version_is_refused() {
        let hub = Arc::new(Hub::new(HubConfig::default()));
        let (fake, task) = start(&hub, quick());
        fake.send(&DeviceFrame::Ping);
        assert_eq!(task.await.unwrap(), Closed::NoHello);

        let (fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION + 1))
            .unwrap();
        assert_eq!(
            task.await.unwrap(),
            Closed::WrongProtocol(PROTOCOL_VERSION + 1)
        );

        let (_fake, task) = start(&hub, quick());
        assert_eq!(task.await.unwrap(), Closed::NoHello);
        assert!(hub.online("ten_a").is_empty());
    }

    #[tokio::test]
    async fn garbage_oversize_and_silence_close_the_connection() {
        let hub = Arc::new(Hub::new(HubConfig::default()));
        let (mut fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION))
            .unwrap();
        fake.next().await;
        fake.to_cell.unbounded_send("{not json".into()).unwrap();
        assert!(matches!(task.await.unwrap(), Closed::Malformed(_)));

        let (mut fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION))
            .unwrap();
        fake.next().await;
        fake.to_cell.unbounded_send("x".repeat(2048)).unwrap();
        assert_eq!(task.await.unwrap(), Closed::TooLarge(2048));

        let (mut fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION))
            .unwrap();
        fake.next().await;
        // Says nothing more: idle after 200 ms, though the cell kept pinging.
        assert_eq!(task.await.unwrap(), Closed::Idle);
        assert!(hub.online("ten_a").is_empty());
        drop(fake);
    }

    #[tokio::test]
    async fn revoking_closes_a_live_connection_and_fails_its_waiter() {
        let hub = Arc::new(Hub::new(HubConfig::default()));
        let (mut fake, task) = start(&hub, quick());
        fake.to_cell
            .unbounded_send(hello(PROTOCOL_VERSION))
            .unwrap();
        fake.next().await;
        let asker = {
            let hub = hub.clone();
            tokio::spawn(async move {
                hub.start_session("ten_a", "dev_1", "x", Duration::from_secs(60))
                    .await
            })
        };
        assert!(matches!(
            fake.next().await,
            CellFrame::SessionRequest { .. }
        ));
        hub.revoke("dev_1");
        assert_eq!(asker.await.unwrap(), Err(DeviceError::Disconnected));
        assert_eq!(task.await.unwrap(), Closed::Dropped);
    }
}
