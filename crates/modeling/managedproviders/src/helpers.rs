//! Small path / string / clone helpers ported from `helpers.go` and the
//! free functions in `bridges.go`.

use std::collections::{HashMap, HashSet};

use kura_llm::Message;

/// Go `baseName`: `filepath.Base` of the trimmed value, or "" when the
/// trimmed value is empty. The fallback keeps root-like values ("/") intact,
/// matching `filepath.Base` behavior for the CLI-path use cases.
#[must_use]
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| trimmed.to_string())
}

/// Go `filepath.Join`: joins path segments with the platform separator.
/// (Note: Go also `Clean`s the result; this port normalizes only the join,
/// which is sufficient for the fixed relative segments used here.)
#[must_use]
pub fn filepath_join(parts: &[&str]) -> String {
    let mut path = std::path::PathBuf::new();
    for part in parts {
        path.push(part);
    }
    path.to_string_lossy().into_owned()
}

/// Go `resolvePath`: "~" and "~/..." expand against `home_dir`; empty stays
/// empty; everything else is returned verbatim.
#[must_use]
pub fn resolve_path(home_dir: &str, value: &str) -> String {
    let trimmed = value.trim();
    match trimmed {
        "" => String::new(),
        "~" => home_dir.to_string(),
        _ if trimmed.starts_with("~/") => {
            filepath_join(&[home_dir, trimmed.trim_start_matches("~/")])
        }
        _ => trimmed.to_string(),
    }
}

/// Go `homeFallbackWorkdir`.
#[must_use]
pub fn home_fallback_workdir(home_dir: &str) -> String {
    if home_dir.trim().is_empty() {
        ".".to_string()
    } else {
        home_dir.to_string()
    }
}

/// Go `firstNonEmpty`: the first value whose trimmed form is non-empty (the
/// original, untrimmed value), else "".
#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).to_string())
        .unwrap_or_default()
}

/// Go `nowPtr`.
#[must_use]
pub fn now_ptr(value: chrono::DateTime<chrono::Utc>) -> Option<chrono::DateTime<chrono::Utc>> {
    Some(value)
}

/// Go `firstAvailablePath`: the explicit value when set, otherwise the first
/// candidate found on `PATH` (a light `exec.LookPath`).
#[must_use]
pub fn first_available_path(explicit: &str, candidates: &[&str]) -> String {
    if !explicit.trim().is_empty() {
        return explicit.to_string();
    }
    for candidate in candidates {
        if look_path(candidate) {
            return (*candidate).to_string();
        }
    }
    String::new()
}

#[must_use]
fn look_path(name: &str) -> bool {
    let candidate = std::path::Path::new(name);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return is_executable_file(candidate);
    }
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| is_executable_file(&dir.join(name)))
}

