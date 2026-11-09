//! macOS: Quartz events for input, `screencapture(1)` for the screen, the
//! window server's window list for never-touch apps, a CoreFoundation alert
//! for consent, and a listen-only event tap that sees the person's own
//! input and the kill-switch chord (docs/harness/remote-computer-use.md §9).
//!
//! Capture shells out to `/usr/sbin/screencapture` rather than binding
//! ScreenCaptureKit: Apple keeps it on whatever capture API the OS uses
//! (`CGDisplayCreateImage` is obsolete), it honours the Screen Recording
//! permission the same way, and it keeps async Objective-C out of the
//! device. The cost is one process per screenshot; behind [`Platform`] it
//! can be swapped without touching the session logic.
//!
//! Every event the device posts carries [`OWN_EVENT_TAG`], so the tap can
//! tell the agent's input from the person's.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use image::RgbaImage;
use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{
    CFDictionary, CFMachPort, CFNumber, CFRetained, CFRunLoop, CFString, CFType, CGPoint, CGRect,
    kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGEventField, CGEventFlags,
    CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMainDisplayID, CGMouseButton,
    CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestScreenCaptureAccess, CGScrollEventUnit, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
};
use tokio::sync::mpsc;

use super::{
    Answer, Consent, Display, Input, MouseButton, Performed, Platform, PlatformError, Stop, Window,
};
use crate::agent::Activity;
use crate::keys::{Chord, Key};
use crate::log;
use crate::shot::Rect;

/// Marks events the device posts ("KURA").
const OWN_EVENT_TAG: i64 = 0x4b55_5241;
/// Most UTF-16 units one keyboard event carries reliably.
const TYPE_CHUNK: usize = 16;
/// Wheel lines one scroll may move, either way.
const MAX_SCROLL_LINES: i32 = 100;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

/// Whether this process may post input events (Accessibility).
#[must_use]
pub fn input_allowed() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Whether this process may capture the screen (Screen Recording).
#[must_use]
pub fn capture_allowed() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Adds this process to the Screen Recording list so the person only has to
/// tick it. Returns whether access is already granted.
pub fn request_capture() -> bool {
    CGRequestScreenCaptureAccess()
}

pub const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const SCREEN_RECORDING_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

pub struct Mac {
    scratch: PathBuf,
    counter: AtomicU64,
}

impl Mac {
    /// `scratch` holds each capture for the moment it takes to read it; it
    /// is created private to the user.
    pub fn new(scratch: PathBuf) -> Result<Self, PlatformError> {
        std::fs::create_dir_all(&scratch)
            .map_err(|err| PlatformError(format!("{}: {err}", scratch.display())))?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))
                .map_err(|err| PlatformError(format!("{}: {err}", scratch.display())))?;
        }
        Ok(Self {
            scratch,
            counter: AtomicU64::new(0),
        })
    }

    fn source() -> Result<CFRetained<CGEventSource>, PlatformError> {
        // A private source: the person's held modifiers do not leak into the
        // agent's keystrokes.
        CGEventSource::new(CGEventSourceStateID::Private)
            .ok_or_else(|| PlatformError("no event source".into()))
    }

    fn post(event: Option<CFRetained<CGEvent>>) -> Result<(), PlatformError> {
        let event = event.ok_or_else(|| PlatformError("could not create an event".into()))?;
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            OWN_EVENT_TAG,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    fn mouse(
        source: &CGEventSource,
        kind: CGEventType,
        at: (f64, f64),
        button: CGMouseButton,
        clicks: i64,
    ) -> Result<(), PlatformError> {
        let event = CGEvent::new_mouse_event(Some(source), kind, point(at), button);
        if let (Some(event), true) = (&event, clicks > 0) {
            CGEvent::set_integer_value_field(
                Some(event),
                CGEventField::MouseEventClickState,
                clicks,
            );
        }
        Self::post(event)
    }

    fn key(
        source: &CGEventSource,
        code: u16,
        flags: CGEventFlags,
        down: bool,
    ) -> Result<(), PlatformError> {
        let event = CGEvent::new_keyboard_event(Some(source), code, down);
        if let Some(event) = &event {
            CGEvent::set_flags(Some(event), flags);
        }
        Self::post(event)
    }

    fn press(source: &CGEventSource, code: u16, flags: CGEventFlags) -> Result<(), PlatformError> {
        Self::key(source, code, flags, true)?;
        Self::key(source, code, flags, false)
    }

    /// Types `text` chunk by chunk, asking `stop` before each one.
    fn type_text(
        source: &CGEventSource,
        text: &str,
        stop: Stop<'_>,
    ) -> Result<Performed, PlatformError> {
        let mut chunk: Vec<u16> = Vec::with_capacity(TYPE_CHUNK);
        let flush = |chunk: &mut Vec<u16>| -> Result<bool, PlatformError> {
            if chunk.is_empty() {
                return Ok(true);
            }
            if stop() {
                return Ok(false);
            }
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(Some(source), 0, down);
                if let Some(event) = &event {
                    CGEvent::set_flags(Some(event), CGEventFlags::empty());
                    // SAFETY: the pointer and length describe `chunk`, alive
                    // for the call.
                    unsafe {
                        CGEvent::keyboard_set_unicode_string(
                            Some(event),
                            chunk.len() as _,
                            chunk.as_ptr(),
                        );
                    }
                }
                Self::post(event)?;
            }
            chunk.clear();
            std::thread::sleep(Duration::from_millis(5));
            Ok(true)
        };
        for c in text.chars() {
            // Return and Tab are keys, not characters, to the apps that
            // handle them (forms, terminals, editors).
            let special = match c {
                '\n' => Some(KEY_RETURN),
                '\t' => Some(KEY_TAB),
                '\r' => continue,
                _ => None,
            };
            if let Some(code) = special {
                if !flush(&mut chunk)? || stop() {
                    return Ok(Performed::Stopped);
                }
                Self::press(source, code, CGEventFlags::empty())?;
                continue;
            }
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            if chunk.len() + units.len() > TYPE_CHUNK && !flush(&mut chunk)? {
                return Ok(Performed::Stopped);
            }
            chunk.extend_from_slice(units);
        }
        Ok(if flush(&mut chunk)? {
            Performed::Done
        } else {
            Performed::Stopped
        })
    }
}

