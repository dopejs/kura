//! What the device needs from the operating system, behind one trait so
//! the session logic is tested with a fake and other platforms are
//! additive (docs/harness/remote-computer-use.md §9).
//!
//! Every method blocks; the agent calls them on the blocking pool.

use std::fmt;

use image::RgbaImage;

use crate::keys::Chord;
use crate::shot::Rect;

#[cfg(target_os = "macos")]
pub mod macos;

/// The main display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    /// Native pixels, the size a capture comes back in.
    pub pixels: (u32, u32),
    /// Native pixels per point.
    pub scale: u32,
    /// Where the display is in global points, the space input events use.
    pub bounds: Rect,
}

/// A window on screen, in global points.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The owning app's bundle id, when it has one.
    pub app: Option<String>,
    pub bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// One input, already in global points.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Click {
        at: (f64, f64),
        button: MouseButton,
        count: u8,
    },
    Move {
        to: (f64, f64),
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
    },
    Type {
        text: String,
    },
    Key {
        chord: Chord,
    },
    /// Wheel lines; positive `dy` scrolls the content down (as a person
    /// scrolling toward the end of a page).
    Scroll {
        at: (f64, f64),
        dx: i32,
        dy: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError(pub String);

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlatformError {}

pub trait Platform: Send + Sync + 'static {
    fn display(&self) -> Result<Display, PlatformError>;
    /// The main display at native resolution.
    fn capture(&self) -> Result<RgbaImage, PlatformError>;
    /// The bundle id of the app that receives keystrokes.
    fn frontmost_app(&self) -> Option<String>;
    /// Windows on screen, front to back.
    fn windows(&self) -> Vec<Window>;
    /// Whether a password field (or anything else) has turned on secure
    /// keyboard entry.
    fn secure_input(&self) -> bool;
    fn perform(&self, input: &Input) -> Result<(), PlatformError>;
}

/// The person's answer to a request for control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
    /// No answer in time; treated as a refusal.
    Timeout,
}

/// Asks the person at the computer whether the agent may take control.
pub trait Consent: Send + Sync + 'static {
    /// Blocks until the person answers or `timeout` passes. Only an explicit
    /// allow is [`Answer::Allow`].
    fn ask(&self, reason: &str, ttl_s: u64, timeout: std::time::Duration) -> Answer;
}
