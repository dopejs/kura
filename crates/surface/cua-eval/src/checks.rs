//! Whether a task was done, read from the computer's state: files in the
//! task's work directory and the app in front. Never from the model's
//! reply.

use std::path::Path;

use serde::Serialize;

use crate::desktop::Desktop;
use crate::suite::{Check, PathKind};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// The check as written in the suite, for the report.
    pub check: String,
    pub passed: bool,
    /// What was found, when it was not what the check wanted.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Runs every check (not stopping at the first failure, so the report
/// shows all of them).
pub fn run_checks(checks: &[Check], work: &Path, desktop: &dyn Desktop) -> Vec<CheckResult> {
    checks
        .iter()
        .map(|check| {
            let outcome = run_check(check, work, desktop);
            CheckResult {
                check: describe(check),
                passed: outcome.is_ok(),
                detail: outcome.err().unwrap_or_default(),
            }
        })
        .collect()
}

fn describe(check: &Check) -> String {
    serde_json::to_string(check).unwrap_or_else(|_| format!("{check:?}"))
}

fn run_check(check: &Check, work: &Path, desktop: &dyn Desktop) -> Result<(), String> {
    match check {
        Check::FileEquals { path, text } => {
            let found = read(work, path)?;
            if normalize(&found) == normalize(text) {
                Ok(())
            } else {
                Err(format!("found {}", excerpt(&found)))
            }
        }
        Check::FileLastLine { path, text } => {
            let found = read(work, path)?;
            let last = found
                .lines()
                .map(str::trim)
                .rfind(|line| !line.is_empty())
                .unwrap_or("");
            if last == text.trim() {
                Ok(())
            } else {
                Err(format!("last line is {}", excerpt(last)))
            }
        }
        Check::FormFields { path, fields } => {
            let found = read(work, path)?;
            let mut wrong = Vec::new();
            for (name, want) in fields {
                let prefix = format!("{name}:");
                let got = found
                    .lines()
                    .map(str::trim)
                    .find(|line| line.starts_with(&prefix))
                    .map(|line| squash(&line[prefix.len()..]));
                match got {
                    Some(got) if got == squash(want) => {}
                    Some(got) => wrong.push(format!("{name}={}", excerpt(&got))),
                    None => wrong.push(format!("{name} missing")),
                }
            }
            if wrong.is_empty() {
                Ok(())
            } else {
                Err(wrong.join(", "))
            }
        }
        Check::Exists { path, kind } => {
            let full = work.join(path);
            let ok = match kind {
                PathKind::File => full.is_file(),
                PathKind::Dir => full.is_dir(),
            };
            if ok {
                Ok(())
            } else if full.exists() {
                Err("exists, but is not that kind".into())
            } else {
                Err("missing".into())
            }
        }
        Check::Absent(path) => {
            // symlink_metadata: a dangling link still counts as present.
            if std::fs::symlink_metadata(work.join(path)).is_err() {
                Ok(())
            } else {
                Err("still there".into())
            }
        }
        Check::FrontmostApp(app) => match desktop.frontmost_app()? {
            Some(front) if front == *app => Ok(()),
            Some(front) => Err(format!("{front} is in front")),
            None => Err("no app is in front".into()),
        },
    }
}

fn read(work: &Path, path: &str) -> Result<String, String> {
    let full = work.join(path);
    let bytes = std::fs::read(&full).map_err(|err| format!("{path}: {err}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_string()
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn excerpt(text: &str) -> String {
    let cut: String = text.chars().take(120).collect();
    if cut.len() < text.len() {
        format!("{cut:?}…")
    } else {
        format!("{cut:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Front(Option<&'static str>);

    impl Desktop for Front {
        fn open(&self, _: &Path, _: Option<&str>) -> Result<(), String> {
            Ok(())
        }
        fn app_running(&self, _: &str) -> Result<bool, String> {
            Ok(false)
        }
        fn frontmost_app(&self) -> Result<Option<String>, String> {
            Ok(self.0.map(str::to_string))
        }
        fn quit_app(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn close_documents(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn close_finder_windows(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn check(json: serde_json::Value, work: &Path, front: Option<&'static str>) -> CheckResult {
        let check: Check = serde_json::from_value(json).unwrap();
        run_checks(&[check], work, &Front(front)).remove(0)
    }

    #[test]
    fn file_checks_read_the_file_and_say_what_they_found() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path();
        std::fs::write(work.join("a.txt"), "hello\r\n\n").unwrap();
        assert!(
            check(
                serde_json::json!({"fileEquals": {"path": "a.txt", "text": "hello"}}),
                work,
                None
            )
            .passed
        );
        let miss = check(
            serde_json::json!({"fileEquals": {"path": "a.txt", "text": "bye"}}),
            work,
            None,
        );
        assert!(!miss.passed && miss.detail.contains("hello"), "{miss:?}");
        let gone = check(
            serde_json::json!({"fileEquals": {"path": "b.txt", "text": ""}}),
            work,
            None,
        );
        assert!(!gone.passed && gone.detail.starts_with("b.txt"), "{gone:?}");

        std::fs::write(work.join("long.txt"), "CODE: 4817\nmore\n  4817  \n\n").unwrap();
        assert!(
            check(
                serde_json::json!({"fileLastLine": {"path": "long.txt", "text": "4817"}}),
                work,
                None
            )
            .passed
        );
        assert!(
            !check(
                serde_json::json!({"fileLastLine": {"path": "long.txt", "text": "more"}}),
                work,
                None
            )
            .passed
        );
    }

    #[test]
    fn form_fields_tolerate_spacing_but_not_wrong_values() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path();
        std::fs::write(work.join("f.txt"), "Name:Ada   Lovelace\nCity: Paris\n").unwrap();
        let result = check(
            serde_json::json!({"formFields": {"path": "f.txt", "fields": {
                "Name": "Ada Lovelace", "City": "London", "Phone": "1"
            }}}),
            work,
            None,
        );
        assert!(!result.passed);
        assert_eq!(result.detail, "City=\"Paris\", Phone missing");
    }

    #[test]
    fn existence_and_the_app_in_front() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path();
        std::fs::create_dir(work.join("Done")).unwrap();
        assert!(
            check(
                serde_json::json!({"exists": {"path": "Done", "kind": "dir"}}),
                work,
                None
            )
            .passed
        );
        let kind = check(
            serde_json::json!({"exists": {"path": "Done", "kind": "file"}}),
            work,
            None,
        );
        assert_eq!(kind.detail, "exists, but is not that kind");
        assert!(!check(serde_json::json!({"absent": "Done"}), work, None).passed);
        assert!(check(serde_json::json!({"absent": "nope"}), work, None).passed);
        assert!(
            check(
                serde_json::json!({"frontmostApp": "Calculator"}),
                work,
                Some("Calculator")
            )
            .passed
        );
        let front = check(
            serde_json::json!({"frontmostApp": "Calculator"}),
            work,
            Some("Finder"),
        );
        assert_eq!(front.detail, "Finder is in front");
    }
}
