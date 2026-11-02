//! Tool names as the wire accepts them.
//!
//! Kura names its tools by namespace (`memory.lookup`, `browser.open`,
//! `web.search`). The OpenAI and Anthropic APIs accept only
//! `[a-zA-Z0-9_-]`, 1–64 characters, and refuse the whole request
//! otherwise, so every turn that offered a tool failed against them; the fakes
//! in tests never checked. Each client sends the wire form and turns the
//! names in the model's calls back.
//!
//! The mapping depends only on the tool list, in order, so a turn's rounds
//! send byte-identical tool definitions and the prompt cache (2.3) still
//! applies. A name that is already valid is sent unchanged.

use std::collections::HashMap;

use futures::StreamExt;
use futures::stream::{BoxStream, Stream};

use crate::provider::{ProviderError, ResponseEvent, ToolSpec};

const MAX_LEN: usize = 64;

#[derive(Debug, Default)]
pub(crate) struct WireNames {
    to_wire: HashMap<String, String>,
    from_wire: HashMap<String, String>,
}

impl WireNames {
    pub(crate) fn new(tools: &[ToolSpec]) -> Self {
        let mut names = Self::default();
        for tool in tools {
            if names.to_wire.contains_key(&tool.name) {
                continue;
            }
            let base = sanitize(&tool.name);
            let mut wire = base.clone();
            let mut n = 2;
            // Two names that sanitize alike (`a.b`, `a_b`) stay apart.
            while names.from_wire.contains_key(&wire) {
                let suffix = format!("_{n}");
                let keep = base
                    .char_indices()
                    .nth(MAX_LEN - suffix.len())
                    .map_or(base.len(), |(i, _)| i);
                wire = format!("{}{suffix}", &base[..keep]);
                n += 1;
            }
            names.to_wire.insert(tool.name.clone(), wire.clone());
            names.from_wire.insert(wire, tool.name.clone());
        }
        names
    }

    /// What goes on the wire for `name`. A name not in the tool list (a call
    /// from earlier in the history to a tool no longer offered) is sanitized
    /// the same way.
    pub(crate) fn wire(&self, name: &str) -> String {
        self.to_wire
            .get(name)
            .cloned()
            .unwrap_or_else(|| sanitize(name))
    }

    /// The model's call, with Kura's name for the tool.
    pub(crate) fn restore(&self, event: ResponseEvent) -> ResponseEvent {
        match event {
            ResponseEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => ResponseEvent::FunctionCall {
                call_id,
                name: self.from_wire.get(&name).cloned().unwrap_or(name),
                arguments,
            },
            other => other,
        }
    }
}

/// `events` with the model's calls named as Kura named the tools.
pub(crate) fn restore_names<'a>(
    names: WireNames,
    events: impl Stream<Item = Result<ResponseEvent, ProviderError>> + Send + 'a,
) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
    Box::pin(events.map(move |event| event.map(|event| names.restore(event))))
}

fn sanitize(name: &str) -> String {
    let mut wire: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_LEN)
        .collect();
    if wire.is_empty() {
        wire.push_str("tool");
    }
    wire
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    fn call(name: &str) -> ResponseEvent {
        ResponseEvent::FunctionCall {
            call_id: "c".into(),
            name: name.into(),
            arguments: "{}".into(),
        }
    }

    #[test]
    fn namespaced_names_go_out_valid_and_come_back_as_they_were() {
        let names = WireNames::new(&[
            tool("memory.lookup"),
            tool("get_weather"),
            tool("a.b"),
            tool("a_b"),
        ]);
        assert_eq!(names.wire("memory.lookup"), "memory_lookup");
        assert_eq!(
            names.wire("get_weather"),
            "get_weather",
            "valid names are unchanged"
        );
        assert_eq!(names.wire("a.b"), "a_b");
        assert_eq!(
            names.wire("a_b"),
            "a_b_2",
            "names that sanitize alike stay apart"
        );
        assert_eq!(names.restore(call("memory_lookup")), call("memory.lookup"));
        assert_eq!(names.restore(call("a_b_2")), call("a_b"));
        assert_eq!(names.restore(call("unknown")), call("unknown"));
        // A tool no longer offered, from history, is still sent valid.
        assert_eq!(names.wire("browser.open"), "browser_open");
    }

    #[test]
    fn every_wire_name_matches_the_api_pattern() {
        let long = "x.".repeat(60);
        let names = WireNames::new(&[
            tool(&long),
            tool(&long.replace('.', "_")),
            tool(""),
            tool("ünï.cødé"),
        ]);
        for name in [
            long.clone(),
            long.replace('.', "_"),
            String::new(),
            "ünï.cødé".to_string(),
        ] {
            let wire = names.wire(&name);
            assert!(
                (1..=MAX_LEN).contains(&wire.len())
                    && wire
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{name:?} -> {wire:?}"
            );
        }
        assert_ne!(names.wire(&long), names.wire(&long.replace('.', "_")));
    }

    #[test]
    fn the_mapping_is_the_same_for_the_same_tool_list() {
        let tools = [
            tool("browser.open"),
            tool("browser_open"),
            tool("memory.lookup"),
        ];
        let (a, b) = (WireNames::new(&tools), WireNames::new(&tools));
        for tool in &tools {
            assert_eq!(a.wire(&tool.name), b.wire(&tool.name));
        }
    }
}
