//! The device side of a control session: what the cell may do, and when.
//!
//! The Mac is the trust boundary (docs/harness/remote-computer-use.md §8),
//! so every rule that protects the person is enforced here and none can be
//! switched off by the cell:
//! - no action outside a session the person granted on this computer;
//! - a session ends when the person touches the mouse or keyboard
//!   (`user_active`), presses the kill switch, or its time runs out;
//! - after the kill switch no session is granted for [`Policy::kill_block`];
//! - nothing is clicked or typed into a never-touch app, and its windows
//!   are black in every screenshot;
//! - nothing is typed while secure keyboard entry is on;
//! - the action limits are checked again, whatever the cell checked.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kura_device_protocol::{
    Action, ActionResult, Button, DeviceFrame, Hello, PROTOCOL_VERSION, Screen, Screenshot,
};
use parking_lot::Mutex;

use crate::keys;
use crate::log;
use crate::platform::{Answer, Consent, Display, Input, MouseButton, Platform, PlatformError};
use crate::shot::{self, Rect};

/// Bundle ids the agent may never click or type into, and never see.
pub const DEFAULT_DENY_APPS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "com.lastpass.LastPass",
    "com.dashlane.dashlanephonefinal",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.apple.systempreferences",
    // Terminals: one keystroke there runs anything as the person.
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "com.github.wez.wezterm",
];

#[derive(Debug, Clone)]
pub struct Policy {
    pub deny_apps: Vec<String>,
    /// How long the person has to answer a request for control.
    pub consent_timeout: Duration,
    /// The longest session granted, whatever the cell asks for.
    pub max_session: Duration,
    /// How long after the kill switch requests are refused unasked.
    pub kill_block: Duration,
    /// How long to let the screen settle after an input before the
    /// screenshot that answers it.
    pub settle: Duration,
    /// Sessions requested before this (Unix ms) are granted without asking
    /// (`--trust-for`). Zero: always ask.
    pub trust_until_ms: u64,
    /// Input this soon after a grant does not end the session: the person
    /// has just clicked Allow and is moving their hand away.
    pub takeover_grace: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            deny_apps: DEFAULT_DENY_APPS.iter().map(|s| (*s).to_owned()).collect(),
            consent_timeout: Duration::from_secs(60),
            max_session: Duration::from_secs(30 * 60),
            kill_block: Duration::from_secs(10 * 60),
            settle: Duration::from_millis(300),
            trust_until_ms: 0,
            takeover_grace: Duration::from_secs(3),
        }
    }
}

/// When the person last used the mouse or keyboard, fed by the platform's
/// input watcher. Input the device itself posts is not counted.
#[derive(Debug, Default)]
pub struct Activity {
    last_input_ms: AtomicU64,
}

impl Activity {
    pub fn touched(&self, at_ms: u64) {
        self.last_input_ms.fetch_max(at_ms, Ordering::Relaxed);
    }

    fn last(&self) -> u64 {
        self.last_input_ms.load(Ordering::Relaxed)
    }
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(|| chrono::Utc::now().timestamp_millis().max(0) as u64)
}

struct Session {
    id: String,
    until_ms: u64,
    /// Input by the person after this ends the session: the grant time plus
    /// [`Policy::takeover_grace`]. The device's own input never counts (the
    /// watcher tells them apart), so it does not move.
    quiet_since_ms: u64,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    blocked_until_ms: u64,
    /// The screen the cell last heard about; a change re-sends `hello`.
    announced: Option<Screen>,
}

pub struct Agent {
    platform: Arc<dyn Platform>,
    consent: Arc<dyn Consent>,
    activity: Arc<Activity>,
    policy: Policy,
    clock: Clock,
    state: Mutex<State>,
}