impl Platform for Mac {
    fn display(&self) -> Result<Display, PlatformError> {
        let id = CGMainDisplayID();
        let bounds = CGDisplayBounds(id);
        let mode = CGDisplayCopyDisplayMode(id)
            .ok_or_else(|| PlatformError("no display mode for the main display".into()))?;
        let pixels = (
            CGDisplayMode::pixel_width(Some(&mode)) as u32,
            CGDisplayMode::pixel_height(Some(&mode)) as u32,
        );
        let scale = if bounds.size.width > 0.0 {
            (f64::from(pixels.0) / bounds.size.width).round().max(1.0) as u32
        } else {
            1
        };
        Ok(Display {
            pixels,
            scale,
            bounds: rect(bounds),
        })
    }

    fn capture(&self) -> Result<RgbaImage, PlatformError> {
        if !capture_allowed() {
            // Without the permission screencapture silently returns the
            // wallpaper; an agent acting on that would be acting blind.
            return Err(PlatformError("screen recording permission missing".into()));
        }
        let path = self.scratch.join(format!(
            "shot-{}-{}.png",
            std::process::id(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        ));
        let status = std::process::Command::new("/usr/sbin/screencapture")
            // -x silent, -C with the pointer, -D1 the main display.
            .args(["-x", "-C", "-D1", "-t", "png"])
            .arg(&path)
            .status()
            .map_err(|err| PlatformError(format!("screencapture: {err}")))?;
        let image = if status.success() {
            image::open(&path)
                .map(image::DynamicImage::into_rgba8)
                .map_err(|err| PlatformError(format!("screencapture output: {err}")))
        } else {
            Err(PlatformError(format!("screencapture: {status}")))
        };
        let _ = std::fs::remove_file(&path);
        image
    }

    fn frontmost_app(&self) -> Option<String> {
        // The owner of the topmost normal window is the app in front. The
        // window list is queried fresh each time; NSWorkspace's cached
        // frontmost app goes stale without a main run loop.
        let mut names = HashMap::new();
        window_list()
            .into_iter()
            .find(|(_, layer, _)| *layer == 0)
            .and_then(|(pid, _, _)| bundle_id(pid, &mut names))
    }

    fn windows(&self) -> Vec<Window> {
        let mut names = HashMap::new();
        window_list()
            .into_iter()
            .map(|(pid, _, bounds)| Window {
                app: bundle_id(pid, &mut names),
                bounds,
            })
            .collect()
    }

    fn secure_input(&self) -> bool {
        unsafe { IsSecureEventInputEnabled() != 0 }
    }

    fn perform(&self, input: &Input, stop: Stop<'_>) -> Result<Performed, PlatformError> {
        if !input_allowed() {
            return Err(PlatformError("accessibility permission missing".into()));
        }
        let source = Self::source()?;
        let done = |posted: Result<(), PlatformError>| posted.map(|()| Performed::Done);
        match input {
            Input::Move { to } => done(Self::mouse(
                &source,
                CGEventType::MouseMoved,
                *to,
                CGMouseButton::Left,
                0,
            )),
            Input::Click { at, button, count } => {
                let (down, up, button) = match button {
                    MouseButton::Left => (
                        CGEventType::LeftMouseDown,
                        CGEventType::LeftMouseUp,
                        CGMouseButton::Left,
                    ),
                    MouseButton::Right => (
                        CGEventType::RightMouseDown,
                        CGEventType::RightMouseUp,
                        CGMouseButton::Right,
                    ),
                    MouseButton::Middle => (
                        CGEventType::OtherMouseDown,
                        CGEventType::OtherMouseUp,
                        CGMouseButton::Center,
                    ),
                };
                Self::mouse(&source, CGEventType::MouseMoved, *at, button, 0)?;
                std::thread::sleep(Duration::from_millis(30));
                for n in 1..=i64::from(*count) {
                    // Each click is a down and its up: stopping between
                    // clicks never leaves the button held.
                    if stop() {
                        return Ok(Performed::Stopped);
                    }
                    Self::mouse(&source, down, *at, button, n)?;
                    Self::mouse(&source, up, *at, button, n)?;
                }
                Ok(Performed::Done)
            }
            Input::Drag { from, to } => {
                let left = CGMouseButton::Left;
                Self::mouse(&source, CGEventType::MouseMoved, *from, left, 0)?;
                Self::mouse(&source, CGEventType::LeftMouseDown, *from, left, 1)?;
                // Intermediate points: many apps only start a drag after
                // seeing movement with the button held.
                const STEPS: u32 = 12;
                let mut at = *from;
                for step in 1..=STEPS {
                    if stop() {
                        // Let go where the pointer is: a button left down
                        // would drag whatever the person touches next.
                        Self::mouse(&source, CGEventType::LeftMouseUp, at, left, 1)?;
                        return Ok(Performed::Stopped);
                    }
                    let t = f64::from(step) / f64::from(STEPS);
                    at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                    std::thread::sleep(Duration::from_millis(15));
                    Self::mouse(&source, CGEventType::LeftMouseDragged, at, left, 1)?;
                }
                done(Self::mouse(&source, CGEventType::LeftMouseUp, *to, left, 1))
            }
            Input::Scroll { at, dx, dy } => {
                Self::mouse(
                    &source,
                    CGEventType::MouseMoved,
                    *at,
                    CGMouseButton::Left,
                    0,
                )?;
                let lines = |v: i32| -v.clamp(-MAX_SCROLL_LINES, MAX_SCROLL_LINES);
                done(Self::post(CGEvent::new_scroll_wheel_event2(
                    Some(&source),
                    CGScrollEventUnit::Line,
                    2,
                    lines(*dy),
                    lines(*dx),
                    0,
                )))
            }
            Input::Type { text } => Self::type_text(&source, text, stop),
            Input::Key { chord } => {
                let (code, flags) = keycode(chord);
                done(Self::press(&source, code, flags))
            }
        }
    }
}

/// `(pid, layer, bounds)` for each on-screen window, front to back.
fn window_list() -> Vec<(i32, i64, Rect)> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: the window server documents each entry as a dictionary keyed
    // by strings.
    let list = unsafe { list.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    let number = |dict: &CFDictionary<CFString, CFType>, key: &CFString| {
        dict.get(key)
            .and_then(|value| value.downcast::<CFNumber>().ok())
            .and_then(|n| n.as_i64())
    };
    let mut windows = Vec::new();
    for dict in list.iter() {
        let (Some(pid), Some(layer)) = (
            number(&dict, unsafe { kCGWindowOwnerPID }),
            number(&dict, unsafe { kCGWindowLayer }),
        ) else {
            continue;
        };
        let Some(bounds) = dict
            .get(unsafe { kCGWindowBounds })
            .and_then(|value| value.downcast::<CFDictionary>().ok())
        else {
            continue;
        };
        let mut cg = CGRect::default();
        // SAFETY: `bounds` is the window server's rect dictionary and `cg`
        // is a valid out pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut cg) } {
            continue;
        }
        windows.push((pid as i32, layer, rect(cg)));
    }
    windows
}

