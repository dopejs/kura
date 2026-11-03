//! Live device connections, the sessions on them, and the one action each
//! may have in flight (docs/harness/remote-computer-use.md §5).
//!
//! The hub is in memory: a connection, a session and a pending action all
//! end with the process, and a device dials back in after a restart. One
//! connection per device; a new one replaces the old, whose waiters learn
//! that the outcome of what they asked is unknown.
//!
//! Every call names the tenant it acts for, and a device of another tenant
//! answers as offline, so ids cannot be probed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kura_device_protocol::{Action, ActionResult, CellFrame, DeviceFrame, Hello, Screen};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

/// Frames the cell may queue for one device before a send fails; a device
/// that stops reading is cut off rather than buffered without bound.
const OUTBOX: usize = 16;

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// How long the user has to answer a session request. Longer than the
    /// device's own prompt (60 s) so its answer, not ours, decides.
    pub consent_timeout: Duration,
    /// How long one action may take from send to result.
    pub action_timeout: Duration,
    /// The most a session may be asked to last.
    pub max_session: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            consent_timeout: Duration::from_secs(75),
            action_timeout: Duration::from_secs(10),
            max_session: Duration::from_secs(30 * 60),
        }
    }
}

/// Why a call to a device did not produce a result. `code` is what the
/// agent reads; `Timeout` and `Disconnected` say the outcome is unknown, so
/// it looks before it acts again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    Offline,
    Busy,
    NoSession,
    Denied(String),
    SessionEnded(String),
    Timeout,
    Disconnected,
    Invalid(String),
}

impl DeviceError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Offline => "device_offline",
            Self::Busy => "device_busy",
            Self::NoSession => "no_session",
            Self::Denied(_) => "session_denied",
            Self::SessionEnded(_) => "session_ended",
            Self::Timeout => "outcome_unknown_timeout",
            Self::Disconnected => "outcome_unknown_disconnected",
            Self::Invalid(_) => "invalid",
        }
    }
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => f.write_str("the device is not connected"),
            Self::Busy => f.write_str("the device is already doing something"),
            Self::NoSession => f.write_str("there is no control session on the device"),
            Self::Denied(reason) => write!(f, "the user did not allow control: {reason}"),
            Self::SessionEnded(reason) => write!(f, "the control session ended: {reason}"),
            Self::Timeout => f.write_str(
                "the device did not answer in time; the action may or may not have happened, \
                 take a screenshot before acting again",
            ),
            Self::Disconnected => f.write_str(
                "the device disconnected; the action may or may not have happened, take a \
                 screenshot before acting again",
            ),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DeviceError {}

/// What callers see of a connected device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineDevice {
    pub device_id: String,
    pub platform: String,
    pub screen: Screen,
    /// The granted session, if one is live.
    pub session: Option<String>,
}

enum Consent {
    Granted { until_ms: u64 },
    Denied(String),
}

struct Session {
    id: String,
    until_ms: u64,
}

struct Live {
    tenant_id: String,
    conn: u64,
    tx: mpsc::Sender<CellFrame>,
    hello: Hello,
    session: Option<Session>,
    asking: Option<(String, oneshot::Sender<Consent>)>,
    pending: Option<(String, oneshot::Sender<Result<ActionResult, DeviceError>>)>,
}

pub struct Hub {
    devices: Mutex<HashMap<String, Live>>,
    next_conn: AtomicU64,
    config: HubConfig,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(HubConfig::default())
    }
}

