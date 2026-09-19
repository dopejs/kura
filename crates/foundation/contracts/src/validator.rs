//! Port of daemon/internal/contracts/validator.go.
//!
//! Minimal JSON-schema validator that walks the repository's `schemas/`
//! directory. Supported keywords mirror the Go implementation exactly:
//! `$ref` (local `#/...` pointers and relative file refs with optional
//! pointer), `allOf`, `enum`, `const`, `type`, `format` (`date-time` only),
//! `minimum`, `minLength` (byte length, matching Go's `len`), `minItems`,
//! `required`, `properties`, `additionalProperties: false`, and `items`.
//!
//! Known, intentional divergences from the Go original:
//! - Numeric equality in `enum`/`const` is structural on `serde_json::Value`;
//!   Go compared decoded `json.Number` against schema `float64`, which made a
//!   document `1` fail to match a schema enum `1`. The Rust port treats them
//!   as equal.
//! - `"integer"` type matching requires the JSON number to carry no
//!   fractional part in its lexical form (Go rejected `1.0` documents because
//!   `json.Number.Int64()` fails on them); schema-side numeric declarations
//!   (`minimum`, `minLength`, `minItems`) still accept integral floats, as Go
//!   did via its `float64` branch of `toInt64`.
//! - Error messages use JSON type names where Go printed `%T` Go type names.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use parking_lot::Mutex;
use serde_json::Value;

/// Errors produced while loading schemas or validating documents.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("read schema {path}: {source}")]
    ReadSchema {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("decode schema {path}: {source}")]
    DecodeSchema {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("decode document {path}: {source}")]
    DecodeDocument {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{0}")]
    Validation(String),
}

fn validation(message: String) -> ContractError {
    ContractError::Validation(message)
}

/// Validates JSON documents against JSON-schema files on disk.
///
/// Schemas are loaded lazily and cached by path for the lifetime of the
/// validator, mirroring the Go implementation.
pub struct Validator {
    root_dir: PathBuf,
    cache: Mutex<HashMap<PathBuf, Value>>,
}

impl Validator {
    /// Creates a validator rooted at the repository directory that contains
    /// the `schemas/` tree.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Validates `document` against the schema at `schema_path` relative to
    /// the validator root directory.
    pub fn validate_relative(
        &self,
        schema_path: impl AsRef<Path>,
        document: &[u8],
    ) -> Result<(), ContractError> {
        // Go: filepath.Join(rootDir, filepath.Clean(schemaPath)). Go's Join
        // keeps the root even when the second argument is absolute
        // (Join("a", "/b") == "a/b"), unlike PathBuf::join.
        let cleaned = clean_path(schema_path.as_ref());
        let mut absolute = self.root_dir.clone();
        for component in cleaned.components() {
            if matches!(component, Component::RootDir) {
                continue;
            }
            absolute.push(component.as_os_str());
        }
        self.validate_absolute(&absolute, document)
    }

    /// Validates `document` against the schema at an explicit filesystem
    /// path.
    pub fn validate_absolute(
        &self,
        schema_path: &Path,
        document: &[u8],
    ) -> Result<(), ContractError> {
        let schema = self.load_schema(schema_path)?;

        let value: Value =
            serde_json::from_slice(document).map_err(|source| ContractError::DecodeDocument {
                path: schema_path.to_path_buf(),
                source,
            })?;

        self.validate_schema(schema_path, schema_path, &schema, &value, "$")
    }

    fn load_schema(&self, schema_path: &Path) -> Result<Value, ContractError> {
        if let Some(cached) = self.cache.lock().get(schema_path) {
            return Ok(cached.clone());
        }

        let raw = std::fs::read(schema_path).map_err(|source| ContractError::ReadSchema {
            path: schema_path.to_path_buf(),
            source,
        })?;

        let schema: Value =
            serde_json::from_slice(&raw).map_err(|source| ContractError::DecodeSchema {
                path: schema_path.to_path_buf(),
                source,
            })?;

        self.cache
            .lock()
            .insert(schema_path.to_path_buf(), schema.clone());
        Ok(schema)
    }