fn bundle_id(pid: i32, cache: &mut HashMap<i32, Option<String>>) -> Option<String> {
    cache
        .entry(pid)
        .or_insert_with(|| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .and_then(|app| app.bundleIdentifier())
                .map(|id| id.to_string())
        })
        .clone()
}

fn point((x, y): (f64, f64)) -> CGPoint {
    CGPoint { x, y }
}

fn rect(r: CGRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}

// Virtual key codes (HIToolbox Events.h, ANSI layout).
const KEY_RETURN: u16 = 0x24;
const KEY_TAB: u16 = 0x30;
const KEY_PERIOD: u16 = 0x2F;

fn keycode(chord: &Chord) -> (u16, CGEventFlags) {
    let mut flags = CGEventFlags::empty();
    let m = chord.modifiers;
    for (on, flag) in [
        (m.cmd, CGEventFlags::MaskCommand),
        (m.ctrl, CGEventFlags::MaskControl),
        (m.opt, CGEventFlags::MaskAlternate),
        (m.shift, CGEventFlags::MaskShift),
        (m.func, CGEventFlags::MaskSecondaryFn),
    ] {
        if on {
            flags |= flag;
        }
    }
    let code = match chord.key {
        Key::Return => KEY_RETURN,
        Key::Tab => KEY_TAB,
        Key::Space => 0x31,
        Key::Escape => 0x35,
        Key::Backspace => 0x33,
        Key::ForwardDelete => 0x75,
        Key::Up => 0x7E,
        Key::Down => 0x7D,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
        Key::Home => 0x73,
        Key::End => 0x77,
        Key::PageUp => 0x74,
        Key::PageDown => 0x79,
        Key::F(n) => [
            0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B,
            0x71, 0x6A, 0x40, 0x4F, 0x50, 0x5A,
        ][usize::from(n - 1)],
        Key::Char('+') => {
            flags |= CGEventFlags::MaskShift;
            0x18
        }
        Key::Char(c) => char_code(c),
    };
    (code, flags)
}

