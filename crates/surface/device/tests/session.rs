//! The device's session rules, against a fake computer, and the device
//! against the cell's own hub and connection loop (no network): what one
//! side writes, the other must read.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine as _;
use futures::channel::mpsc as channel;
use image::{Rgba, RgbaImage};
use kura_device::agent::{Activity, Agent, Clock, Policy};
use kura_device::keys::{Key, Modifiers};
use kura_device::link::{self, Ended, LinkConfig};
use kura_device::platform::{
    Answer, Consent, Display, Input, MouseButton, Performed, Platform, PlatformError, Stop, Window,
};
use kura_device::shot::Rect;
use kura_device_protocol::{Action, ActionResult, Button, DeviceFrame, Point};
use kura_devices::{ConnectionConfig, DeviceError, Hub, HubConfig};
use kura_store::DeviceRecord;
use parking_lot::Mutex;
use tokio::sync::mpsc;

const T0: u64 = 1_800_000_000_000;

type StepHook = Arc<dyn Fn(usize) + Send + Sync>;

#[derive(Default)]
struct Fake {
    windows: Mutex<Vec<Window>>,
    frontmost: Mutex<Option<String>>,
    secure: Mutex<bool>,
    performed: Mutex<Vec<Input>>,
    /// Characters a `type` posted, one step each, so a test can stop it
    /// part way.
    typed: Mutex<String>,
    /// Called before each typed character with its index.
    on_step: Mutex<Option<StepHook>>,
}

impl Platform for Fake {
    fn display(&self) -> Result<Display, PlatformError> {
        Ok(Display {
            pixels: (2880, 1800),
            scale: 2,
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
        })
    }

    fn capture(&self) -> Result<RgbaImage, PlatformError> {
        Ok(RgbaImage::from_pixel(
            2880,
            1800,
            Rgba([255, 255, 255, 255]),
        ))
    }

    fn frontmost_app(&self) -> Option<String> {
        self.frontmost.lock().clone()
    }

    fn windows(&self) -> Vec<Window> {
        self.windows.lock().clone()
    }

    fn secure_input(&self) -> bool {
        *self.secure.lock()
    }

    fn perform(&self, input: &Input, stop: Stop<'_>) -> Result<Performed, PlatformError> {
        self.performed.lock().push(input.clone());
        if let Input::Type { text } = input {
            let on_step = self.on_step.lock().clone();
            for (index, c) in text.chars().enumerate() {
                if let Some(on_step) = &on_step {
                    on_step(index);
                }
                if stop() {
                    return Ok(Performed::Stopped);
                }
                self.typed.lock().push(c);
            }
        }
        Ok(Performed::Done)
    }
}

struct Person {
    answer: Mutex<Answer>,
    asked: AtomicU64,
    /// Never answers: the prompt stays up until it is withdrawn.
    away: AtomicBool,
    withdrawn: AtomicU64,
}

impl Consent for Person {
    fn ask(&self, _reason: &str, _ttl_s: u64, _timeout: Duration, cancel: Stop<'_>) -> Answer {
        self.asked.fetch_add(1, Ordering::Relaxed);
        if self.away.load(Ordering::Relaxed) {
            let give_up = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < give_up {
                if cancel() {
                    self.withdrawn.fetch_add(1, Ordering::Relaxed);
                    return Answer::Deny;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            return Answer::Timeout;
        }
        *self.answer.lock()
    }
}

struct Rig {
    agent: Arc<Agent>,
    fake: Arc<Fake>,
    person: Arc<Person>,
    activity: Arc<Activity>,
    now: Arc<AtomicU64>,
}

fn rig() -> Rig {
    let fake = Arc::new(Fake::default());
    let person = Arc::new(Person {
        answer: Mutex::new(Answer::Allow),
        asked: AtomicU64::new(0),
        away: AtomicBool::new(false),
        withdrawn: AtomicU64::new(0),
    });
    let activity = Arc::new(Activity::default());
    let now = Arc::new(AtomicU64::new(T0));
    let clock: Clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::Relaxed))
    };
    let policy = Policy {
        settle: Duration::ZERO,
        ..Policy::default()
    };
    let agent = Arc::new(Agent::new(
        fake.clone(),
        person.clone(),
        activity.clone(),
        policy,
        clock,
    ));
    Rig {
        agent,
        fake,
        person,
        activity,
        now,
    }
}