impl Hub {
    #[must_use]
    pub fn new(config: HubConfig) -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            config,
        }
    }

    /// Registers a connection that said `hello`, replacing any earlier one
    /// of the same device. Returns the connection's number, which later
    /// calls from it must give, and the frames to send it.
    pub fn connect(
        &self,
        device_id: &str,
        tenant_id: &str,
        hello: Hello,
    ) -> (u64, mpsc::Receiver<CellFrame>) {
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(OUTBOX);
        let replaced = self.devices.lock().insert(
            device_id.to_string(),
            Live {
                tenant_id: tenant_id.to_string(),
                conn,
                tx,
                hello,
                session: None,
                asking: None,
                pending: None,
            },
        );
        if let Some(old) = replaced {
            fail_waiters(old);
            record_connection("replaced");
        }
        record_connection("opened");
        (conn, rx)
    }

    /// Forgets connection `conn` of a device, if it is still the current
    /// one; a replaced connection leaving does not touch its successor.
    pub fn disconnect(&self, device_id: &str, conn: u64) {
        let mut devices = self.devices.lock();
        if devices.get(device_id).is_some_and(|live| live.conn == conn) {
            if let Some(old) = devices.remove(device_id) {
                drop(devices);
                fail_waiters(old);
            }
            record_connection("closed");
        }
    }

    /// Drops a revoked device's connection at once.
    pub fn revoke(&self, device_id: &str) {
        let removed = self.devices.lock().remove(device_id);
        if let Some(old) = removed {
            let _ = old.tx.try_send(CellFrame::SessionEnd {
                session: old
                    .session
                    .as_ref()
                    .map(|s| s.id.clone())
                    .unwrap_or_default(),
                reason: "revoked".into(),
            });
            fail_waiters(old);
            record_connection("revoked");
        }
    }

    /// A tenant's connected devices.
    pub fn online(&self, tenant_id: &str) -> Vec<OnlineDevice> {
        let now = now_ms();
        let mut online: Vec<OnlineDevice> = self
            .devices
            .lock()
            .iter()
            .filter(|(_, live)| live.tenant_id == tenant_id)
            .map(|(device_id, live)| OnlineDevice {
                device_id: device_id.clone(),
                platform: live.hello.platform.clone(),
                screen: live.hello.screen,
                session: live
                    .session
                    .as_ref()
                    .filter(|session| session.until_ms > now)
                    .map(|session| session.id.clone()),
            })
            .collect();
        online.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        online
    }

    /// A frame from connection `conn` of a device. Frames from a replaced
    /// connection, and answers nobody waits for, are dropped.
    pub fn on_frame(&self, device_id: &str, conn: u64, frame: DeviceFrame) {
        let mut devices = self.devices.lock();
        let Some(live) = devices.get_mut(device_id).filter(|live| live.conn == conn) else {
            return;
        };
        match frame {
            DeviceFrame::Hello(hello) => live.hello = hello,
            DeviceFrame::SessionGranted { session, until_ms } => {
                if let Some((asked, answer)) = live.asking.take() {
                    if asked == session {
                        let _ = answer.send(Consent::Granted { until_ms });
                    } else {
                        live.asking = Some((asked, answer));
                    }
                }
            }
            DeviceFrame::SessionDenied { session, reason } => {
                if let Some((asked, answer)) = live.asking.take() {
                    if asked == session {
                        let _ = answer.send(Consent::Denied(reason));
                    } else {
                        live.asking = Some((asked, answer));
                    }
                }
            }
            DeviceFrame::Result(result) => {
                if let Some((id, answer)) = live.pending.take() {
                    if id == result.id {
                        let _ = answer.send(Ok(result));
                    } else {
                        live.pending = Some((id, answer));
                    }
                }
            }
            DeviceFrame::SessionEnd { session, reason } => {
                if live.session.as_ref().is_some_and(|s| s.id == session) {
                    live.session = None;
                    if let Some((_, answer)) = live.pending.take() {
                        let _ = answer.send(Err(DeviceError::SessionEnded(reason)));
                    }
                }
            }
            // The connection loop answers pings and counts pongs itself.
            DeviceFrame::Ping | DeviceFrame::Pong => {}
        }
    }

    /// Asks the user for control of a device and waits for the answer.
    /// Returns the session id. A session already granted is reused, so an
    /// agent that asks twice does not prompt the user twice.
    pub async fn start_session(
        &self,
        tenant_id: &str,
        device_id: &str,
        reason: &str,
        ttl: Duration,
    ) -> Result<String, DeviceError> {
        let ttl = ttl.min(self.config.max_session);
        let session = format!("ses_{}", uuid::Uuid::new_v4().simple());
        let (answer_tx, answer_rx) = oneshot::channel();
        {
            let mut devices = self.devices.lock();
            let live = owned(&mut devices, tenant_id, device_id)?;
            if let Some(current) = &live.session {
                if current.until_ms > now_ms() {
                    return Ok(current.id.clone());
                }
                live.session = None;
            }
            if live.asking.is_some() {
                return Err(DeviceError::Busy);
            }
            live.tx
                .try_send(CellFrame::SessionRequest {
                    session: session.clone(),
                    reason: reason.to_string(),
                    ttl_s: ttl.as_secs(),
                })
                .map_err(|_| DeviceError::Disconnected)?;
            live.asking = Some((session.clone(), answer_tx));
        }
        let outcome = tokio::time::timeout(self.config.consent_timeout, answer_rx).await;
        let mut devices = self.devices.lock();
        let live = devices
            .get_mut(device_id)
            .filter(|live| live.tenant_id == tenant_id);
        match outcome {
            Ok(Ok(Consent::Granted { until_ms })) => {
                let Some(live) = live else {
                    return Err(DeviceError::Disconnected);
                };
                // The device may grant less than was asked, never more.
                let until_ms = until_ms.min(now_ms() + ttl.as_millis() as u64);
                live.session = Some(Session {
                    id: session.clone(),
                    until_ms,
                });
                record_session("granted");
                Ok(session)
            }
            Ok(Ok(Consent::Denied(reason))) => {
                record_session("denied");
                Err(DeviceError::Denied(reason))
            }
            // The connection went away with the question open.
            Ok(Err(_)) => Err(DeviceError::Disconnected),
            Err(_) => {
                if let Some(live) = live {
                    if live.asking.as_ref().is_some_and(|(id, _)| *id == session) {
                        live.asking = None;
                    }
                    let _ = live.tx.try_send(CellFrame::SessionEnd {
                        session,
                        reason: "no answer".into(),
                    });
                }
                record_session("timeout");
                Err(DeviceError::Denied("no answer in time".into()))
            }
        }
    }

    /// Runs one action in a granted session and waits for its result.
    pub async fn act(
        &self,
        tenant_id: &str,
        device_id: &str,
        session: &str,
        action: Action,
    ) -> Result<ActionResult, DeviceError> {
        action.validate().map_err(DeviceError::Invalid)?;
        let op = action.op();
        let id = format!("act_{}", uuid::Uuid::new_v4().simple());
        let deadline = self.config.action_timeout
            + match &action {
                Action::Wait { ms } => Duration::from_millis(*ms),
                _ => Duration::ZERO,
            };
        let (answer_tx, answer_rx) = oneshot::channel();
        {
            let mut devices = self.devices.lock();
            let live = owned(&mut devices, tenant_id, device_id)?;
            match &live.session {
                Some(current) if current.id == session && current.until_ms > now_ms() => {}
                Some(current) if current.id == session => {
                    live.session = None;
                    return Err(DeviceError::SessionEnded("expired".into()));
                }
                _ => return Err(DeviceError::NoSession),
            }
            if live.pending.is_some() {
                return Err(DeviceError::Busy);
            }
            live.tx
                .try_send(CellFrame::Action {
                    id: id.clone(),
                    session: session.to_string(),
                    action,
                })
                .map_err(|_| DeviceError::Disconnected)?;
            live.pending = Some((id.clone(), answer_tx));
        }
        let started = Instant::now();
        let outcome = match tokio::time::timeout(deadline, answer_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(DeviceError::Disconnected),
            Err(_) => {
                // A late result for this id is dropped by `on_frame`.
                let mut devices = self.devices.lock();
                if let Some(live) = devices.get_mut(device_id) {
                    if live
                        .pending
                        .as_ref()
                        .is_some_and(|(pending, _)| *pending == id)
                    {
                        live.pending = None;
                    }
                }
                Err(DeviceError::Timeout)
            }
        };
        record_action(op, &outcome, started.elapsed());
        outcome
    }

    /// Ends a session from the cell's side. Unknown sessions are ignored.
    pub fn end_session(&self, tenant_id: &str, device_id: &str, session: &str, reason: &str) {
        let mut devices = self.devices.lock();
        let Ok(live) = owned(&mut devices, tenant_id, device_id) else {
            return;
        };
        if live.session.as_ref().is_some_and(|s| s.id == session) {
            live.session = None;
            let _ = live.tx.try_send(CellFrame::SessionEnd {
                session: session.to_string(),
                reason: reason.to_string(),
            });
        }
    }
}

