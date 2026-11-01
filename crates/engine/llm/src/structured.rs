//! Structured output: replies that must be JSON conforming to a caller's
//! schema (agent-runtime-program.md 2.8, G7).
//!
//! The caller's schema is checked when the request arrives, against a
//! deliberately small subset of JSON Schema: the keywords this module can
//! enforce. A schema using anything else is refused up front rather than
//! half-enforced, because a keyword that is silently skipped is a reply that
//! is silently wrong. Replies are then validated here, whatever the provider
//! claims to have enforced: an OpenAI-compatible gateway may ignore a
//! constraint it does not know.
//!
//! Supported: `type` (one or several of `object`, `array`, `string`,
//! `number`, `integer`, `boolean`, `null`), `properties`, `required`,
//! `additionalProperties`, `items`, `enum`, `const`, `anyOf`, `oneOf`,
//! `allOf`, `$ref` to a pointer inside the schema (`#/$defs/...`),
//! `$defs`/`definitions`, `minimum`, `maximum`, `exclusiveMinimum`,
//! `exclusiveMaximum`, `minLength`, `maxLength`, `minItems`, `maxItems`,
//! `pattern`, and boolean schemas. `title`, `description`, `$schema`, `$id`,
//! `$comment`, `examples`, `default` and `format` are annotations: allowed,
//! not enforced (`format` is an annotation in draft 2020-12 as well).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest schema accepted, serialized. A schema rides on every round of the
/// turn it constrains.
pub const MAX_SCHEMA_BYTES: usize = 32 * 1024;

/// Deepest nesting followed, in the schema and in the reply alike. Bounds
/// the work a recursive `$ref` can ask for.
const MAX_DEPTH: usize = 32;

/// Problems reported for one reply; the rest are summarised.
const MAX_ERRORS: usize = 8;

/// Schema nodes visited validating one reply. Depth alone does not bound
/// the work: `anyOf` tries every branch, so branches that refer back to
/// the schema multiply at every level.
const MAX_STEPS: usize = 100_000;

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

/// A reply that must be a JSON value conforming to `schema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseFormat {
    /// What the reply is, for the model and for providers that take a
    /// schema name: 1-64 of `[A-Za-z0-9_-]`.
    pub name: String,
    pub schema: Value,
}

impl ResponseFormat {
    /// Whether this format can be enforced: the name is well formed, the
    /// schema is small enough and uses only supported keywords, and every
    /// `$ref` resolves.
    pub fn check(&self) -> Result<(), String> {
        let name_ok = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !name_ok {
            return Err("name must be 1-64 characters of A-Z, a-z, 0-9, _ or -".to_string());
        }
        let size = serde_json::to_string(&self.schema)
            .map(|text| text.len())
            .unwrap_or(usize::MAX);
        if size > MAX_SCHEMA_BYTES {
            return Err(format!(
                "schema is {size} bytes; at most {MAX_SCHEMA_BYTES} are accepted"
            ));
        }
        check_schema(&self.schema, &self.schema, "#", 0)
    }

    /// What the model is told, on every round of the turn.
    #[must_use]
    pub fn instruction(&self) -> String {
        format!(
            "Your final reply must be only a JSON value -- no prose before or after it, no code \
             fences -- that conforms to this JSON Schema, named `{}`:\n{}",
            self.name, self.schema
        )
    }

    /// The reply as JSON, if it is JSON that conforms; otherwise what is
    /// wrong with it, one problem per entry.
    pub fn accept(&self, reply: &str) -> Result<Value, Vec<String>> {
        let value = parse_reply(reply).map_err(|error| vec![error])?;
        let errors = validate(&self.schema, &value);
        if errors.is_empty() {
            Ok(value)
        } else {
            Err(errors)
        }
    }
}

/// What the model is told when its reply did not conform.
#[must_use]
pub fn repair_prompt(errors: &[String]) -> String {
    let mut prompt = "Your reply does not conform to the required JSON Schema:\n".to_string();
    for error in errors {
        prompt.push_str("- ");
        prompt.push_str(error);
        prompt.push('\n');
    }
    prompt.push_str("Reply again with only the corrected JSON value.");
    prompt
}