impl Agent {
    pub fn new(
        platform: Arc<dyn Platform>,
        consent: Arc<dyn Consent>,
        activity: Arc<Activity>,
        policy: Policy,
        clock: Clock,
    ) -> Self {
        Self {
            platform,
            consent,
            activity,
            policy,
            clock,
            state: Mutex::new(State::default()),
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// The first frame of a connection. Forgets any session: none survives
    /// a reconnect (§11).
    pub async fn hello(&self) -> Result<DeviceFrame, PlatformError> {
        let platform = self.platform.clone();
        let display = blocking(move || platform.display()).await?;
        let screen = screen_of(&display);
        let mut state = self.state.lock();
        state.session = None;
        state.announced = Some(screen);
        Ok(DeviceFrame::Hello(hello(screen)))
    }

    /// Asks the person, unless the kill switch or `--trust-for` already
    /// decides. A new request replaces any current session.
    pub async fn request(&self, session: String, reason: String, ttl_s: u64) -> DeviceFrame {
        self.state.lock().session = None;
        let now = self.now();
        if self.state.lock().blocked_until_ms > now {
            log::info(&format!("session {session} refused: kill switch"));
            return denied(session, "kill_switch");
        }
        let answer = if now < self.policy.trust_until_ms {
            Answer::Allow
        } else {
            let consent = self.consent.clone();
            let timeout = self.policy.consent_timeout;
            let reason = clip(&reason, 300);
            blocking(move || Ok(consent.ask(&reason, ttl_s, timeout)))
                .await
                .unwrap_or(Answer::Deny)
        };
        let now = self.now();
        let mut state = self.state.lock();
        // The kill switch may have been pressed while the dialog was up.
        if state.blocked_until_ms > now {
            return denied(session, "kill_switch");
        }
        match answer {
            Answer::Allow => {
                let ttl = Duration::from_secs(ttl_s).min(self.policy.max_session);
                let until_ms = now + ttl.as_millis() as u64;
                log::info(&format!("session {session} granted for {}s", ttl.as_secs()));
                state.session = Some(Session {
                    id: session.clone(),
                    until_ms,
                    quiet_since_ms: now + self.policy.takeover_grace.as_millis() as u64,
                });
                DeviceFrame::SessionGranted { session, until_ms }
            }
            Answer::Deny => {
                log::info(&format!("session {session} denied by the user"));
                denied(session, "user_denied")
            }
            Answer::Timeout => {
                log::info(&format!("session {session} not answered in time"));
                denied(session, "timeout")
            }
        }
    }

    /// The cell ended the session.
    pub fn end(&self, session: &str) {
        let mut state = self.state.lock();
        if state.session.as_ref().is_some_and(|s| s.id == session) {
            state.session = None;
            log::info(&format!("session {session} ended by the cell"));
        }
    }

    /// The connection is gone, and with it any session (§11).
    pub fn disconnected(&self) {
        if let Some(session) = self.state.lock().session.take() {
            log::info(&format!("session {} ended: disconnected", session.id));
        }
    }

    /// The kill switch: ends the session now and refuses new ones for a
    /// while. Returns the frame that tells the cell, when a session was live.
    pub fn kill(&self) -> Option<DeviceFrame> {
        let mut state = self.state.lock();
        state.blocked_until_ms = self.now() + self.policy.kill_block.as_millis() as u64;
        log::info(&format!(
            "kill switch: control refused for {}s",
            self.policy.kill_block.as_secs()
        ));
        state.session.take().map(|s| DeviceFrame::SessionEnd {
            session: s.id,
            reason: "user_stopped".into(),
        })
    }

    /// Runs one action and answers it. May also end the session, or
    /// re-announce a changed display before the result.
    pub async fn act(&self, id: String, session: String, action: Action) -> Vec<DeviceFrame> {
        let op = action.op();
        let started = std::time::Instant::now();
        let frames = self.act_inner(&id, &session, action).await;
        if let Some(DeviceFrame::Result(result)) = frames
            .iter()
            .find(|frame| matches!(frame, DeviceFrame::Result(_)))
        {
            log::info(&format!(
                "action {id} {op}: {} in {}ms",
                result.error.as_deref().unwrap_or("ok"),
                started.elapsed().as_millis()
            ));
        }
        frames
    }

    // A session that ends here is announced before the result: the cell then
    // fails the pending call as `session_ended` and drops the late result,
    // so the agent cannot race a next action into a session that is gone.
    async fn act_inner(&self, id: &str, session: &str, action: Action) -> Vec<DeviceFrame> {
        let refuse = |code: &str| vec![failure(id, code)];
        {
            let now = self.now();
            let mut state = self.state.lock();
            let Some(current) = state.session.as_ref().filter(|s| s.id == session) else {
                return refuse("no_session");
            };
            if current.until_ms <= now {
                let ended = state.session.take().map(|s| s.id).unwrap_or_default();
                return vec![session_end(ended, "expired"), failure(id, "no_session")];
            }
            if self.activity.last() > current.quiet_since_ms {
                let ended = state.session.take().map(|s| s.id).unwrap_or_default();
                log::info(&format!("session {ended} ended: the user took over"));
                return vec![
                    session_end(ended, "user_active"),
                    failure(id, "user_active"),
                ];
            }
        }
        if action.validate().is_err() {
            return refuse("invalid");
        }

        let platform = self.platform.clone();
        let deny = self.policy.deny_apps.clone();
        let settle = self.policy.settle;
        let outcome = blocking(move || Ok(run(&*platform, &deny, settle, action)))
            .await
            .unwrap_or_else(|err| Err(err.into()));

        let mut frames = Vec::new();
        let mut state = self.state.lock();
        match outcome {
            Ok((display, screenshot)) => {
                let screen = screen_of(&display);
                if state.announced != Some(screen) {
                    state.announced = Some(screen);
                    frames.push(DeviceFrame::Hello(hello(screen)));
                }
                frames.push(DeviceFrame::Result(ActionResult {
                    id: id.to_owned(),
                    ok: true,
                    error: None,
                    screenshot: Some(screenshot),
                }));
            }
            Err(Refusal(code)) => frames.push(failure(id, code)),
        }
        frames
    }
}

struct Refusal(&'static str);

impl From<PlatformError> for Refusal {
    fn from(err: PlatformError) -> Self {
        log::warn(&format!("platform: {err}"));
        Refusal("failed")
    }
}

/// The blocking part of an action: check, perform, settle, capture.
fn run(
    platform: &dyn Platform,
    deny: &[String],
    settle: Duration,
    action: Action,
) -> Result<(Display, Screenshot), Refusal> {
    let display = platform.display()?;
    let shot_size = shot::fit(display.pixels.0, display.pixels.1);
    let at = |x: u32, y: u32| shot::to_display(x, y, shot_size, display.bounds);
    let denied = |app: &Option<String>| app.as_ref().is_some_and(|a| deny.contains(a));
    let windows = platform.windows();
    // The app whose window is topmost at a point gets the click.
    let target_denied = |(x, y): (f64, f64)| {
        windows
            .iter()
            .find(|w| w.bounds.contains(x, y))
            .is_some_and(|w| denied(&w.app))
    };

    let input = match action {
        Action::Screenshot => None,
        Action::Wait { ms } => {
            std::thread::sleep(Duration::from_millis(ms));
            None
        }
        Action::Move { x, y } => Some(Input::Move { to: at(x, y) }),
        Action::Click {
            x,
            y,
            button,
            count,
        } => Some(Input::Click {
            at: at(x, y),
            button: match button {
                Button::Left => MouseButton::Left,
                Button::Right => MouseButton::Right,
                Button::Middle => MouseButton::Middle,
            },
            count,
        }),
        Action::Drag { from, to } => Some(Input::Drag {
            from: at(from.x, from.y),
            to: at(to.x, to.y),
        }),
        Action::Scroll { x, y, dx, dy } => Some(Input::Scroll {
            at: at(x, y),
            dx,
            dy,
        }),
        Action::Type { text } => Some(Input::Type { text }),
        Action::Key { keys } => Some(Input::Key {
            chord: keys::parse(&keys).map_err(|_| Refusal("invalid"))?,
        }),
    };

    if let Some(input) = &input {
        let points: Vec<(f64, f64)> = match input {
            Input::Click { at, .. } | Input::Scroll { at, .. } => vec![*at],
            Input::Drag { from, to } => vec![*from, *to],
            Input::Move { .. } | Input::Type { .. } | Input::Key { .. } => Vec::new(),
        };
        let moves_only = matches!(input, Input::Move { .. });
        if !moves_only && denied(&platform.frontmost_app()) {
            return Err(Refusal("denied_app"));
        }
        if points.into_iter().any(target_denied) {
            return Err(Refusal("denied_app"));
        }
        if matches!(input, Input::Type { .. }) && platform.secure_input() {
            return Err(Refusal("secure_input"));
        }
        platform.perform(input)?;
        std::thread::sleep(settle);
    }

    let native = platform.capture()?;
    // Window bounds are in points; the capture is in the display's pixels.
    let scale_x = f64::from(native.width()) / display.bounds.width.max(1.0);
    let scale_y = f64::from(native.height()) / display.bounds.height.max(1.0);
    let hidden: Vec<Rect> = platform
        .windows()
        .into_iter()
        .filter(|w| denied(&w.app))
        .map(|w| Rect {
            x: (w.bounds.x - display.bounds.x) * scale_x,
            y: (w.bounds.y - display.bounds.y) * scale_y,
            width: w.bounds.width * scale_x,
            height: w.bounds.height * scale_y,
        })
        .collect();
    let screenshot = shot::encode(native, &hidden, shot_size).map_err(PlatformError)?;
    Ok((display, screenshot))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, PlatformError> + Send + 'static,
) -> Result<T, PlatformError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| PlatformError(format!("worker: {err}")))?
}

fn screen_of(display: &Display) -> Screen {
    Screen {
        width: display.pixels.0,
        height: display.pixels.1,
        scale: display.scale,
    }
}

fn hello(screen: Screen) -> Hello {
    Hello {
        protocol: PROTOCOL_VERSION,
        agent: concat!("kura-device ", env!("CARGO_PKG_VERSION")).into(),
        platform: std::env::consts::OS.into(),
        screen,
    }
}

fn failure(id: &str, code: &str) -> DeviceFrame {
    DeviceFrame::Result(ActionResult {
        id: id.to_owned(),
        ok: false,
        error: Some(code.to_owned()),
        screenshot: None,
    })
}

fn denied(session: String, reason: &str) -> DeviceFrame {
    DeviceFrame::SessionDenied {
        session,
        reason: reason.into(),
    }
}

fn session_end(session: String, reason: &str) -> DeviceFrame {
    DeviceFrame::SessionEnd {
        session,
        reason: reason.into(),
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}