fn owned<'a>(
    devices: &'a mut HashMap<String, Live>,
    tenant_id: &str,
    device_id: &str,
) -> Result<&'a mut Live, DeviceError> {
    devices
        .get_mut(device_id)
        .filter(|live| live.tenant_id == tenant_id)
        .ok_or(DeviceError::Offline)
}

/// Whoever waited on a connection that is gone hears so now, not at their
/// timeout.
fn fail_waiters(live: Live) {
    if let Some((_, answer)) = live.pending {
        let _ = answer.send(Err(DeviceError::Disconnected));
    }
    // Dropping the consent sender wakes its waiter with `Disconnected`.
    drop(live.asking);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn record_connection(event: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::DEVICE_CONNECTIONS_TOTAL,
        &[("event", event)],
    );
}

fn record_session(outcome: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::DEVICE_SESSIONS_TOTAL,
        &[("outcome", outcome)],
    );
}

fn record_action(op: &str, outcome: &Result<ActionResult, DeviceError>, took: Duration) {
    let outcome = match outcome {
        Ok(result) if result.ok => "ok",
        Ok(_) => "refused",
        Err(err) => err.code(),
    };
    let metrics = kura_telemetry::metrics::registry();
    metrics.inc(
        kura_telemetry::metrics::DEVICE_ACTIONS_TOTAL,
        &[("op", op), ("outcome", outcome)],
    );
    metrics.observe(
        kura_telemetry::metrics::DEVICE_ACTION_DURATION_SECONDS,
        &[("op", op)],
        took.as_secs_f64(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_device_protocol::PROTOCOL_VERSION;
    use std::sync::Arc;

    fn hello() -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            agent: "test".into(),
            platform: "macos".into(),
            screen: Screen {
                width: 1440,
                height: 900,
                scale: 2,
            },
        }
    }

    fn quick() -> HubConfig {
        HubConfig {
            consent_timeout: Duration::from_millis(200),
            action_timeout: Duration::from_millis(200),
            max_session: Duration::from_secs(60),
        }
    }

    /// Answers what the fake device is asked, like a user who allows.
    fn allow_and_answer(
        hub: &Arc<Hub>,
        device: &str,
        conn: u64,
        mut rx: mpsc::Receiver<CellFrame>,
    ) {
        let hub = hub.clone();
        let device = device.to_string();
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let reply = match frame {
                    CellFrame::SessionRequest { session, .. } => DeviceFrame::SessionGranted {
                        session,
                        until_ms: now_ms() + 3_600_000,
                    },
                    CellFrame::Action { id, .. } => DeviceFrame::Result(ActionResult {
                        id,
                        ok: true,
                        error: None,
                        screenshot: None,
                    }),
                    _ => continue,
                };
                hub.on_frame(&device, conn, reply);
            }
        });
    }

    #[tokio::test]
    async fn a_granted_session_runs_actions_and_is_reused() {
        let hub = Arc::new(Hub::new(quick()));
        let (conn, rx) = hub.connect("dev_1", "ten_a", hello());
        allow_and_answer(&hub, "dev_1", conn, rx);

        let session = hub
            .start_session("ten_a", "dev_1", "check mail", Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(
            hub.start_session("ten_a", "dev_1", "again", Duration::from_secs(30))
                .await
                .unwrap(),
            session
        );
        let result = hub
            .act("ten_a", "dev_1", &session, Action::Screenshot)
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(
            hub.online("ten_a")[0].session.as_deref(),
            Some(session.as_str())
        );

        hub.end_session("ten_a", "dev_1", &session, "turn over");
        assert_eq!(
            hub.act("ten_a", "dev_1", &session, Action::Screenshot)
                .await,
            Err(DeviceError::NoSession)
        );
    }

    #[tokio::test]
    async fn another_tenant_sees_the_device_as_offline() {
        let hub = Arc::new(Hub::new(quick()));
        let (conn, rx) = hub.connect("dev_1", "ten_a", hello());
        allow_and_answer(&hub, "dev_1", conn, rx);
        assert!(hub.online("ten_b").is_empty());
        assert_eq!(
            hub.start_session("ten_b", "dev_1", "x", Duration::from_secs(5))
                .await,
            Err(DeviceError::Offline)
        );
        let session = hub
            .start_session("ten_a", "dev_1", "x", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            hub.act("ten_b", "dev_1", &session, Action::Screenshot)
                .await,
            Err(DeviceError::Offline)
        );
    }

    #[tokio::test]
    async fn a_denial_and_a_silent_user_both_refuse() {
        let hub = Arc::new(Hub::new(quick()));
        let (conn, mut rx) = hub.connect("dev_1", "ten_a", hello());
        let asker = {
            let hub = hub.clone();
            tokio::spawn(async move {
                hub.start_session("ten_a", "dev_1", "x", Duration::from_secs(5))
                    .await
            })
        };
        let Some(CellFrame::SessionRequest { session, .. }) = rx.recv().await else {
            panic!("no request");
        };
        hub.on_frame(
            "dev_1",
            conn,
            DeviceFrame::SessionDenied {
                session,
                reason: "not now".into(),
            },
        );
        assert_eq!(
            asker.await.unwrap(),
            Err(DeviceError::Denied("not now".into()))
        );

        // Nobody answers: refused at the timeout, and the device is told.
        let silent = hub
            .start_session("ten_a", "dev_1", "x", Duration::from_secs(5))
            .await;
        assert!(matches!(silent, Err(DeviceError::Denied(_))));
        assert!(matches!(
            rx.recv().await,
            Some(CellFrame::SessionRequest { .. })
        ));
        assert!(matches!(
            rx.recv().await,
            Some(CellFrame::SessionEnd { .. })
        ));
    }

    #[tokio::test]
    async fn a_lost_connection_or_a_late_answer_leaves_the_outcome_unknown() {
        let hub = Arc::new(Hub::new(quick()));
        let (conn, rx) = hub.connect("dev_1", "ten_a", hello());
        // Grants, but never answers actions.
        let granter = {
            let hub = hub.clone();
            let mut rx = rx;
            tokio::spawn(async move {
                let mut actions = Vec::new();
                while let Some(frame) = rx.recv().await {
                    match frame {
                        CellFrame::SessionRequest { session, .. } => hub.on_frame(
                            "dev_1",
                            conn,
                            DeviceFrame::SessionGranted {
                                session,
                                until_ms: now_ms() + 60_000,
                            },
                        ),
                        CellFrame::Action { id, .. } => actions.push(id),
                        _ => {}
                    }
                }
                actions
            })
        };
        let session = hub
            .start_session("ten_a", "dev_1", "x", Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(
            hub.act("ten_a", "dev_1", &session, Action::Screenshot)
                .await,
            Err(DeviceError::Timeout)
        );
        // After a timeout the next action may go.
        let busy_free = {
            let hub = hub.clone();
            let session = session.clone();
            tokio::spawn(async move {
                hub.act("ten_a", "dev_1", &session, Action::Screenshot)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        // One in flight: a second is refused, not queued.
        assert_eq!(
            hub.act("ten_a", "dev_1", &session, Action::Screenshot)
                .await,
            Err(DeviceError::Busy)
        );
        hub.disconnect("dev_1", conn);
        assert_eq!(busy_free.await.unwrap(), Err(DeviceError::Disconnected));
        assert_eq!(granter.await.unwrap().len(), 2);
        assert!(hub.online("ten_a").is_empty());
    }

    #[tokio::test]
    async fn a_new_connection_replaces_the_old_one() {
        let hub = Arc::new(Hub::new(quick()));
        let (old, _old_rx) = hub.connect("dev_1", "ten_a", hello());
        let (new, rx) = hub.connect("dev_1", "ten_a", hello());
        allow_and_answer(&hub, "dev_1", new, rx);
        // The old connection leaving does not take the new one with it.
        hub.disconnect("dev_1", old);
        assert_eq!(hub.online("ten_a").len(), 1);
        // Frames from the old connection are ignored.
        hub.on_frame(
            "dev_1",
            old,
            DeviceFrame::SessionGranted {
                session: "forged".into(),
                until_ms: u64::MAX,
            },
        );
        assert!(hub.online("ten_a")[0].session.is_none());

        hub.revoke("dev_1");
        assert!(hub.online("ten_a").is_empty());
    }

    #[tokio::test]
    async fn invalid_actions_never_reach_the_device() {
        let hub = Arc::new(Hub::new(quick()));
        let (_conn, mut rx) = hub.connect("dev_1", "ten_a", hello());
        assert!(matches!(
            hub.act("ten_a", "dev_1", "s", Action::Wait { ms: 60_000 })
                .await,
            Err(DeviceError::Invalid(_))
        ));
        assert!(rx.try_recv().is_err());
    }
}