/// The reply's JSON. A reply that is one fenced code block is read as its
/// contents: models fence JSON out of habit, and the fence carries nothing.
fn parse_reply(reply: &str) -> Result<Value, String> {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return Err("the reply is empty".to_string());
    }
    let body = match trimmed.strip_prefix("```") {
        Some(rest) => {
            let rest = rest.strip_suffix("```").unwrap_or(rest);
            // The opening fence's info string (`json`) ends at the newline.
            rest.split_once('\n').map_or(rest, |(_, body)| body)
        }
        None => trimmed,
    };
    serde_json::from_str(body.trim()).map_err(|error| format!("the reply is not JSON: {error}"))
}

fn check_schema(node: &Value, root: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema nests deeper than {MAX_DEPTH}"));
    }
    let map = match node {
        Value::Bool(_) => return Ok(()),
        Value::Object(map) => map,
        _ => return Err(format!("{path}: a schema must be an object or a boolean")),
    };
    let schemas = |value: &Value, key: &str| -> Result<(), String> {
        let items = value
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or_else(|| format!("{path}/{key}: must be a non-empty array of schemas"))?;
        for (index, item) in items.iter().enumerate() {
            check_schema(item, root, &format!("{path}/{key}/{index}"), depth + 1)?;
        }
        Ok(())
    };
    let count = |value: &Value, key: &str| -> Result<(), String> {
        value
            .as_u64()
            .map(|_| ())
            .ok_or_else(|| format!("{path}/{key}: must be a non-negative integer"))
    };
    for (key, value) in map {
        match key.as_str() {
            "title" | "description" | "$schema" | "$id" | "$comment" | "examples" | "default"
            | "format" => {}
            "type" => {
                let names: Vec<&Value> = match value {
                    Value::Array(names) if !names.is_empty() => names.iter().collect(),
                    Value::String(_) => vec![value],
                    _ => {
                        return Err(format!(
                            "{path}/type: must be a type name or a list of them"
                        ));
                    }
                };
                for name in names {
                    if !name.as_str().is_some_and(|name| TYPES.contains(&name)) {
                        return Err(format!("{path}/type: unknown type {name}"));
                    }
                }
            }
            "properties" | "$defs" | "definitions" => {
                let members = value
                    .as_object()
                    .ok_or_else(|| format!("{path}/{key}: must be an object of schemas"))?;
                for (name, member) in members {
                    check_schema(member, root, &format!("{path}/{key}/{name}"), depth + 1)?;
                }
            }
            "required" => {
                if !value
                    .as_array()
                    .is_some_and(|names| names.iter().all(Value::is_string))
                {
                    return Err(format!("{path}/required: must be an array of strings"));
                }
            }
            "additionalProperties" | "items" => {
                check_schema(value, root, &format!("{path}/{key}"), depth + 1)?;
            }
            "enum" => {
                if value.as_array().is_none_or(|items| items.is_empty()) {
                    return Err(format!("{path}/enum: must be a non-empty array"));
                }
            }
            "const" => {}
            "anyOf" | "oneOf" | "allOf" => schemas(value, key)?,
            "$ref" => {
                let reference = value
                    .as_str()
                    .ok_or_else(|| format!("{path}/$ref: must be a string"))?;
                if resolve(root, reference).is_none() {
                    return Err(format!(
                        "{path}/$ref: `{reference}` does not point inside this schema"
                    ));
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if !value.is_number() {
                    return Err(format!("{path}/{key}: must be a number"));
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" => count(value, key)?,
            "pattern" => {
                let pattern = value
                    .as_str()
                    .ok_or_else(|| format!("{path}/pattern: must be a string"))?;
                regex::Regex::new(pattern).map_err(|error| format!("{path}/pattern: {error}"))?;
            }
            other => {
                return Err(format!(
                    "{path}: keyword `{other}` is not supported for structured output"
                ));
            }
        }
    }
    Ok(())
}

/// A `$ref` inside the schema: `#` or a JSON pointer after it.
fn resolve<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        Some(root)
    } else {
        root.pointer(pointer)
    }
}

