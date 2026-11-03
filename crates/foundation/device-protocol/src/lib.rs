//! The wire between a tenant's cell and `kura-device` on the user's
//! computer (docs/harness/remote-computer-use.md §5).
//!
//! JSON text frames over one WebSocket the device opens. Both sides build
//! from this crate, so a frame one side writes the other can read; it holds
//! nothing but the shapes and their limits, and depends only on serde.
//!
//! The cell asks for a control session, the user grants or denies it on the
//! device, and while it lasts the cell sends one action at a time; each
//! answer carries a fresh screenshot. Either side may end a session.

use serde::{Deserialize, Serialize};

/// Bumped on any change a peer on the previous version could misread. The
/// cell refuses a `hello` with another version rather than guess.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest text one `type` action may carry.
pub const MAX_TYPE_CHARS: usize = 2000;
/// Longest one `wait` action may last.
pub const MAX_WAIT_MS: u64 = 5000;
/// Most clicks one `click` action may make (a triple click selects a line).
pub const MAX_CLICKS: u8 = 3;
/// Longest a chord in `key` may be, in characters, e.g. `cmd+shift+t`.
pub const MAX_KEYS_CHARS: usize = 64;

/// A frame the device sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceFrame {
    /// First frame on every connection, and again when the display changes.
    Hello(Hello),
    /// The user allowed the session, until `until_ms` (Unix ms) at the latest.
    SessionGranted {
        session: String,
        until_ms: u64,
    },
    /// The user refused, or did not answer in time.
    SessionDenied {
        session: String,
        reason: String,
    },
    /// The outcome of one action.
    Result(ActionResult),
    /// The device ended the session: the user stopped it, took over, or it
    /// ran out of time.
    SessionEnd {
        session: String,
        reason: String,
    },
    Ping,
    Pong,
}

/// A frame the cell sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CellFrame {
    /// The cell accepted the `hello`; names the device as the cell knows it.
    Welcome {
        protocol: u32,
        device_id: String,
    },
    /// The agent wants control. The device asks the user and answers with
    /// `session_granted` or `session_denied`.
    SessionRequest {
        session: String,
        reason: String,
        ttl_s: u64,
    },
    /// One action inside a granted session.
    Action {
        id: String,
        session: String,
        action: Action,
    },
    /// The cell ended the session: the turn is over, or the device was
    /// revoked.
    SessionEnd {
        session: String,
        reason: String,
    },
    Ping,
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The device's own build, for logs and support.
    pub agent: String,
    /// `macos`, …
    pub platform: String,
    /// The main display, in native pixels.
    pub screen: Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    /// Native pixels per point (2 on a Retina display).
    pub scale: u32,
}

/// What the agent asks the computer to do. Coordinates are in the pixel
/// space of the last screenshot; the device maps them to the display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    Screenshot,
    Click {
        x: u32,
        y: u32,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u8,
    },
    Move {
        x: u32,
        y: u32,
    },
    Drag {
        from: Point,
        to: Point,
    },
    Type {
        text: String,
    },
    Key {
        keys: String,
    },
    Scroll {
        x: u32,
        y: u32,
        dx: i32,
        dy: i32,
    },
    Wait {
        ms: u64,
    },
}

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Action {
    /// A short, bounded name for logs and metric labels.
    #[must_use]
    pub fn op(&self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Click { .. } => "click",
            Self::Move { .. } => "move",
            Self::Drag { .. } => "drag",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Scroll { .. } => "scroll",
            Self::Wait { .. } => "wait",
        }
    }

    /// Checks the limits both sides enforce. The cell checks before it
    /// sends, so a bad call fails at once; the device checks again, because
    /// it does not trust the cell with the user's computer.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Click { count, .. } if !(1..=MAX_CLICKS).contains(count) => {
                Err(format!("count must be 1 to {MAX_CLICKS}"))
            }
            Self::Type { text } if text.is_empty() => Err("text is empty".into()),
            Self::Type { text } if text.chars().count() > MAX_TYPE_CHARS => {
                Err(format!("text is longer than {MAX_TYPE_CHARS} characters"))
            }
            Self::Key { keys } if keys.trim().is_empty() => Err("keys is empty".into()),
            Self::Key { keys } if keys.chars().count() > MAX_KEYS_CHARS => {
                Err(format!("keys is longer than {MAX_KEYS_CHARS} characters"))
            }
            Self::Wait { ms } if *ms > MAX_WAIT_MS => {
                Err(format!("wait is longer than {MAX_WAIT_MS} ms"))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub id: String,
    pub ok: bool,
    /// Why the action failed, as a stable code the agent can act on:
    /// `user_active`, `denied_app`, `secure_input`, `invalid`, `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The screen after the action settled, present on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<Screenshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    /// `image/jpeg` or `image/png`.
    pub media_type: String,
    /// The screenshot's own size, the space the agent's coordinates use.
    pub width: u32,
    pub height: u32,
    /// Base64, without a `data:` prefix.
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_have_the_documented_shape() {
        let action = CellFrame::Action {
            id: "a1".into(),
            session: "s1".into(),
            action: Action::Click {
                x: 10,
                y: 20,
                button: Button::Left,
                count: 2,
            },
        };
        assert_eq!(
            serde_json::to_value(&action).unwrap(),
            json!({"type": "action", "id": "a1", "session": "s1",
                   "action": {"op": "click", "x": 10, "y": 20, "button": "left", "count": 2}})
        );
        // Defaults: a plain click is one left click.
        let parsed: Action =
            serde_json::from_value(json!({"op": "click", "x": 1, "y": 2})).unwrap();
        assert_eq!(
            parsed,
            Action::Click {
                x: 1,
                y: 2,
                button: Button::Left,
                count: 1
            }
        );

        let result: DeviceFrame = serde_json::from_value(json!({
            "type": "result", "id": "a1", "ok": true,
            "screenshot": {"media_type": "image/jpeg", "width": 1280, "height": 800, "data": "AA"},
        }))
        .unwrap();
        let DeviceFrame::Result(result) = result else {
            panic!("not a result");
        };
        assert!(result.ok && result.error.is_none());
        assert_eq!(result.screenshot.unwrap().width, 1280);

        let hello = DeviceFrame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            agent: "kura-device 0.1".into(),
            platform: "macos".into(),
            screen: Screen {
                width: 2880,
                height: 1800,
                scale: 2,
            },
        });
        let text = serde_json::to_string(&hello).unwrap();
        assert_eq!(serde_json::from_str::<DeviceFrame>(&text).unwrap(), hello);
    }

    #[test]
    fn limits_are_checked() {
        assert!(
            Action::Click {
                x: 0,
                y: 0,
                button: Button::Left,
                count: 4
            }
            .validate()
            .is_err()
        );
        assert!(
            Action::Type {
                text: String::new()
            }
            .validate()
            .is_err()
        );
        assert!(
            Action::Type {
                text: "x".repeat(MAX_TYPE_CHARS + 1)
            }
            .validate()
            .is_err()
        );
        assert!(Action::Type { text: "hi".into() }.validate().is_ok());
        assert!(Action::Key { keys: " ".into() }.validate().is_err());
        assert!(
            Action::Wait {
                ms: MAX_WAIT_MS + 1
            }
            .validate()
            .is_err()
        );
        assert!(Action::Wait { ms: MAX_WAIT_MS }.validate().is_ok());
        assert_eq!(Action::Screenshot.op(), "screenshot");
    }
}
