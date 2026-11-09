//! Key chords as the agent writes them (`cmd+shift+t`, `Return`,
//! `ctrl+opt+cmd+.`), parsed into modifiers and one key.
//!
//! Models write chords in several dialects (xdotool's `Page_Down` and
//! `super`, macOS's `command` and `option`), so names are matched loosely:
//! case, `_` and `-` are ignored. Anything unknown is refused rather than
//! guessed, because a wrong key on someone's computer is worse than an error.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub cmd: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
    pub func: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable key on the ANSI layout: `a`–`z`, `0`–`9` and punctuation.
    Char(char),
    Return,
    Tab,
    Space,
    Escape,
    Backspace,
    ForwardDelete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Modifiers,
    pub key: Key,
}

/// Parses one chord. Exactly one non-modifier key is required.
pub fn parse(chord: &str) -> Result<Chord, String> {
    let chord = chord.trim();
    // `cmd++` presses plus; split would see an empty last part.
    let (head, plus) = match chord.strip_suffix("++") {
        Some(head) => (head, true),
        None if chord == "+" => ("", true),
        None => (chord, false),
    };
    let mut parts: Vec<&str> = if head.is_empty() {
        Vec::new()
    } else {
        head.split('+').map(str::trim).collect()
    };
    if plus {
        parts.push("+");
    }
    let Some((last, mods)) = parts.split_last() else {
        return Err("no key".into());
    };
    let mut modifiers = Modifiers::default();
    for part in mods {
        let flag = match normalize(part).as_str() {
            "cmd" | "command" | "super" | "meta" | "win" => &mut modifiers.cmd,
            "ctrl" | "control" => &mut modifiers.ctrl,
            "opt" | "option" | "alt" => &mut modifiers.opt,
            "shift" => &mut modifiers.shift,
            "fn" => &mut modifiers.func,
            _ => return Err(format!("unknown modifier `{part}`")),
        };
        *flag = true;
    }
    let key = key(last).ok_or_else(|| format!("unknown key `{last}`"))?;
    Ok(Chord { modifiers, key })
}

fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let c = c.to_ascii_lowercase();
        return (c.is_ascii_alphanumeric() || "`-=[]\\;',./+".contains(c)).then_some(Key::Char(c));
    }
    let name = normalize(name);
    Some(match name.as_str() {
        "return" | "enter" | "kpenter" => Key::Return,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "escape" | "esc" => Key::Escape,
        "backspace" | "delete" => Key::Backspace,
        "forwarddelete" | "del" => Key::ForwardDelete,
        "up" | "arrowup" => Key::Up,
        "down" | "arrowdown" => Key::Down,
        "left" | "arrowleft" => Key::Left,
        "right" | "arrowright" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "prior" => Key::PageUp,
        "pagedown" | "next" => Key::PageDown,
        "plus" => Key::Char('+'),
        "minus" => Key::Char('-'),
        "period" => Key::Char('.'),
        "comma" => Key::Char(','),
        "slash" => Key::Char('/'),
        _ => {
            let n: u8 = name.strip_prefix('f')?.parse().ok()?;
            if !(1..=20).contains(&n) {
                return None;
            }
            Key::F(n)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(cmd: bool, ctrl: bool, opt: bool, shift: bool, key: Key) -> Chord {
        Chord {
            modifiers: Modifiers {
                cmd,
                ctrl,
                opt,
                shift,
                func: false,
            },
            key,
        }
    }

    #[test]
    fn dialects_parse_to_the_same_chord() {
        let want = chord(true, false, false, true, Key::Char('t'));
        for text in [
            "cmd+shift+t",
            "Command+Shift+T",
            "super+shift+t",
            " meta + shift + t ",
        ] {
            assert_eq!(parse(text), Ok(want), "{text}");
        }
        assert_eq!(
            parse("Page_Down"),
            Ok(chord(false, false, false, false, Key::PageDown))
        );
        assert_eq!(
            parse("Return"),
            Ok(chord(false, false, false, false, Key::Return))
        );
        assert_eq!(
            parse("ctrl+opt+cmd+."),
            Ok(chord(true, true, true, false, Key::Char('.')))
        );
        assert_eq!(
            parse("cmd++"),
            Ok(chord(true, false, false, false, Key::Char('+')))
        );
        assert_eq!(
            parse("+"),
            Ok(chord(false, false, false, false, Key::Char('+')))
        );
        assert_eq!(
            parse("F12"),
            Ok(chord(false, false, false, false, Key::F(12)))
        );
    }

    #[test]
    fn unknown_names_are_refused() {
        assert!(parse("").is_err());
        assert!(parse("cmd+").is_err());
        assert!(parse("hyper+a").is_err());
        assert!(parse("cmd+launchrockets").is_err());
        assert!(parse("f99").is_err());
        assert!(parse("é").is_err());
    }
}