fn result(frames: &[DeviceFrame]) -> &ActionResult {
    frames
        .iter()
        .find_map(|frame| match frame {
            DeviceFrame::Result(result) => Some(result),
            _ => None,
        })
        .expect("a result")
}

fn click(x: u32, y: u32) -> Action {
    Action::Click {
        x,
        y,
        button: Button::Left,
        count: 1,
    }
}

async fn granted(rig: &Rig) -> String {
    rig.agent.hello().await.unwrap();
    let frame = rig.agent.request("ses_1".into(), "test".into(), 600).await;
    assert!(
        matches!(frame, DeviceFrame::SessionGranted { .. }),
        "{frame:?}"
    );
    "ses_1".into()
}

#[tokio::test]
async fn nothing_happens_outside_a_granted_session() {
    let rig = rig();
    rig.agent.hello().await.unwrap();
    let frames = rig
        .agent
        .act("a1".into(), "ses_x".into(), click(1, 1))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("no_session"));

    *rig.person.answer.lock() = Answer::Deny;
    let frame = rig.agent.request("ses_1".into(), "test".into(), 600).await;
    assert_eq!(
        frame,
        DeviceFrame::SessionDenied {
            session: "ses_1".into(),
            reason: "user_denied".into()
        }
    );
    *rig.person.answer.lock() = Answer::Timeout;
    let frame = rig.agent.request("ses_2".into(), "test".into(), 600).await;
    assert!(matches!(frame, DeviceFrame::SessionDenied { reason, .. } if reason == "timeout"));
    let frames = rig
        .agent
        .act("a2".into(), "ses_1".into(), click(1, 1))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("no_session"));
    assert!(rig.fake.performed.lock().is_empty());
}

#[tokio::test]
async fn a_click_lands_in_display_points_and_answers_with_a_screenshot() {
    let rig = rig();
    let session = granted(&rig).await;
    let frames = rig.agent.act("a1".into(), session, click(640, 400)).await;
    let result = result(&frames);
    assert!(result.ok, "{result:?}");
    let shot = result.screenshot.as_ref().unwrap();
    assert_eq!((shot.width, shot.height), (1280, 800));
    assert_eq!(
        rig.fake.performed.lock().as_slice(),
        [Input::Click {
            at: (720.0, 450.0),
            button: MouseButton::Left,
            count: 1
        }]
    );
}

#[tokio::test]
async fn the_person_touching_the_computer_ends_the_session() {
    let rig = rig();
    let session = granted(&rig).await;
    // Moving the hand away right after clicking Allow does not count.
    rig.activity.touched(T0 + 1_000);
    let frames = rig
        .agent
        .act("a1".into(), session.clone(), click(1, 1))
        .await;
    assert!(result(&frames).ok);

    rig.now.store(T0 + 10_000, Ordering::Relaxed);
    rig.activity.touched(T0 + 9_000);
    let frames = rig
        .agent
        .act("a2".into(), session.clone(), click(1, 1))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("user_active"));
    assert!(frames.contains(&DeviceFrame::SessionEnd {
        session: session.clone(),
        reason: "user_active".into()
    }));
    // The session is gone: the agent has to ask again.
    let frames = rig.agent.act("a3".into(), session, click(1, 1)).await;
    assert_eq!(result(&frames).error.as_deref(), Some("no_session"));
    assert_eq!(rig.fake.performed.lock().len(), 1);
}

