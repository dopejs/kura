//! The report a round ends with (work-items.md §3.4), and the question a
//! round is asked.

use serde_json::{Value, json};

use crate::types::{Attention, Report, ReportReason, ReportStatus};

/// The name the report schema goes by in a round's `responseFormat`.
pub const REPORT_FORMAT_NAME: &str = "work_item_report";

/// Longest summary and question a report may carry, matching the schema.
const MAX_SUMMARY_CHARS: usize = 4000;

/// The schema a round's reply must conform to. The same constraints as
/// `schemas/api/work-item-report.schema.json`, without its annotations: it
/// rides on every model call of the round. Only keywords the structured
/// output validator enforces are used (`kura_llm::ResponseFormat::check`).
#[must_use]
pub fn report_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["status", "summary"],
        "properties": {
            "status": {
                "type": "string",
                "enum": ["progress", "waiting", "ready", "blocked", "failed", "dropped"]
            },
            "summary": {"type": "string", "minLength": 1, "maxLength": MAX_SUMMARY_CHARS},
            "attention": {
                "type": "object",
                "additionalProperties": false,
                "required": ["question"],
                "properties": {
                    "question": {"type": "string", "minLength": 1, "maxLength": 2000},
                    "options": {
                        "type": "array",
                        "maxItems": 8,
                        "items": {"type": "string", "minLength": 1, "maxLength": 200}
                    }
                }
            }
        }
    })
}

/// The report in a round's structured reply. The chat service already
/// validated the reply against [`report_schema`]; this reads it again
/// rather than trusting that, because a report that is wrongly accepted
/// would move the item on what the model never said.
pub fn read_report(structured: &Value) -> Result<Report, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        status: ReportStatus,
        summary: String,
        #[serde(default)]
        attention: Option<Attention>,
    }
    let wire: Wire = serde_json::from_value(structured.clone())
        .map_err(|err| format!("the report does not match its schema: {err}"))?;
    if wire.summary.trim().is_empty() {
        return Err("the report's summary is empty".to_string());
    }
    if wire
        .attention
        .as_ref()
        .is_some_and(|attention| attention.question.trim().is_empty())
    {
        return Err("the report's attention has no question".to_string());
    }
    Ok(Report {
        status: wire.status,
        summary: wire.summary,
        attention: wire.attention,
        reason: None,
    })
}

/// What is recorded when a round's reply could not be read as a report:
/// `blocked`, so a person sees the item instead of it idling silently.
#[must_use]
pub fn unreadable_report(detail: &str) -> Report {
    let mut summary = format!("The round's report could not be read: {}", detail.trim());
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        summary = summary.chars().take(MAX_SUMMARY_CHARS).collect();
    }
    Report {
        status: ReportStatus::Blocked,
        summary,
        attention: None,
        reason: Some(ReportReason::ReportUnreadable),
    }
}

/// The question a round is asked: the item, what was asked of it, and how
/// to end the round. Stored before the turn starts, so a rerun of the round
/// asks exactly the same and finds its checkpoint (§6).
///
/// The scope is the requester's own words. It sits in the question while
/// every input is the requester's (slice 1); it moves to the system frame
/// when inputs from third parties arrive (slice 2).
#[must_use]
pub fn round_query(title: &str, scope: &str, seq: i64) -> String {
    format!(
        "You are working on a work item: a piece of work that may take more than one round. \
         This is round {seq}.\n\n\
         Title: {title}\n\n\
         What you were asked to do:\n{scope}\n\n\
         Do as much of the work as you can in this round. Then end the round with your report:\n\
         - \"ready\": the work is done.\n\
         - \"progress\": you made progress and will go on when something new arrives.\n\
         - \"waiting\": nothing can happen until an outside event arrives.\n\
         - \"blocked\": you cannot go on without a person. Ask them in \"attention\".\n\
         - \"failed\": the work cannot be done.\n\
         - \"dropped\": the work is no longer wanted.\n\
         The summary says what you did and where the work stands, for the person who asked. \
         Set \"attention\" only when you need a person's answer to go on, with one question.",
        title = title.trim(),
        scope = scope.trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_one_the_chat_service_can_enforce() {
        let format = kura_llm::ResponseFormat {
            name: REPORT_FORMAT_NAME.to_string(),
            schema: report_schema(),
        };
        format.check().expect("enforceable");
        assert!(
            format
                .accept(r#"{"status":"waiting","summary":"Waiting for CI."}"#)
                .is_ok()
        );
        for bad in [
            r#"{"status":"done","summary":"x"}"#,
            r#"{"status":"ready","summary":""}"#,
            r#"{"status":"ready","summary":"x","extra":1}"#,
            r#"{"status":"blocked","summary":"x","attention":{}}"#,
            "I finished the work.",
        ] {
            assert!(format.accept(bad).is_err(), "accepted {bad}");
        }
    }

    /// The contract file and the schema sent to the model must not drift:
    /// they are the same constraints, the file adding only annotations.
    #[test]
    fn the_schema_matches_the_published_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../schemas/api/work-item-report.schema.json");
        let text = std::fs::read_to_string(&path).expect("read the contract");
        let mut published: Value = serde_json::from_str(&text).expect("contract json");
        strip_annotations(&mut published);
        assert_eq!(published, report_schema());
    }

    fn strip_annotations(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for key in ["$schema", "$id", "title", "description"] {
                    map.remove(key);
                }
                for (key, member) in map.iter_mut() {
                    // A property may itself be named like an annotation.
                    if key != "properties" {
                        strip_annotations(member);
                    } else if let Value::Object(properties) = member {
                        properties.values_mut().for_each(strip_annotations);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(strip_annotations),
            _ => {}
        }
    }

    #[test]
    fn a_conforming_report_reads_and_a_strange_one_does_not() {
        let report = read_report(&json!({
            "status": "blocked",
            "summary": "Two fixes are possible.",
            "attention": {"question": "Which one?", "options": ["pin", "patch"]}
        }))
        .unwrap();
        assert_eq!(report.status, ReportStatus::Blocked);
        assert_eq!(report.attention.unwrap().options, ["pin", "patch"]);
        assert!(report.reason.is_none());

        for bad in [
            json!({"status": "ready"}),
            json!({"status": "ready", "summary": "  "}),
            json!({"status": "ready", "summary": "x", "reason": "report_unreadable"}),
            json!({"status": "blocked", "summary": "x", "attention": {"question": " "}}),
            json!("ready"),
        ] {
            assert!(read_report(&bad).is_err(), "read {bad}");
        }
    }

    #[test]
    fn an_unreadable_report_is_blocked_and_says_why() {
        let report = unreadable_report(&"x".repeat(10_000));
        assert_eq!(report.status, ReportStatus::Blocked);
        assert_eq!(report.reason, Some(ReportReason::ReportUnreadable));
        assert_eq!(report.summary.chars().count(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn the_round_query_is_stable_for_the_same_round() {
        let a = round_query("Fix CI", " Find why main fails. ", 1);
        assert_eq!(a, round_query("Fix CI", "Find why main fails.", 1));
        assert!(a.contains("round 1."));
        assert!(a.contains("Find why main fails."));
        assert_ne!(a, round_query("Fix CI", "Find why main fails.", 2));
    }
}