#[must_use]
fn is_executable_file(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The current user's home directory (Go `os.UserHomeDir`).
#[must_use]
pub fn user_home_dir() -> Option<String> {
    #[cfg(unix)]
    let candidate = std::env::var("HOME").ok();
    #[cfg(windows)]
    let candidate = std::env::var("USERPROFILE").ok();
    #[cfg(not(any(unix, windows)))]
    let candidate = std::env::var("HOME").ok();
    candidate
}

/// Go `decodeJWTPayload`: base64url (no padding) decodes the claims segment
/// of a JWT and returns the parsed JSON object, or `None` on any failure.
#[must_use]
pub fn decode_jwt_payload(token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let decoded = base64_url_decode_no_pad(parts[1])?;
    serde_json::from_slice(&decoded).ok()
}

/// base64url (RFC 4648 §5) decoding without padding. The workspace does not
/// vendor a base64 crate, so this small decoder mirrors Go's
/// `base64.RawURLEncoding.DecodeString` behavior for JWT claims segments.
#[must_use]
fn base64_url_decode_no_pad(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut values = Vec::with_capacity(input.len());
    for byte in input.bytes() {
        let index = ALPHABET.iter().position(|candidate| *candidate == byte)? as u32;
        values.push(index);
    }
    let len = values.len();
    if len % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((len * 3) / 4);
    let mut index = 0;
    while index + 4 <= len {
        let a = values[index];
        let b = values[index + 1];
        let c = values[index + 2];
        let d = values[index + 3];
        out.push(((a << 2) | (b >> 4)) as u8);
        out.push(((b << 4) | (c >> 2)) as u8);
        out.push(((c << 6) | d) as u8);
        index += 4;
    }
    match len - index {
        2 => {
            let a = values[index];
            let b = values[index + 1];
            out.push(((a << 2) | (b >> 4)) as u8);
        }
        3 => {
            let a = values[index];
            let b = values[index + 1];
            let c = values[index + 2];
            out.push(((a << 2) | (b >> 4)) as u8);
            out.push(((b << 4) | (c >> 2)) as u8);
        }
        _ => {}
    }
    Some(out)
}

/// Go `cloneRoots`: trims, drops empties, dedupes, and clones.
#[must_use]
pub fn clone_roots(values: &[String]) -> Vec<String> {
    let mut items = Vec::with_capacity(values.len());
    let mut seen = HashSet::new();
    for value in values {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() || seen.contains(&trimmed) {
            continue;
        }
        seen.insert(trimmed.clone());
        items.push(trimmed);
    }
    items
}

/// Go `cloneStrings`.
#[must_use]
pub fn clone_strings(values: &[String]) -> Vec<String> {
    values.to_vec()
}

/// Go `cloneInts`.
#[must_use]
pub fn clone_ints(values: &[i64]) -> Vec<i64> {
    values.to_vec()
}

/// Go `cloneStringMap` (nil-safe in Go; empty map here).
#[must_use]
pub fn clone_string_map(values: &HashMap<String, String>) -> HashMap<String, String> {
    values.clone()
}

/// Go `mergeStringMaps`.
#[must_use]
pub fn merge_string_maps(
    base: &HashMap<String, String>,
    extra: &HashMap<String, String>,
) -> HashMap<String, String> {
    if base.is_empty() && extra.is_empty() {
        return HashMap::new();
    }
    let mut merged = base.clone();
    for (key, value) in extra {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// Go `redactedPathSummary`: the file base name, or "redacted" for
/// empty/root/current-directory paths.
#[must_use]
pub fn redacted_path_summary(path: &str) -> String {
    let base = std::path::Path::new(path.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = base.trim();
    if base.is_empty() || base == "." || base == std::path::MAIN_SEPARATOR.to_string() {
        return "redacted".to_string();
    }
    base.to_string()
}

/// Go `firstNonEmptyRoots`.
#[must_use]
pub fn first_non_empty_roots(preferred: &[String], fallback: &[String]) -> Vec<String> {
    if preferred.is_empty() {
        fallback.to_vec()
    } else {
        preferred.to_vec()
    }
}

/// Go `pathsWithinDeclared`.
#[must_use]
pub fn paths_within_declared(paths: &[String], declared: &[String]) -> bool {
    if paths.is_empty() {
        return true;
    }
    if declared.is_empty() {
        return false;
    }
    paths.iter().all(|path| path_within_any(path, declared))
}

/// Go `pathWithinAny`: a cleaned path is within a root when it equals the
/// root or is a strict descendant (equivalent to Go's `filepath.Rel` + ".."
/// checks for the "/"-separated paths this package deals with).
#[must_use]
pub fn path_within_any(path: &str, roots: &[String]) -> bool {
    let clean_path = clean_path_str(path);
    if clean_path.is_empty() {
        return false;
    }
    for root in roots {
        let clean_root = clean_path_str(root);
        if clean_root.is_empty() {
            continue;
        }
        if clean_path == clean_root {
            return true;
        }
        if std::path::Path::new(&clean_path)
            .strip_prefix(std::path::Path::new(&clean_root))
            .is_ok()
        {
            return true;
        }
    }
    false
}

/// A light `filepath.Clean` for "/"-separated paths: collapses duplicate
/// separators, resolves "." and "..", and preserves absolute roots. Used by
/// the declaration-scope check; documented as an approximation of Go's Clean.
#[must_use]
pub fn clean_path_str(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed == "." {
        return ".".to_string();
    }
    let absolute = trimmed.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if absolute {
                    parts.pop();
                } else if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let body = parts.join("/");
    if absolute {
        if body.is_empty() {
            "/".to_string()
        } else {
            format!("/{body}")
        }
    } else if body.is_empty() {
        ".".to_string()
    } else {
        body
    }
}

/// Go `latestUserMessage`: the last message with non-empty content.
#[must_use]
pub fn latest_user_message(messages: &[Message]) -> String {
    for message in messages.iter().rev() {
        if !message.content.trim().is_empty() {
            return message.content.clone();
        }
    }
    String::new()
}