/// Everything wrong with `value` against `schema`, at most a handful.
#[must_use]
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let steps = std::cell::Cell::new(0);
    let mut validator = Validator {
        root: schema,
        steps: &steps,
        errors: Vec::new(),
    };
    validator.check(schema, value, "$", 0);
    if steps.get() > MAX_STEPS {
        return vec![format!(
            "$: the schema is too complex to check this reply against (over {MAX_STEPS} steps)"
        )];
    }
    if validator.errors.len() > MAX_ERRORS {
        let more = validator.errors.len() - MAX_ERRORS;
        validator.errors.truncate(MAX_ERRORS);
        validator.errors.push(format!("and {more} more"));
    }
    validator.errors
}

struct Validator<'a> {
    root: &'a Value,
    /// Shared with every probe, so the budget covers branches tried.
    steps: &'a std::cell::Cell<usize>,
    errors: Vec<String>,
}

impl Validator<'_> {
    fn fail(&mut self, path: &str, message: impl std::fmt::Display) {
        self.errors.push(format!("{path}: {message}"));
    }

    /// Whether `value` conforms, without reporting why not.
    fn conforms(&self, schema: &Value, value: &Value, depth: usize) -> bool {
        let mut probe = Validator {
            root: self.root,
            steps: self.steps,
            errors: Vec::new(),
        };
        probe.check(schema, value, "$", depth);
        probe.errors.is_empty()
    }

    fn check(&mut self, schema: &Value, value: &Value, path: &str, depth: usize) {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MAX_STEPS {
            return;
        }
        if depth > MAX_DEPTH {
            self.fail(path, format!("nests deeper than {MAX_DEPTH}"));
            return;
        }
        let map = match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                self.fail(path, "no value is allowed here");
                return;
            }
            Value::Object(map) => map,
            // Refused by `check_schema`; nothing to enforce.
            _ => return,
        };

        if let Some(target) = map
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| resolve(self.root, reference))
        {
            self.check(target, value, path, depth + 1);
        }
        if let Some(types) = map.get("type") {
            let allowed: Vec<&str> = match types {
                Value::String(name) => vec![name.as_str()],
                Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !allowed.iter().any(|name| has_type(value, name)) {
                self.fail(
                    path,
                    format!(
                        "expected {}, found {}",
                        allowed.join(" or "),
                        type_of(value)
                    ),
                );
                return;
            }
        }
        if let Some(options) = map.get("enum").and_then(Value::as_array) {
            if !options.iter().any(|option| json_eq(option, value)) {
                self.fail(
                    path,
                    format!("must be one of {}", Value::Array(options.clone())),
                );
            }
        }
        if let Some(expected) = map.get("const") {
            if !json_eq(expected, value) {
                self.fail(path, format!("must be {expected}"));
            }
        }
        if let Some(branches) = map.get("allOf").and_then(Value::as_array) {
            for branch in branches {
                self.check(branch, value, path, depth + 1);
            }
        }
        if let Some(branches) = map.get("anyOf").and_then(Value::as_array) {
            if !branches
                .iter()
                .any(|branch| self.conforms(branch, value, depth + 1))
            {
                self.fail(path, "matches none of the allowed shapes (anyOf)");
            }
        }
        if let Some(branches) = map.get("oneOf").and_then(Value::as_array) {
            let matched = branches
                .iter()
                .filter(|branch| self.conforms(branch, value, depth + 1))
                .count();
            if matched != 1 {
                self.fail(
                    path,
                    format!("must match exactly one allowed shape (oneOf), matches {matched}"),
                );
            }
        }

        match value {
            Value::Object(object) => {
                let properties = map.get("properties").and_then(Value::as_object);
                if let Some(required) = map.get("required").and_then(Value::as_array) {
                    for name in required.iter().filter_map(Value::as_str) {
                        if !object.contains_key(name) {
                            self.fail(path, format!("missing required property `{name}`"));
                        }
                    }
                }
                for (name, member) in object {
                    let member_path = format!("{path}.{name}");
                    match properties.and_then(|properties| properties.get(name)) {
                        Some(member_schema) => {
                            self.check(member_schema, member, &member_path, depth + 1);
                        }
                        None => match map.get("additionalProperties") {
                            Some(Value::Bool(false)) => {
                                self.fail(path, format!("property `{name}` is not allowed"));
                            }
                            Some(extra) => self.check(extra, member, &member_path, depth + 1),
                            None => {}
                        },
                    }
                }
            }
            Value::Array(items) => {
                self.bound(path, "items", items.len(), map, "minItems", "maxItems");
                if let Some(item_schema) = map.get("items") {
                    for (index, item) in items.iter().enumerate() {
                        self.check(item_schema, item, &format!("{path}[{index}]"), depth + 1);
                    }
                }
            }
            Value::String(text) => {
                self.bound(
                    path,
                    "characters",
                    text.chars().count(),
                    map,
                    "minLength",
                    "maxLength",
                );
                if let Some(pattern) = map.get("pattern").and_then(Value::as_str) {
                    // Compiled when the schema was checked; a pattern that
                    // fails here was never accepted.
                    if let Ok(regex) = regex::Regex::new(pattern) {
                        if !regex.is_match(text) {
                            self.fail(path, format!("does not match pattern `{pattern}`"));
                        }
                    }
                }
            }
            Value::Number(number) => {
                let Some(number) = number.as_f64() else {
                    return;
                };
                let limit = |key: &str| map.get(key).and_then(Value::as_f64);
                if limit("minimum").is_some_and(|min| number < min) {
                    self.fail(path, format!("must be at least {}", map["minimum"]));
                }
                if limit("maximum").is_some_and(|max| number > max) {
                    self.fail(path, format!("must be at most {}", map["maximum"]));
                }
                if limit("exclusiveMinimum").is_some_and(|min| number <= min) {
                    self.fail(
                        path,
                        format!("must be more than {}", map["exclusiveMinimum"]),
                    );
                }
                if limit("exclusiveMaximum").is_some_and(|max| number >= max) {
                    self.fail(
                        path,
                        format!("must be less than {}", map["exclusiveMaximum"]),
                    );
                }
            }
            Value::Bool(_) | Value::Null => {}
        }
    }

    fn bound(
        &mut self,
        path: &str,
        unit: &str,
        len: usize,
        map: &serde_json::Map<String, Value>,
        min_key: &str,
        max_key: &str,
    ) {
        let len = len as u64;
        if let Some(min) = map.get(min_key).and_then(Value::as_u64) {
            if len < min {
                self.fail(path, format!("needs at least {min} {unit}, has {len}"));
            }
        }
        if let Some(max) = map.get(max_key).and_then(Value::as_u64) {
            if len > max {
                self.fail(path, format!("allows at most {max} {unit}, has {len}"));
            }
        }
    }
}