    fn validate_schema(
        &self,
        root_schema_path: &Path,
        current_schema_path: &Path,
        schema: &Value,
        value: &Value,
        field_path: &str,
    ) -> Result<(), ContractError> {
        let schema_map = schema.as_object().ok_or_else(|| {
            validation(format!(
                "{field_path}: unsupported schema node in {}",
                current_schema_path.display()
            ))
        })?;

        if let Some(reference) = schema_map.get("$ref").and_then(Value::as_str) {
            let (resolved_root, resolved_current, resolved_schema) = self
                .resolve_ref(root_schema_path, current_schema_path, reference)
                .map_err(|err| {
                    validation(format!("{field_path}: resolve ref {reference:?}: {err}"))
                })?;
            self.validate_schema(
                &resolved_root,
                &resolved_current,
                &resolved_schema,
                value,
                field_path,
            )?;
        }

        if let Some(all_of) = schema_map.get("allOf").and_then(Value::as_array) {
            for item in all_of {
                self.validate_schema(
                    root_schema_path,
                    current_schema_path,
                    item,
                    value,
                    field_path,
                )?;
            }
        }

        if let Some(enum_items) = schema_map.get("enum").and_then(Value::as_array) {
            if !enum_items.iter().any(|item| json_value_equal(item, value)) {
                return Err(validation(format!(
                    "{field_path}: value {} is not in enum {}",
                    go_value(value),
                    go_list(enum_items)
                )));
            }
        }
        if let Some(constant) = schema_map.get("const") {
            if !json_value_equal(constant, value) {
                return Err(validation(format!(
                    "{field_path}: value {} does not match const {}",
                    go_value(value),
                    go_value(constant)
                )));
            }
        }

        if let Some(type_decl) = schema_map.get("type") {
            let matched = matches_declared_type(type_decl, value)
                .map_err(|err| validation(format!("{field_path}: {err}")))?;
            if !matched {
                return Err(validation(format!(
                    "{field_path}: value of type {} does not match schema type {}",
                    json_type_name(value),
                    go_value(type_decl)
                )));
            }
        }

        if let Some(format) = schema_map.get("format").and_then(Value::as_str) {
            if !value.is_null() {
                validate_format(field_path, format, value)?;
            }
        }

        if let Some(minimum) = schema_map.get("minimum") {
            if !value.is_null() {
                validate_minimum(field_path, minimum, value)?;
            }
        }

        if let Some(min_length) = schema_map.get("minLength") {
            if !value.is_null() {
                validate_min_length(field_path, min_length, value)?;
            }
        }

        if let Some(min_items) = schema_map.get("minItems") {
            if !value.is_null() {
                validate_min_items(field_path, min_items, value)?;
            }
        }

        if let Some(object_value) = value.as_object() {
            if let Some(required) = schema_map.get("required").and_then(Value::as_array) {
                for item in required {
                    let key = item.as_str().ok_or_else(|| {
                        validation(format!(
                            "{field_path}: invalid required key declaration {}",
                            go_value(item)
                        ))
                    })?;
                    if !object_value.contains_key(key) {
                        return Err(validation(format!(
                            "{field_path}.{key}: required property is missing"
                        )));
                    }
                }
            }

            let empty_properties;
            let properties = match schema_map.get("properties").and_then(Value::as_object) {
                Some(declared) => declared,
                None => {
                    empty_properties = serde_json::Map::new();
                    &empty_properties
                }
            };

            if schema_map
                .get("additionalProperties")
                .and_then(Value::as_bool)
                == Some(false)
            {
                for key in object_value.keys() {
                    if !properties.contains_key(key) {
                        return Err(validation(format!(
                            "{field_path}.{key}: additional property is not allowed"
                        )));
                    }
                }
            }

            for (key, property_schema) in properties {
                let Some(property_value) = object_value.get(key) else {
                    continue;
                };
                self.validate_schema(
                    root_schema_path,
                    current_schema_path,
                    property_schema,
                    property_value,
                    &format!("{field_path}.{key}"),
                )?;
            }
        }

        if let Some(array_value) = value.as_array() {
            if let Some(item_schema) = schema_map.get("items") {
                for (index, item) in array_value.iter().enumerate() {
                    self.validate_schema(
                        root_schema_path,
                        current_schema_path,
                        item_schema,
                        item,
                        &format!("{field_path}[{index}]"),
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Resolves a `$ref` to (`root_schema_path`, `current_schema_path`,
    /// schema). Local `#/...` pointers keep the current file context but
    /// resolve against the root schema; external file refs switch both to
    /// the target file.
    fn resolve_ref(
        &self,
        root_schema_path: &Path,
        current_schema_path: &Path,
        reference: &str,
    ) -> Result<(PathBuf, PathBuf, Value), ContractError> {
        if reference.starts_with("#/") {
            let root_schema = self.load_schema(root_schema_path)?;
            let resolved = resolve_json_pointer(
                &root_schema,
                reference.strip_prefix('#').unwrap_or(reference),
            )?;
            return Ok((
                root_schema_path.to_path_buf(),
                current_schema_path.to_path_buf(),
                resolved,
            ));
        }

        if reference.starts_with('#') {
            return Err(validation(format!("unsupported local ref {reference:?}")));
        }

        let (reference_path, pointer) = match reference.find('#') {
            Some(index) => (&reference[..index], &reference[index..]),
            None => (reference, ""),
        };

        let base_dir = current_schema_path.parent().unwrap_or(Path::new(""));
        let target_path = clean_path(&base_dir.join(reference_path));
        let target_schema = self.load_schema(&target_path)?;

        if !pointer.is_empty() {
            let resolved =
                resolve_json_pointer(&target_schema, pointer.strip_prefix('#').unwrap_or(pointer))?;
            return Ok((target_path.clone(), target_path, resolved));
        }

        Ok((target_path.clone(), target_path, target_schema))
    }
}

/// Lexical path normalization equivalent to Go's `filepath.Clean`: drops
/// `.` segments, resolves `..` against preceding normal segments, and keeps
/// leading `..` on relative paths.
fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().last(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(out.components().last(), Some(Component::RootDir)) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_json_pointer(node: &Value, pointer: &str) -> Result<Value, ContractError> {
    if pointer.is_empty() {
        return Ok(node.clone());
    }

    let mut current = node;
    let without_lead = pointer.strip_prefix('/').unwrap_or(pointer);
    for raw in without_lead.split('/') {
        let part = raw.replace("~1", "/").replace("~0", "~");
        match current {
            Value::Object(map) => {
                current = map
                    .get(&part)
                    .ok_or_else(|| validation(format!("pointer segment {part:?} not found")))?;
            }
            Value::Array(items) => {
                let index: i64 = part.parse().map_err(|_| {
                    validation(format!("pointer segment {part:?} is not an array index"))
                })?;
                if index < 0 || index as usize >= items.len() {
                    return Err(validation(format!("pointer index {index} out of range")));
                }
                current = &items[index as usize];
            }
            other => {
                return Err(validation(format!(
                    "pointer segment {part:?} cannot be resolved through {}",
                    json_type_name(other)
                )));
            }
        }
    }

    Ok(current.clone())
}

fn json_value_equal(left: &Value, right: &Value) -> bool {
    left == right
}

fn matches_declared_type(type_decl: &Value, value: &Value) -> Result<bool, ContractError> {
    match type_decl {
        Value::String(name) => Ok(matches_single_type(name, value)),
        Value::Array(items) => {
            for item in items {
                let name = item.as_str().ok_or_else(|| {
                    validation(format!("invalid type declaration {}", go_value(item)))
                })?;
                if matches_single_type(name, value) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        other => Err(validation(format!(
            "unsupported type declaration {}",
            go_value(other)
        ))),
    }
}

fn matches_single_type(type_name: &str, value: &Value) -> bool {
    match type_name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => match value {
            Value::Number(number) => number.is_i64() || number.is_u64(),
            _ => false,
        },
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn validate_format(field_path: &str, format: &str, value: &Value) -> Result<(), ContractError> {
    if format == "date-time" {
        let text = value.as_str().ok_or_else(|| {
            validation(format!(
                "{field_path}: date-time format requires string value"
            ))
        })?;
        if chrono::DateTime::parse_from_rfc3339(text).is_err() {
            return Err(validation(format!(
                "{field_path}: invalid date-time {text:?}"
            )));
        }
    }
    Ok(())
}

fn validate_minimum(field_path: &str, minimum: &Value, value: &Value) -> Result<(), ContractError> {
    let min_float = to_f64(minimum).map_err(|_| {
        validation(format!(
            "{field_path}: invalid schema minimum {}",
            go_value(minimum)
        ))
    })?;
    let value_float = to_f64(value)
        .map_err(|_| validation(format!("{field_path}: minimum requires numeric value")))?;
    if value_float < min_float {
        return Err(validation(format!(
            "{field_path}: value {} is smaller than minimum {min_float}",
            go_value(value)
        )));
    }
    Ok(())
}

fn validate_min_length(
    field_path: &str,
    min_length: &Value,
    value: &Value,
) -> Result<(), ContractError> {
    let required_length = to_i64(min_length).map_err(|_| {
        validation(format!(
            "{field_path}: invalid schema minLength {}",
            go_value(min_length)
        ))
    })?;
    let text = value
        .as_str()
        .ok_or_else(|| validation(format!("{field_path}: minLength requires string value")))?;
    // Go compares int64(len(text)) against the declared length: byte
    // length, not rune count, and signed (a negative declaration never
    // fails).
    if (text.len() as i64) < required_length {
        return Err(validation(format!(
            "{field_path}: string length {} is smaller than minLength {required_length}",
            text.len()
        )));
    }
    Ok(())
}

fn validate_min_items(
    field_path: &str,
    min_items: &Value,
    value: &Value,
) -> Result<(), ContractError> {
    let required_length = to_i64(min_items).map_err(|_| {
        validation(format!(
            "{field_path}: invalid schema minItems {}",
            go_value(min_items)
        ))
    })?;
    let items = value
        .as_array()
        .ok_or_else(|| validation(format!("{field_path}: minItems requires array value")))?;
    if (items.len() as i64) < required_length {
        return Err(validation(format!(
            "{field_path}: array length {} is smaller than minItems {required_length}",
            items.len()
        )));
    }
    Ok(())
}

fn to_f64(value: &Value) -> Result<f64, ContractError> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| validation(format!("not a number: {}", json_type_name(value)))),
        other => Err(validation(format!(
            "not a number: {}",
            json_type_name(other)
        ))),
    }
}

fn to_i64(value: &Value) -> Result<i64, ContractError> {
    match value {
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Ok(integer);
            }
            if let Some(unsigned) = number.as_u64() {
                return i64::try_from(unsigned)
                    .map_err(|_| validation(format!("not an integer: {unsigned}")));
            }
            if let Some(float) = number.as_f64() {
                // Mirrors Go's float64 branch: integral floats are accepted.
                if float != float as i64 as f64 {
                    return Err(validation(format!("not an integer: {float}")));
                }
                return Ok(float as i64);
            }
            Err(validation(format!("not an integer: {number}")))
        }
        other => Err(validation(format!(
            "not an integer: {}",
            json_type_name(other)
        ))),
    }
}

/// JSON type name used in error messages where Go printed `%T`.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Renders a value the way Go's `%v` verb does for decoded JSON: strings
/// unquoted, everything else compact.
fn go_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Renders a list the way Go's `%v` verb prints `[]any`: `[a b c]`.
fn go_list(items: &[Value]) -> String {
    let inner = items.iter().map(go_value).collect::<Vec<_>>().join(" ");
    format!("[{inner}]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kura-contracts-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp schema root");
        dir
    }

    fn write_schema(root: &Path, relative: &str, schema: &Value) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create schema parent dir");
        }
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(schema).expect("encode schema"),
        )
        .expect("write schema");
    }

    #[test]
    fn json_pointer_resolves_nested_escaped_and_indexed_segments() {
        let doc = json!({
            "a/b": { "c~d": ["x", "y"] },
            "list": [{"name": "first"}, {"name": "second"}],
        });

        assert_eq!(resolve_json_pointer(&doc, "").unwrap(), doc);
        assert_eq!(
            resolve_json_pointer(&doc, "/a~1b/c~0d/1").unwrap(),
            json!("y")
        );
        assert_eq!(
            resolve_json_pointer(&doc, "/list/0/name").unwrap(),
            json!("first")
        );
    }

    #[test]
    fn json_pointer_reports_resolution_failures() {
        let doc = json!({"obj": {"scalar": 1}, "list": [1]});

        let missing = resolve_json_pointer(&doc, "/obj/nope").unwrap_err();
        assert!(missing.to_string().contains("not found"), "{missing}");

        let not_index = resolve_json_pointer(&doc, "/list/abc").unwrap_err();
        assert!(
            not_index.to_string().contains("is not an array index"),
            "{not_index}"
        );

        let out_of_range = resolve_json_pointer(&doc, "/list/4").unwrap_err();
        assert!(
            out_of_range.to_string().contains("out of range"),
            "{out_of_range}"
        );

        let negative = resolve_json_pointer(&doc, "/list/-1").unwrap_err();
        assert!(negative.to_string().contains("out of range"), "{negative}");

        let through_scalar = resolve_json_pointer(&doc, "/obj/scalar/deeper").unwrap_err();
        assert!(
            through_scalar.to_string().contains("cannot be resolved"),
            "{through_scalar}"
        );
    }

    #[test]
    fn enforces_type_enum_const_and_numeric_keywords() {
        let root = temp_root("keywords");
        write_schema(
            &root,
            "schemas/widget.schema.json",
            &json!({
                "type": "object",
                "required": ["name", "count"],
                "additionalProperties": false,
                "properties": {
                    "name": {"type": "string", "minLength": 2},
                    "count": {"type": "integer", "minimum": 1},
                    "kind": {"enum": ["alpha", "beta"]},
                    "mode": {"const": "fixed"},
                    "tags": {"type": "array", "minItems": 1, "items": {"type": "string"}},
                    "createdAt": {"type": "string", "format": "date-time"},
                    "note": {"type": ["string", "null"]}
                }
            }),
        );

        let validator = Validator::new(&root);

        let valid = br#"{
            "name": "widget",
            "count": 2,
            "kind": "alpha",
            "mode": "fixed",
            "tags": ["a"],
            "createdAt": "2026-04-18T12:00:00Z",
            "note": null
        }"#;
        validator
            .validate_relative("schemas/widget.schema.json", valid)
            .expect("valid document should pass");

        let cases: &[(&[u8], &str)] = &[
            (br#"{"count": 1}"#, "required property is missing"),
            (
                br#"{"name": "w", "count": 1, "extra": true}"#,
                "additional property is not allowed",
            ),
            (br#"{"name": 3, "count": 1}"#, "does not match schema type"),
            (
                br#"{"name": "w", "count": 1.5}"#,
                "does not match schema type",
            ),
            (br#"{"name": "w", "count": 0}"#, "smaller than minimum"),
            (br#"{"name": "w", "count": 1}"#, "smaller than minLength"),
            (
                br#"{"name": "ww", "count": 1, "kind": "gamma"}"#,
                "is not in enum",
            ),
            (
                br#"{"name": "ww", "count": 1, "mode": "other"}"#,
                "does not match const",
            ),
            (
                br#"{"name": "ww", "count": 1, "tags": []}"#,
                "smaller than minItems",
            ),
            (br#"{"name": "ww", "count": 1, "tags": [3]}"#, "$.tags[0]"),
            (
                br#"{"name": "ww", "count": 1, "createdAt": "not-a-date"}"#,
                "invalid date-time",
            ),
            (
                br#"{"name": "ww", "count": 1, "note": 4}"#,
                "does not match schema type",
            ),
        ];
        for (document, expected) in cases {
            let err = validator
                .validate_relative("schemas/widget.schema.json", document)
                .expect_err("document should fail validation");
            assert!(
                err.to_string().contains(expected),
                "expected {expected:?} in: {err}"
            );
        }
    }

    #[test]
    fn min_length_counts_bytes_like_go() {
        let root = temp_root("minlength-bytes");
        write_schema(
            &root,
            "schemas/text.schema.json",
            &json!({"type": "object", "properties": {"v": {"type": "string", "minLength": 3}}}),
        );
        let validator = Validator::new(&root);
        // "é" is 2 bytes but 1 rune; Go's len() counts bytes, so "éx" (3
        // bytes, 2 runes) satisfies minLength 3.
        validator
            .validate_relative("schemas/text.schema.json", "{\"v\":\"éx\"}".as_bytes())
            .expect("byte-length semantics should accept 3-byte string");
    }

    #[test]
    fn resolves_local_and_external_refs() {
        let root = temp_root("refs");
        write_schema(
            &root,
            "schemas/defs.json",
            &json!({
                "definitions": {
                    "identifier": {"type": "string", "minLength": 3}
                }
            }),
        );
        write_schema(
            &root,
            "schemas/entry.schema.json",
            &json!({
                "type": "object",
                "required": ["id", "refId"],
                "properties": {
                    "id": {"$ref": "#/definitions/localId"},
                    "refId": {"$ref": "defs.json#/definitions/identifier"}
                },
                "definitions": {
                    "localId": {"type": "string", "minLength": 2}
                }
            }),
        );

        let validator = Validator::new(&root);
        validator
            .validate_relative(
                "schemas/entry.schema.json",
                br#"{"id": "ab", "refId": "abc"}"#,
            )
            .expect("document satisfying both refs should pass");

        let local_err = validator
            .validate_relative(
                "schemas/entry.schema.json",
                br#"{"id": "a", "refId": "abc"}"#,
            )
            .expect_err("local ref constraint should apply");
        assert!(local_err.to_string().contains("$.id"), "{local_err}");

        let external_err = validator
            .validate_relative(
                "schemas/entry.schema.json",
                br#"{"id": "ab", "refId": "x"}"#,
            )
            .expect_err("external ref constraint should apply");
        assert!(
            external_err.to_string().contains("$.refId"),
            "{external_err}"
        );
    }

    #[test]
    fn ref_resolution_errors_are_wrapped_with_field_path() {
        let root = temp_root("ref-errors");
        write_schema(
            &root,
            "schemas/bad-local.schema.json",
            &json!({"$ref": "#nope"}),
        );
        write_schema(
            &root,
            "schemas/missing-pointer.schema.json",
            &json!({"$ref": "#/definitions/absent"}),
        );
        write_schema(
            &root,
            "schemas/missing-file.schema.json",
            &json!({"$ref": "does-not-exist.json"}),
        );

        let validator = Validator::new(&root);

        let err = validator
            .validate_relative("schemas/bad-local.schema.json", b"{}")
            .expect_err("unsupported local ref should fail");
        assert!(
            err.to_string()
                .contains("$: resolve ref \"#nope\": unsupported local ref"),
            "{err}"
        );

        let err = validator
            .validate_relative("schemas/missing-pointer.schema.json", b"{}")
            .expect_err("missing pointer target should fail");
        assert!(
            err.to_string()
                .contains("resolve ref \"#/definitions/absent\""),
            "{err}"
        );

        let err = validator
            .validate_relative("schemas/missing-file.schema.json", b"{}")
            .expect_err("missing ref target should fail");
        assert!(err.to_string().contains("read schema"), "{err}");
    }

    #[test]
    fn allof_applies_every_subschema() {
        let root = temp_root("allof");
        write_schema(
            &root,
            "schemas/combo.schema.json",
            &json!({
                "allOf": [
                    {"type": "object", "required": ["a"]},
                    {"type": "object", "required": ["b"]}
                ]
            }),
        );
        let validator = Validator::new(&root);
        validator
            .validate_relative("schemas/combo.schema.json", br#"{"a": 1, "b": 2}"#)
            .expect("document satisfying all branches should pass");
        let err = validator
            .validate_relative("schemas/combo.schema.json", br#"{"a": 1}"#)
            .expect_err("missing branch requirement should fail");
        assert!(err.to_string().contains("$.b"), "{err}");
    }

    #[test]
    fn non_object_schema_nodes_are_rejected() {
        let root = temp_root("non-object-schema");
        write_schema(
            &root,
            "schemas/list.schema.json",
            &json!(["not", "an", "object"]),
        );
        let validator = Validator::new(&root);
        let err = validator
            .validate_relative("schemas/list.schema.json", b"{}")
            .expect_err("array schema node should fail");
        assert!(err.to_string().contains("unsupported schema node"), "{err}");
    }

    #[test]
    fn invalid_documents_and_schemas_report_decode_errors() {
        let root = temp_root("decode-errors");
        write_schema(&root, "schemas/ok.schema.json", &json!({"type": "object"}));
        std::fs::write(root.join("schemas/broken.schema.json"), b"{not json")
            .expect("write broken schema");

        let validator = Validator::new(&root);

        let err = validator
            .validate_relative("schemas/ok.schema.json", b"{not json")
            .expect_err("invalid document should fail");
        assert!(
            matches!(err, ContractError::DecodeDocument { .. }),
            "{err:?}"
        );

        let err = validator
            .validate_relative("schemas/broken.schema.json", b"{}")
            .expect_err("invalid schema should fail");
        assert!(matches!(err, ContractError::DecodeSchema { .. }), "{err:?}");

        let err = validator
            .validate_relative("schemas/absent.schema.json", b"{}")
            .expect_err("missing schema should fail");
        assert!(matches!(err, ContractError::ReadSchema { .. }), "{err:?}");
    }

    #[test]
    fn schemas_are_cached_after_first_load() {
        let root = temp_root("cache");
        write_schema(
            &root,
            "schemas/cached.schema.json",
            &json!({"type": "object", "required": ["a"]}),
        );
        let validator = Validator::new(&root);
        validator
            .validate_relative("schemas/cached.schema.json", br#"{"a": 1}"#)
            .expect("first validation should pass");

        // Removing the file after a successful validation must not break
        // subsequent validations: the cached schema is reused.
        std::fs::remove_file(root.join("schemas/cached.schema.json")).expect("remove schema");
        validator
            .validate_relative("schemas/cached.schema.json", br#"{"a": 1}"#)
            .expect("cached schema should be reused");
    }

    #[test]
    fn clean_path_normalizes_dot_segments() {
        assert_eq!(clean_path(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(clean_path(Path::new("./a")), PathBuf::from("a"));
        assert_eq!(clean_path(Path::new("../a")), PathBuf::from("../a"));
    }
}
