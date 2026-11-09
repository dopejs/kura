//! The cell's own rules for remote computer use
//! (docs/harness/remote-computer-use.md §8, "In the cell").
//!
//! The device protects the person whatever the cell asks; these rules are
//! the cell's share: which tenants may use the feature at all, and which
//! actions a person must approve before the device ever sees them. Screen
//! content is data, never instructions, but a model can still be talked
//! into "press cmd+q" by what it reads; the approval is what blunts that.

use std::collections::BTreeSet;

use kura_device_protocol::Action;
use kura_device_protocol::keys::{self, Key};

/// Which tenants are offered the `computer.*` tools
/// (`entries.computer-use.config.remote`). Off unless configured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RemoteGate {
    #[default]
    Off,
    /// Every tenant: a single-user daemon.
    AllTenants,
    /// Only these, on a daemon that hosts many.
    Tenants(BTreeSet<String>),
}

impl RemoteGate {
    #[must_use]
    pub fn allows(&self, tenant_id: &str) -> bool {
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return false;
        }
        match self {
            Self::Off => false,
            Self::AllTenants => true,
            Self::Tenants(tenants) => tenants.contains(tenant_id),
        }
    }
}

/// Why an action needs a person's approval first, or `None`.
///
/// `after_url` says the last text typed in this session looked like a URL,
/// so a following Return would navigate to it.
#[must_use]
pub fn approval_reason(action: &Action, after_url: bool) -> Option<String> {
    match action {
        Action::Key { keys } => {
            // A chord the device cannot parse is refused there as `invalid`;
            // the parser is the same one, so nothing slips between the two.
            let chord = keys::parse(keys).ok()?;
            let m = chord.modifiers;
            let reason = match chord.key {
                Key::Char('q') if m.cmd => "quits the app (or logs out)",
                Key::Char('w') if m.cmd => "closes the window or tab",
                Key::Backspace | Key::ForwardDelete if m.cmd => {
                    "deletes the selection (in Finder: moves files to the Trash or empties it)"
                }
                Key::Escape if m.cmd && m.opt => "opens Force Quit",
                Key::Return if after_url && !m.cmd && !m.ctrl && !m.opt => {
                    "opens the address that was just typed"
                }
                _ => return None,
            };
            Some(format!("press {}: {reason}", keys.trim()))
        }
        Action::Type { text } => {
            let (before, _) = text.split_once(['\n', '\r'])?;
            let typed_url = looks_like_url(before) || (before.trim().is_empty() && after_url);
            typed_url.then(|| "type an address and press Return, which opens it".to_string())
        }
        _ => None,
    }
}

/// Whether typed text ends in something a browser would open: a URL, a
/// `www.` name, or a bare domain on a common top-level domain.
#[must_use]
pub fn looks_like_url(text: &str) -> bool {
    let Some(token) = text.split_whitespace().last() else {
        return false;
    };
    let token = token.to_ascii_lowercase();
    if token.contains("://") || token.starts_with("www.") {
        return true;
    }
    let host = token.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    let labels: Vec<&str> = host.split('.').collect();
    let well_formed = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    well_formed && labels.last().is_some_and(|tld| COMMON_TLDS.contains(tld))
}

/// Enough to catch an address typed into a browser; a false negative only
/// means the device's own rules are the last line, as for any other click.
const COMMON_TLDS: &[&str] = &[
    "com", "net", "org", "io", "dev", "app", "ai", "co", "me", "info", "xyz", "site", "online",
    "cn", "jp", "uk", "de", "fr", "ru", "sg", "hk", "tw", "us", "ca", "au", "in", "br", "ly", "gg",
    "sh", "to", "tv",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn key(keys: &str) -> Action {
        Action::Key { keys: keys.into() }
    }

    fn typed(text: &str) -> Action {
        Action::Type { text: text.into() }
    }

    #[test]
    fn the_gate_is_off_by_default_and_names_tenants() {
        assert!(!RemoteGate::default().allows("ten_a"));
        assert!(RemoteGate::AllTenants.allows("ten_a"));
        assert!(!RemoteGate::AllTenants.allows(" "));
        let some = RemoteGate::Tenants(["ten_a".to_string()].into());
        assert!(some.allows("ten_a"));
        assert!(!some.allows("ten_b"));
    }

    #[test]
    fn risky_chords_need_approval_in_every_dialect() {
        for chord in [
            "cmd+q",
            "Command+Q",
            "super+q",
            "cmd+opt+q",
            "cmd+w",
            "cmd+shift+w",
            "cmd+delete",
            "cmd+Backspace",
            "cmd+shift+delete",
            "cmd+opt+shift+BackSpace",
            "cmd+opt+esc",
            "option+command+Escape",
        ] {
            assert!(approval_reason(&key(chord), false).is_some(), "{chord}");
        }
        for chord in [
            "cmd+c",
            "cmd+v",
            "cmd+t",
            "q",
            "Return",
            "cmd+esc",
            "cmd+launchrockets",
        ] {
            assert!(approval_reason(&key(chord), false).is_none(), "{chord}");
        }
    }

    #[test]
    fn return_after_an_address_needs_approval() {
        assert!(approval_reason(&key("Return"), true).is_some());
        assert!(approval_reason(&key("enter"), true).is_some());
        assert!(approval_reason(&key("cmd+Return"), true).is_none());
        assert!(approval_reason(&typed("https://evil.example/x?d=1\n"), false).is_some());
        assert!(approval_reason(&typed("example.com\n"), false).is_some());
        assert!(approval_reason(&typed("\n"), true).is_some());
        assert!(approval_reason(&typed("hello world\n"), false).is_none());
        assert!(approval_reason(&typed("https://example.com"), false).is_none());
        assert!(approval_reason(&Action::Screenshot, true).is_none());
    }

    #[test]
    fn urls_are_told_from_prose() {
        for url in [
            "https://kura.example.com",
            "go to www.example.org",
            "example.com/path?q=1",
            "api.github.io:443",
            "HTTP://X",
        ] {
            assert!(looks_like_url(url), "{url}");
        }
        for text in [
            "hello",
            "notes.txt",
            "v1.2",
            "end of sentence.",
            "",
            "a..com",
            "e.g.",
        ] {
            assert!(!looks_like_url(text), "{text}");
        }
    }
}