fn char_code(c: char) -> u16 {
    match c {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '6' => 0x16,
        '5' => 0x17,
        '=' => 0x18,
        '9' => 0x19,
        '7' => 0x1A,
        '-' => 0x1B,
        '8' => 0x1C,
        '0' => 0x1D,
        ']' => 0x1E,
        'o' => 0x1F,
        'u' => 0x20,
        '[' => 0x21,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        '\'' => 0x27,
        'k' => 0x28,
        ';' => 0x29,
        '\\' => 0x2A,
        ',' => 0x2B,
        '/' => 0x2C,
        'n' => 0x2D,
        'm' => 0x2E,
        '.' => KEY_PERIOD,
        '`' => 0x32,
        // `keys::parse` admits only the characters above.
        _ => 0x31,
    }
}

/// The consent prompt: a system alert that needs no app bundle. "Deny" is
/// the default button, so Return or Escape never grants control.
///
/// The alert is created and waited on from one thread (Core Foundation does
/// not promise `CFUserNotification` is safe to share), a quarter second at a
/// time, so that `cancel` can take it off the screen from that same thread.
pub struct Alert;

/// How long one wait for the person's answer lasts before `cancel` is asked.
const ALERT_POLL: Duration = Duration::from_millis(250);

impl Consent for Alert {
    fn ask(&self, reason: &str, ttl_s: u64, timeout: Duration, cancel: Stop<'_>) -> Answer {
        use objc2_core_foundation::{
            CFUserNotification, kCFUserNotificationAlertHeaderKey,
            kCFUserNotificationAlertMessageKey, kCFUserNotificationAlternateButtonTitleKey,
            kCFUserNotificationDefaultButtonTitleKey,
        };
        let header = CFString::from_str("Kura wants to control this Mac");
        let message = CFString::from_str(&format!(
            "Reason: {reason}\n\nFor up to {} minutes. Touching the mouse or keyboard, or \
             pressing Control-Option-Command-Period, takes control back at once.",
            ttl_s.div_ceil(60).max(1)
        ));
        let deny = CFString::from_str("Deny");
        let allow = CFString::from_str("Allow");
        // SAFETY: the keys are Core Foundation's own constant strings.
        let keys = unsafe {
            [
                kCFUserNotificationAlertHeaderKey,
                kCFUserNotificationAlertMessageKey,
                kCFUserNotificationDefaultButtonTitleKey,
                kCFUserNotificationAlternateButtonTitleKey,
            ]
        };
        let Some(keys) = keys.into_iter().collect::<Option<Vec<&CFString>>>() else {
            log::warn("consent: alert keys unavailable");
            return Answer::Deny;
        };
        let values: [&CFString; 4] = [&header, &message, &deny, &allow];
        let dictionary = CFDictionary::<CFString, CFString>::from_slices(&keys, &values);
        // kCFUserNotificationCautionAlertLevel
        const CAUTION: usize = 2;
        let mut error: i32 = 0;
        // SAFETY: `error` is a valid out pointer; the dictionary outlives
        // the call.
        let alert = unsafe {
            CFUserNotification::new(
                None,
                timeout.as_secs_f64(),
                CAUTION as _,
                &mut error,
                Some(dictionary.as_opaque()),
            )
        };
        let Some(alert) = alert.filter(|_| error == 0) else {
            log::warn(&format!("consent: the alert could not be shown ({error})"));
            return Answer::Deny;
        };
        // The alert dismisses itself at `timeout`; this bound only guards
        // against a reply that never comes.
        let give_up = std::time::Instant::now() + timeout + Duration::from_secs(5);
        loop {
            let mut response: usize = 0;
            // SAFETY: `response` is a valid out pointer.
            let status = unsafe {
                alert.receive_response(
                    ALERT_POLL.as_secs_f64(),
                    &mut response as *mut usize as *mut _,
                )
            };
            if status == 0 {
                // kCFUserNotification{Default,Alternate,Other,Cancel}Response;
                // a timeout answers Cancel.
                return match response & 0x3 {
                    1 => Answer::Allow,
                    3 => Answer::Timeout,
                    _ => Answer::Deny,
                };
            }
            if cancel() {
                alert.cancel();
                return Answer::Deny;
            }
            if std::time::Instant::now() >= give_up {
                alert.cancel();
                return Answer::Timeout;
            }
        }
    }
}