#[tokio::test]
async fn the_kill_switch_ends_the_session_and_refuses_new_ones_for_a_while() {
    let rig = rig();
    let session = granted(&rig).await;
    assert_eq!(
        rig.agent.kill(),
        Some(DeviceFrame::SessionEnd {
            session: session.clone(),
            reason: "user_stopped".into()
        })
    );
    let asked = rig.person.asked.load(Ordering::Relaxed);
    let frame = rig.agent.request("ses_2".into(), "again".into(), 600).await;
    assert!(matches!(frame, DeviceFrame::SessionDenied { reason, .. } if reason == "kill_switch"));
    assert_eq!(
        rig.person.asked.load(Ordering::Relaxed),
        asked,
        "not even asked"
    );
    // Ten minutes later the person is asked again.
    rig.now.store(T0 + 10 * 60_000 + 1, Ordering::Relaxed);
    let frame = rig.agent.request("ses_3".into(), "again".into(), 600).await;
    assert!(matches!(frame, DeviceFrame::SessionGranted { .. }));
}

#[tokio::test]
async fn sessions_expire_on_the_device_whatever_the_cell_asked() {
    let rig = rig();
    rig.agent.hello().await.unwrap();
    // Asked for two hours; the device grants at most thirty minutes.
    let DeviceFrame::SessionGranted { until_ms, .. } =
        rig.agent.request("ses_1".into(), "long".into(), 7200).await
    else {
        panic!("not granted");
    };
    assert_eq!(until_ms, T0 + 30 * 60_000);
    rig.now.store(until_ms, Ordering::Relaxed);
    let frames = rig
        .agent
        .act("a1".into(), "ses_1".into(), click(1, 1))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("no_session"));
    assert!(frames.contains(&DeviceFrame::SessionEnd {
        session: "ses_1".into(),
        reason: "expired".into()
    }));
}

#[tokio::test]
async fn never_touch_apps_are_not_clicked_typed_into_or_shown() {
    let rig = rig();
    let session = granted(&rig).await;
    // A password manager window over the left half of the display.
    rig.fake.windows.lock().push(Window {
        app: Some("com.1password.1password".into()),
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            width: 720.0,
            height: 900.0,
        },
    });
    let frames = rig
        .agent
        .act("a1".into(), session.clone(), click(100, 100))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("denied_app"));
    let drag = Action::Drag {
        from: Point { x: 1000, y: 100 },
        to: Point { x: 100, y: 100 },
    };
    let frames = rig.agent.act("a2".into(), session.clone(), drag).await;
    assert_eq!(result(&frames).error.as_deref(), Some("denied_app"));
    // Elsewhere is fine, and the window is black in the screenshot.
    let frames = rig
        .agent
        .act("a3".into(), session.clone(), click(1000, 100))
        .await;
    let shot = result(&frames).screenshot.clone().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot.data)
        .unwrap();
    let image = image::load_from_memory(&bytes).unwrap().into_rgb8();
    assert!(image.get_pixel(300, 400).0.iter().all(|v| *v < 20));
    assert!(image.get_pixel(1000, 400).0.iter().all(|v| *v > 235));

    // With it in front, nothing is typed or pressed, wherever the pointer is.
    *rig.fake.frontmost.lock() = Some("com.1password.1password".into());
    let frames = rig
        .agent
        .act(
            "a4".into(),
            session.clone(),
            Action::Type { text: "x".into() },
        )
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("denied_app"));
    let frames = rig
        .agent
        .act(
            "a5".into(),
            session,
            Action::Key {
                keys: "cmd+v".into(),
            },
        )
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("denied_app"));
    assert_eq!(rig.fake.performed.lock().len(), 1);
}

#[tokio::test]
async fn secure_input_blocks_typing_but_not_keys_and_bad_actions_are_invalid() {
    let rig = rig();
    let session = granted(&rig).await;
    *rig.fake.secure.lock() = true;
    let frames = rig
        .agent
        .act(
            "a1".into(),
            session.clone(),
            Action::Type {
                text: "hunter2".into(),
            },
        )
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("secure_input"));
    let frames = rig
        .agent
        .act(
            "a2".into(),
            session.clone(),
            Action::Key {
                keys: "Return".into(),
            },
        )
        .await;
    assert!(result(&frames).ok);
    assert_eq!(
        rig.fake.performed.lock().as_slice(),
        [Input::Key {
            chord: kura_device::keys::Chord {
                modifiers: Modifiers::default(),
                key: Key::Return
            }
        }]
    );
    // The device checks the limits again, whatever the cell checked.
    for (id, action) in [
        (
            "a3",
            Action::Key {
                keys: "cmd+launch".into(),
            },
        ),
        ("a4", Action::Wait { ms: 60_000 }),
        (
            "a5",
            Action::Click {
                x: 1,
                y: 1,
                button: Button::Left,
                count: 9,
            },
        ),
    ] {
        let frames = rig.agent.act(id.into(), session.clone(), action).await;
        assert_eq!(result(&frames).error.as_deref(), Some("invalid"), "{id}");
    }
}