fn has_type(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => match value {
            Value::Number(number) => {
                number.is_i64()
                    || number.is_u64()
                    || number.as_f64().is_some_and(|float| float.fract() == 0.0)
            }
            _ => false,
        },
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}

/// JSON equality with numbers compared by value, so `1` equals `1.0`.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| json_eq(value, other)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn format(schema: Value) -> ResponseFormat {
        ResponseFormat {
            name: "answer".into(),
            schema,
        }
    }

    fn person() -> ResponseFormat {
        format(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1},
                "age": {"type": "integer", "minimum": 0},
                "role": {"enum": ["admin", "member"]},
                "tags": {"type": "array", "items": {"$ref": "#/$defs/tag"}, "maxItems": 2},
                "email": {"type": ["string", "null"], "pattern": "^[^@]+@[^@]+$"}
            },
            "required": ["name", "age"],
            "additionalProperties": false,
            "$defs": {"tag": {"type": "string", "maxLength": 5}}
        }))
    }

    #[test]
    fn a_conforming_reply_is_accepted_and_parsed() {
        let value = person()
            .accept(r#"{"name":"Ada","age":36,"role":"admin","tags":["x"],"email":"a@b"}"#)
            .unwrap();
        assert_eq!(value["name"], "Ada");
        // 36.0 is an integer by value.
        assert!(person().accept(r#"{"name":"Ada","age":36.0}"#).is_ok());
        assert!(
            person()
                .accept(r#"{"name":"Ada","age":1,"email":null}"#)
                .is_ok()
        );
    }

    #[test]
    fn a_fenced_reply_is_read_as_its_contents() {
        assert!(
            person()
                .accept("```json\n{\"name\":\"Ada\",\"age\":1}\n```")
                .is_ok()
        );
        assert!(
            person()
                .accept("```\n{\"name\":\"Ada\",\"age\":1}```")
                .is_ok()
        );
    }

    #[test]
    fn every_violation_is_reported_with_where_it_is() {
        let errors = person()
            .accept(r#"{"age":-1,"role":"root","tags":["toolong","a","b"],"email":"nope","x":1}"#)
            .unwrap_err();
        let all = errors.join("\n");
        for expected in [
            "$: missing required property `name`",
            "$.age: must be at least 0",
            "$.role: must be one of",
            "$.tags: allows at most 2 items",
            "$.tags[0]: allows at most 5 characters",
            "$.email: does not match pattern",
            "$: property `x` is not allowed",
        ] {
            assert!(all.contains(expected), "{expected} not in:\n{all}");
        }
        assert_eq!(
            person().accept(r#"{"name":"Ada","age":"36"}"#).unwrap_err(),
            vec!["$.age: expected integer, found string"]
        );
        assert!(
            person().accept("Sure! Here it is.").unwrap_err()[0]
                .starts_with("the reply is not JSON")
        );
    }

    #[test]
    fn any_of_and_one_of_count_matching_branches() {
        let either = format(json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}));
        assert!(either.accept("3").is_ok());
        assert!(either.accept("true").is_err());
        let exactly = format(json!({"oneOf": [{"type": "number"}, {"type": "integer"}]}));
        assert!(exactly.accept("1.5").is_ok());
        assert!(exactly.accept("2").is_err(), "matches both");
    }

    #[test]
    fn unsupported_or_broken_schemas_are_refused_up_front() {
        for (schema, needle) in [
            (
                json!({"type": "object", "if": {}}),
                "keyword `if` is not supported",
            ),
            (json!({"type": "objekt"}), "unknown type"),
            (json!({"$ref": "#/$defs/missing"}), "does not point inside"),
            (
                json!({"$ref": "https://example.com/s.json"}),
                "does not point inside",
            ),
            (json!({"pattern": "("}), "/pattern"),
            (json!({"minLength": -1}), "non-negative integer"),
            (json!({"anyOf": []}), "non-empty array"),
            (json!(3), "must be an object or a boolean"),
        ] {
            let error = format(schema.clone()).check().unwrap_err();
            assert!(error.contains(needle), "{schema}: {error}");
        }
        assert!(person().check().is_ok());
        assert!(
            ResponseFormat {
                name: "has space".into(),
                schema: json!({}),
            }
            .check()
            .is_err()
        );
        let huge = format(json!({"description": "x".repeat(MAX_SCHEMA_BYTES)}));
        assert!(huge.check().unwrap_err().contains("bytes"));
    }

    #[test]
    fn a_recursive_ref_is_bounded() {
        let list = format(json!({
            "$defs": {"node": {"type": "object", "properties": {"next": {"$ref": "#/$defs/node"}}}},
            "$ref": "#/$defs/node"
        }));
        assert!(list.check().is_ok());
        let mut deep = json!({});
        for _ in 0..40 {
            deep = json!({"next": deep});
        }
        let errors = list.accept(&deep.to_string()).unwrap_err();
        assert!(errors[0].contains("deeper than"), "{errors:?}");
        // A self-reference with nothing to consume does not loop.
        let looping = format(json!({"$ref": "#"}));
        assert!(looping.check().is_ok());
        assert!(looping.accept("1").unwrap_err()[0].contains("deeper than"));
    }

    #[test]
    fn branching_back_to_the_root_is_bounded_by_steps_not_just_depth() {
        let bomb = format(json!({"anyOf": [{"$ref": "#"}, {"$ref": "#"}, {"type": "null"}]}));
        assert!(bomb.check().is_ok());
        let started = std::time::Instant::now();
        let errors = bomb.accept("1").unwrap_err();
        assert!(errors[0].contains("too complex"), "{errors:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn many_errors_are_summarised() {
        let schema = format(json!({"type": "array", "items": {"type": "string"}}));
        let errors = schema
            .accept(&json!((0..20).collect::<Vec<_>>()).to_string())
            .unwrap_err();
        assert_eq!(errors.len(), MAX_ERRORS + 1);
        assert_eq!(errors.last().unwrap(), "and 12 more");
    }
}