struct Watch {
    activity: Arc<Activity>,
    kills: mpsc::Sender<()>,
    tap: AtomicU64,
}

/// Starts the input watcher on its own thread. Fails when the tap cannot
/// be created (no Accessibility permission): without it neither the kill
/// switch nor take-over detection works, so the device must not run.
pub fn watch(activity: Arc<Activity>, kills: mpsc::Sender<()>) -> Result<(), PlatformError> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("kura-device-watch".into())
        .spawn(move || {
            let watch: &'static Watch = Box::leak(Box::new(Watch {
                activity,
                kills,
                tap: AtomicU64::new(0),
            }));
            let mask: u64 = [
                CGEventType::MouseMoved,
                CGEventType::LeftMouseDown,
                CGEventType::RightMouseDown,
                CGEventType::OtherMouseDown,
                CGEventType::LeftMouseDragged,
                CGEventType::RightMouseDragged,
                CGEventType::KeyDown,
                CGEventType::ScrollWheel,
            ]
            .iter()
            .fold(0, |mask, kind| mask | (1u64 << kind.0));
            // SAFETY: `on_event` matches the callback type and `watch` is
            // leaked, so it outlives the tap.
            let tap = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::SessionEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    mask,
                    Some(on_event),
                    watch as *const Watch as *mut c_void,
                )
            };
            let Some(tap) = tap else {
                let _ = ready_tx.send(false);
                return;
            };
            watch
                .tap
                .store(CFRetained::as_ptr(&tap).as_ptr() as u64, Ordering::Release);
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                let _ = ready_tx.send(false);
                return;
            };
            let Some(run_loop) = CFRunLoop::current() else {
                let _ = ready_tx.send(false);
                return;
            };
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            CGEvent::tap_enable(&tap, true);
            let _ = ready_tx.send(true);
            CFRunLoop::run();
        })
        .map_err(|err| PlatformError(format!("watch thread: {err}")))?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => Ok(()),
        _ => Err(PlatformError(
            "cannot watch the keyboard and mouse: grant Accessibility (kura-device doctor)".into(),
        )),
    }
}

unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `info` is the leaked `Watch` given to `tap_create`.
    let watch = unsafe { &*(info as *const Watch) };
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        // The system turns a slow tap off; turn it back on.
        let tap = watch.tap.load(Ordering::Acquire) as *const CFMachPort;
        if let Some(tap) = unsafe { tap.as_ref() } {
            CGEvent::tap_enable(tap, true);
        }
        return event.as_ptr();
    }
    // SAFETY: the system hands the callback a live event.
    let event_ref = unsafe { event.as_ref() };
    if CGEvent::integer_value_field(Some(event_ref), CGEventField::EventSourceUserData)
        == OWN_EVENT_TAG
    {
        return event.as_ptr();
    }
    if kind == CGEventType::KeyDown {
        let code =
            CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode);
        let flags = CGEvent::flags(Some(event_ref));
        let chord =
            CGEventFlags::MaskControl | CGEventFlags::MaskAlternate | CGEventFlags::MaskCommand;
        if code == i64::from(KEY_PERIOD) && flags.contains(chord) {
            let _ = watch.kills.try_send(());
        }
    }
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    watch.activity.touched(now);
    event.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_map_to_ansi_key_codes() {
        let chord = crate::keys::parse("ctrl+opt+cmd+.").unwrap();
        let (code, flags) = keycode(&chord);
        assert_eq!(code, KEY_PERIOD);
        assert!(flags.contains(
            CGEventFlags::MaskControl | CGEventFlags::MaskAlternate | CGEventFlags::MaskCommand
        ));
        assert_eq!(keycode(&crate::keys::parse("cmd++").unwrap()).0, 0x18);
        assert!(
            keycode(&crate::keys::parse("cmd++").unwrap())
                .1
                .contains(CGEventFlags::MaskShift)
        );
        assert_eq!(keycode(&crate::keys::parse("F12").unwrap()).0, 0x6F);
        assert_eq!(
            keycode(&crate::keys::parse("Return").unwrap()).0,
            KEY_RETURN
        );
    }

    /// Reads the real display, windows and screen; posts no input. Needs
    /// the Screen Recording and Accessibility permissions, so it is run by
    /// hand: `cargo test -p kura-device -- --ignored macos_reads`.
    #[test]
    #[ignore = "shows an alert for about a second in a logged-in macOS session"]
    fn the_consent_alert_waits_in_short_polls_and_can_be_withdrawn() {
        // Each 250 ms wait that times out must leave the alert up: were it
        // dismissed, the next wait would answer at once (as Cancel, read as
        // Timeout) instead of the withdrawal answering Deny after a second.
        let started = std::time::Instant::now();
        let withdraw = || started.elapsed() >= Duration::from_secs(1);
        let answer = Alert.ask(
            "kura-device self-test: this alert closes by itself",
            60,
            Duration::from_secs(30),
            &withdraw,
        );
        let waited = started.elapsed();
        eprintln!("answer {answer:?} after {waited:?}");
        assert_eq!(answer, Answer::Deny);
        assert!(waited >= Duration::from_millis(900), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
    }

    #[test]
    #[ignore = "needs a logged-in macOS session with permissions"]
    fn macos_reads_the_real_computer() {
        let scratch =
            std::env::temp_dir().join(format!("kura-device-smoke-{}", std::process::id()));
        let mac = Mac::new(scratch.clone()).unwrap();
        let display = mac.display().unwrap();
        eprintln!("display: {display:?}");
        assert!(display.pixels.0 > 0 && display.bounds.width > 0.0);
        let windows = mac.windows();
        eprintln!(
            "windows: {} on screen, {} with a bundle id; frontmost: {:?}; secure input: {}",
            windows.len(),
            windows.iter().filter(|w| w.app.is_some()).count(),
            mac.frontmost_app(),
            mac.secure_input()
        );
        let started = std::time::Instant::now();
        let native = mac.capture().unwrap();
        let captured = started.elapsed();
        let size = crate::shot::fit(display.pixels.0, display.pixels.1);
        let shot = crate::shot::encode(native, &[], size).unwrap();
        eprintln!(
            "capture {}ms, total {}ms, {}x{}, {} KB base64",
            captured.as_millis(),
            started.elapsed().as_millis(),
            shot.width,
            shot.height,
            shot.data.len() / 1024
        );
        assert!(
            std::fs::read_dir(&scratch).unwrap().next().is_none(),
            "no capture left behind"
        );
        let _ = std::fs::remove_dir_all(&scratch);
        let (tx, _rx) = mpsc::channel(1);
        watch(Arc::new(Activity::default()), tx).unwrap();
    }
}