/// The device's [`link::session`] against the cell's `serve` and `Hub`,
/// joined by in-memory channels.
fn typing(text: &str) -> Action {
    Action::Type { text: text.into() }
}

#[tokio::test]
async fn the_kill_switch_stops_an_action_part_way() {
    let rig = rig();
    let session = granted(&rig).await;
    let agent = Arc::downgrade(&rig.agent);
    let killed = Arc::new(Mutex::new(None));
    *rig.fake.on_step.lock() = Some({
        let killed = killed.clone();
        Arc::new(move |index| {
            if index == 3 {
                *killed.lock() = agent.upgrade().and_then(|agent| agent.kill());
            }
        })
    });
    let frames = rig
        .agent
        .act("a1".into(), session.clone(), typing("abcdefgh"))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("user_stopped"));
    assert_eq!(
        *rig.fake.typed.lock(),
        "abc",
        "nothing after the kill switch"
    );
    assert_eq!(
        *killed.lock(),
        Some(DeviceFrame::SessionEnd {
            session,
            reason: "user_stopped".into()
        })
    );
}

#[tokio::test]
async fn taking_over_stops_an_action_part_way_and_ends_the_session() {
    let rig = rig();
    let session = granted(&rig).await;
    rig.now.store(T0 + 10_000, Ordering::Relaxed);
    *rig.fake.on_step.lock() = Some({
        let activity = rig.activity.clone();
        Arc::new(move |index| {
            if index == 2 {
                activity.touched(T0 + 10_000);
            }
        })
    });
    let frames = rig
        .agent
        .act("a1".into(), session.clone(), typing("hello"))
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("user_active"));
    assert_eq!(
        frames.first(),
        Some(&DeviceFrame::SessionEnd {
            session: session.clone(),
            reason: "user_active".into()
        }),
        "the end is announced before the result"
    );
    assert_eq!(*rig.fake.typed.lock(), "he");
    let frames = rig.agent.act("a2".into(), session, click(1, 1)).await;
    assert_eq!(result(&frames).error.as_deref(), Some("no_session"));
}

#[tokio::test]
async fn a_wait_ends_as_soon_as_the_person_takes_over() {
    let rig = rig();
    let session = granted(&rig).await;
    rig.now.store(T0 + 10_000, Ordering::Relaxed);
    let activity = rig.activity.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        activity.touched(T0 + 10_000);
    });
    let started = std::time::Instant::now();
    let frames = rig
        .agent
        .act("a1".into(), session, Action::Wait { ms: 5000 })
        .await;
    assert_eq!(result(&frames).error.as_deref(), Some("user_active"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_prompt_nobody_can_answer_is_withdrawn() {
    let rig = rig();
    rig.agent.hello().await.unwrap();
    rig.person.away.store(true, Ordering::Relaxed);

    // The connection drops while the prompt is up.
    let asking = {
        let agent = rig.agent.clone();
        tokio::spawn(async move { agent.request("ses_1".into(), "r".into(), 600).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.agent.disconnected();
    let frame = asking.await.unwrap();
    assert!(
        matches!(&frame, DeviceFrame::SessionDenied { reason, .. } if reason == "cancelled"),
        "{frame:?}"
    );
    assert_eq!(rig.person.withdrawn.load(Ordering::Relaxed), 1);

    // A newer request replaces the open one.
    let first = {
        let agent = rig.agent.clone();
        tokio::spawn(async move { agent.request("ses_2".into(), "r".into(), 600).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.person.away.store(false, Ordering::Relaxed);
    let second = rig.agent.request("ses_3".into(), "r".into(), 600).await;
    assert!(
        matches!(second, DeviceFrame::SessionGranted { .. }),
        "{second:?}"
    );
    let first = first.await.unwrap();
    assert!(
        matches!(&first, DeviceFrame::SessionDenied { reason, .. } if reason == "cancelled"),
        "{first:?}"
    );

    // The kill switch takes the prompt down and says why.
    rig.person.away.store(true, Ordering::Relaxed);
    let asking = {
        let agent = rig.agent.clone();
        tokio::spawn(async move { agent.request("ses_4".into(), "r".into(), 600).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.agent.kill();
    let frame = asking.await.unwrap();
    assert!(
        matches!(&frame, DeviceFrame::SessionDenied { reason, .. } if reason == "kill_switch"),
        "{frame:?}"
    );
    assert_eq!(rig.person.withdrawn.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn the_device_speaks_the_cells_protocol() {
    let rig = rig();
    let hub = Arc::new(Hub::new(HubConfig::default()));
    let record = DeviceRecord {
        device_id: "dev_1".into(),
        tenant_id: "ten_a".into(),
        principal_id: "prn_1".into(),
        name: "mac".into(),
        platform: "macos".into(),
        token_hash: "h".into(),
        created_at: chrono::Utc::now(),
        last_seen_at: None,
        revoked_at: None,
    };
    let (to_cell, from_device) = channel::unbounded::<String>();
    let (to_device, from_cell) = channel::unbounded::<String>();
    let cell = {
        let hub = hub.clone();
        tokio::spawn(async move {
            kura_devices::serve(
                hub,
                &record,
                from_device,
                to_device,
                &ConnectionConfig::default(),
            )
            .await
        })
    };
    let (_kill_tx, mut kills) = mpsc::channel(1);
    let device = {
        let agent = rig.agent.clone();
        tokio::spawn(async move {
            link::session(
                &agent,
                from_cell,
                to_cell,
                &LinkConfig::default(),
                &mut kills,
            )
            .await
        })
    };
    for _ in 0..100 {
        if !hub.online("ten_a").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(hub.online("ten_a").len(), 1, "the device said hello");

    // The cell's session `until` is the device's, so its expiry check uses
    // real time; the rig's clock is pinned, so move it to now.
    rig.now.store(
        chrono::Utc::now().timestamp_millis() as u64,
        Ordering::Relaxed,
    );
    let session = hub
        .start_session("ten_a", "dev_1", "open the report", Duration::from_secs(60))
        .await
        .unwrap();
    let result = hub
        .act("ten_a", "dev_1", &session, click(640, 400))
        .await
        .unwrap();
    assert!(result.ok, "{result:?}");
    assert_eq!(result.screenshot.unwrap().width, 1280);
    let result = hub
        .act(
            "ten_a",
            "dev_1",
            &session,
            Action::Type { text: "hi".into() },
        )
        .await
        .unwrap();
    assert!(result.ok);

    // The person takes over: the cell hears the session ended.
    rig.activity
        .touched(chrono::Utc::now().timestamp_millis() as u64 + 60_000);
    let ended = hub
        .act("ten_a", "dev_1", &session, Action::Screenshot)
        .await;
    assert_eq!(ended, Err(DeviceError::SessionEnded("user_active".into())));
    let again = hub
        .act("ten_a", "dev_1", &session, Action::Screenshot)
        .await;
    assert_eq!(again, Err(DeviceError::NoSession));

    // A denied request is a denial on the cell's side too.
    *rig.person.answer.lock() = Answer::Deny;
    let denied = hub
        .start_session("ten_a", "dev_1", "again", Duration::from_secs(60))
        .await;
    assert!(matches!(denied, Err(DeviceError::Denied(_))), "{denied:?}");

    // Revocation closes the connection; the device sees it end.
    hub.revoke("dev_1");
    let ended = tokio::time::timeout(Duration::from_secs(5), device)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(ended, Ended::Lost(_)), "{ended:?}");
    let _ = cell.await;
}
