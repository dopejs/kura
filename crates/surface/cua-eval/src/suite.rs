//! The task suite: a versioned JSON file of desktop tasks, each with a
//! deterministic setup, a check of the computer's state that does not trust
//! the model's own account, a step budget, and a teardown.
//!
//! Everything a task touches lives in its own work directory under the
//! run's root, and every path in the file is relative to it: a suite cannot
//! name a file outside that directory, quit an app it did not find closed,
//! or close a window or document outside the run (see [`Suite::validate`]).

use std::collections::BTreeSet;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

/// The suite format this build reads.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Suite {
    /// Format version of the file ([`FORMAT_VERSION`]).
    pub format: u32,
    /// Name of the suite, e.g. `macos-desktop`.
    pub suite: String,
    /// Version of the task set. Bump it whenever a task changes, so results
    /// of different task sets are never compared as one.
    pub version: u32,
    /// `macos`, or `fake` for the harness's own tests.
    pub platform: String,
    /// Appended to every instruction, after a blank line.
    #[serde(default)]
    pub preamble: String,
    #[serde(default)]
    pub defaults: Defaults,
    pub verdict: VerdictRule,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Defaults {
    pub step_budget: u32,
    pub time_limit_s: u64,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            step_budget: 25,
            time_limit_s: 300,
        }
    }
}

/// When a run says the generic tools are good enough, and when it says a
/// native computer-use mapping is worth building (docs §12 slice 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerdictRule {
    /// Fewer counted runs per task than this: no verdict.
    pub min_runs_per_task: u32,
    /// Success rate at or above this, with the step ratio at or below
    /// `good_max_step_ratio`: good enough.
    pub good_success_rate: f64,
    pub good_max_step_ratio: f64,
    /// Success rate below this, or step ratio above `native_step_ratio`:
    /// a native mapping is worth building and measuring.
    pub native_success_rate: f64,
    pub native_step_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Task {
    /// Lowercase letters, digits and `-`; names the work directory.
    pub id: String,
    /// What the task exercises: `open-app`, `form`, `find-file`, `edit`,
    /// `navigate`, `drag`, ...
    pub category: String,
    /// What the person asks the agent. `{work}` is the task's work
    /// directory, `{workName}` its last component.
    pub instruction: String,
    /// The fewest `computer.act` calls a person would need, counted by
    /// hand; the step ratio divides by it.
    pub reference_steps: u32,
    /// More `computer.act` calls than this and the run is cancelled and
    /// fails as `over_budget`, whatever the computer's state.
    #[serde(default)]
    pub step_budget: Option<u32>,
    #[serde(default)]
    pub time_limit_s: Option<u64>,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub setup: Vec<SetupStep>,
    /// All must hold for the task to pass.
    pub check: Vec<Check>,
    /// Runs after every attempt, whatever happened; a failing step is
    /// recorded, not fatal.
    #[serde(default)]
    pub teardown: Vec<TeardownStep>,
}

impl Task {
    #[must_use]
    pub fn budget(&self, defaults: &Defaults) -> u32 {
        self.step_budget.unwrap_or(defaults.step_budget)
    }

    #[must_use]
    pub fn time_limit(&self, defaults: &Defaults) -> u64 {
        self.time_limit_s.unwrap_or(defaults.time_limit_s)
    }
}

/// Preconditions; when one does not hold the attempt is skipped, not run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requires {
    /// Apps that must not be running, so the check can tell the agent
    /// opened them and the teardown may quit them.
    #[serde(default)]
    pub apps_not_running: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum SetupStep {
    /// Writes a file (parents created). `{work}` is substituted.
    Write { path: String, content: String },
    /// Creates a directory and its parents.
    Mkdir(String),
    /// Opens a file or directory, with `app` or the default handler.
    Open {
        path: String,
        #[serde(default)]
        app: Option<String>,
    },
    /// Lets an app finish opening before the agent looks.
    Wait { ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum Check {
    /// The file's text equals `text`, ignoring trailing whitespace and
    /// line-ending style.
    FileEquals { path: String, text: String },
    /// The last non-empty line, trimmed, equals `text`.
    FileLastLine { path: String, text: String },
    /// For each field, the first line that starts with `<name>:` has
    /// `value` after the colon (trimmed; inner runs of spaces count as one).
    FormFields {
        path: String,
        fields: std::collections::BTreeMap<String, String>,
    },
    /// The path exists and is a `file` or a `dir`.
    Exists { path: String, kind: PathKind },
    /// Nothing exists at the path.
    Absent(String),
    /// The app receiving keystrokes, by process name.
    FrontmostApp(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum TeardownStep {
    /// Closes, without saving, the app's documents inside this run.
    CloseDocuments(String),
    /// Closes Finder windows that show a folder inside this run.
    CloseFinderWindows,
    /// Quits an app; allowed only for an app in `requires.appsNotRunning`.
    QuitApp(String),
}

/// What to substitute into a task's text.
#[derive(Debug, Clone)]
pub struct Placeholders<'a> {
    pub work: &'a Path,
}

impl Placeholders<'_> {
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        let work = self.work.to_string_lossy();
        let name = self
            .work
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        text.replace("{workName}", &name).replace("{work}", &work)
    }
}

impl Suite {
    /// Reads and validates a suite file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
        Self::parse(&text).map_err(|err| format!("{}: {err}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let suite: Self = serde_json::from_str(text).map_err(|err| err.to_string())?;
        suite.validate()?;
        Ok(suite)
    }

    /// Refuses a suite that could reach outside its work directories, has
    /// no way to fail, or that a run could not interpret the same way twice.
    /// Every problem is listed, not just the first.
    pub fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.format != FORMAT_VERSION {
            problems.push(format!(
                "format {} is not supported (this build reads {FORMAT_VERSION})",
                self.format
            ));
        }
        if !valid_id(&self.suite) {
            problems.push(format!("suite name {:?} is not a valid id", self.suite));
        }
        if self.version == 0 {
            problems.push("version must be 1 or more".into());
        }
        if self.platform != "macos" && self.platform != "fake" {
            problems.push(format!(
                "platform {:?} is not supported (macos or fake)",
                self.platform
            ));
        }
        if self.tasks.is_empty() {
            problems.push("the suite has no tasks".into());
        }
        if self.defaults.step_budget == 0 || self.defaults.time_limit_s == 0 {
            problems.push("defaults must be positive".into());
        }
        let rule = &self.verdict;
        let rates = [rule.good_success_rate, rule.native_success_rate];
        if rates.iter().any(|rate| !(0.0..=1.0).contains(rate))
            || rule.native_success_rate > rule.good_success_rate
            || rule.good_max_step_ratio < 1.0
            || rule.native_step_ratio < rule.good_max_step_ratio
            || rule.min_runs_per_task == 0
        {
            problems.push(
                "verdict: rates in 0..=1 with native <= good, step ratios >= 1 with good <= \
                 native, minRunsPerTask >= 1"
                    .into(),
            );
        }
        let mut ids = BTreeSet::new();
        for task in &self.tasks {
            let at = format!("task {:?}", task.id);
            if !valid_id(&task.id) {
                problems.push(format!("{at}: id must be lowercase letters, digits and -"));
            }
            if !ids.insert(task.id.as_str()) {
                problems.push(format!("{at}: duplicate id"));
            }
            if task.instruction.trim().is_empty() {
                problems.push(format!("{at}: empty instruction"));
            }
            if task.category.trim().is_empty() {
                problems.push(format!("{at}: empty category"));
            }
            let budget = task.budget(&self.defaults);
            if task.reference_steps == 0 || budget < task.reference_steps {
                problems.push(format!(
                    "{at}: referenceSteps must be >= 1 and <= the step budget ({budget})"
                ));
            }
            let limit = task.time_limit(&self.defaults);
            if !(10..=1800).contains(&limit) {
                problems.push(format!("{at}: time limit must be 10..=1800 s"));
            }
            if task.check.is_empty() {
                problems.push(format!("{at}: a task needs at least one check"));
            }
            for step in &task.setup {
                match step {
                    SetupStep::Write { path, .. } => check_path(&at, path, true, &mut problems),
                    // `.` opens the work directory itself (in Finder).
                    SetupStep::Open { path, .. } | SetupStep::Mkdir(path) => {
                        check_path(&at, path, false, &mut problems);
                    }
                    SetupStep::Wait { ms } if *ms > 10_000 => {
                        problems.push(format!("{at}: a setup wait is at most 10000 ms"));
                    }
                    SetupStep::Wait { .. } => {}
                }
            }
            for check in &task.check {
                match check {
                    Check::FileEquals { path, .. }
                    | Check::FileLastLine { path, .. }
                    | Check::FormFields { path, .. }
                    | Check::Exists { path, .. }
                    | Check::Absent(path) => check_path(&at, path, false, &mut problems),
                    Check::FrontmostApp(app) => check_app(&at, app, &mut problems),
                }
                if let Check::FormFields { fields, .. } = check {
                    if fields.is_empty() || fields.keys().any(|name| name.contains(':')) {
                        problems.push(format!(
                            "{at}: formFields needs fields, and names without ':'"
                        ));
                    }
                }
            }
            for app in &task.requires.apps_not_running {
                check_app(&at, app, &mut problems);
            }
            for step in &task.teardown {
                match step {
                    TeardownStep::QuitApp(app) if !task.requires.apps_not_running.contains(app) => {
                        problems.push(format!(
                            "{at}: quitApp {app:?} needs {app:?} in requires.appsNotRunning, \
                             so a run never quits an app the person had open"
                        ));
                    }
                    TeardownStep::QuitApp(app) | TeardownStep::CloseDocuments(app) => {
                        check_app(&at, app, &mut problems);
                    }
                    TeardownStep::CloseFinderWindows => {}
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// A path inside the work directory: relative, no `..`, and (when
/// `must_name`) not the directory itself.
fn check_path(at: &str, path: &str, must_name: bool, problems: &mut Vec<String>) {
    let parsed = Path::new(path);
    let inside = !path.contains('{')
        && parsed
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if !inside || (must_name && path.trim_matches(['.', '/']).is_empty()) {
        problems.push(format!(
            "{at}: path {path:?} must be relative to the work directory, without .."
        ));
    }
}

/// An app name goes into AppleScript and process lookups: plain names only.
fn check_app(at: &str, app: &str, problems: &mut Vec<String>) {
    let plain = !app.trim().is_empty()
        && app
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '.');
    if !plain {
        problems.push(format!(
            "{at}: app {app:?} must be letters, digits, spaces, '-' or '.'"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "format": 1,
            "suite": "t",
            "version": 1,
            "platform": "fake",
            "verdict": {
                "minRunsPerTask": 1,
                "goodSuccessRate": 0.8, "goodMaxStepRatio": 2.0,
                "nativeSuccessRate": 0.6, "nativeStepRatio": 3.0
            },
            "tasks": [{
                "id": "a",
                "category": "edit",
                "instruction": "do it in {work}",
                "referenceSteps": 2,
                "setup": [{"write": {"path": "x.txt", "content": ""}}],
                "check": [{"fileEquals": {"path": "x.txt", "text": "hi"}}]
            }]
        })
    }

    fn with(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Suite, String> {
        let mut value = minimal();
        edit(&mut value);
        Suite::parse(&value.to_string())
    }

    #[test]
    fn the_shipped_macos_suite_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("suites/macos-desktop.v1.json");
        let suite = Suite::load(&path).expect("valid suite");
        assert_eq!(suite.platform, "macos");
        assert!(
            (6..=10).contains(&suite.tasks.len()),
            "{}",
            suite.tasks.len()
        );
        // The categories slice 5 asks for are covered.
        for category in ["open-app", "form", "find-file"] {
            assert!(
                suite.tasks.iter().any(|task| task.category == category),
                "{category}"
            );
        }
    }

    #[test]
    fn a_minimal_suite_parses_and_fills_defaults() {
        let suite = with(|_| {}).expect("valid");
        assert_eq!(suite.tasks[0].budget(&suite.defaults), 25);
        assert_eq!(suite.tasks[0].time_limit(&suite.defaults), 300);
    }

    #[test]
    fn paths_cannot_leave_the_work_directory() {
        for bad in ["../x", "/etc/passwd", "a/../../b", "{work}/x"] {
            let err =
                with(|v| v["tasks"][0]["check"][0]["fileEquals"]["path"] = bad.into()).unwrap_err();
            assert!(err.contains("must be relative"), "{bad}: {err}");
        }
        let err = with(|v| v["tasks"][0]["setup"][0]["write"]["path"] = ".".into()).unwrap_err();
        assert!(err.contains("must be relative"), "{err}");
    }

    #[test]
    fn a_task_may_quit_only_an_app_it_found_closed() {
        let err = with(|v| v["tasks"][0]["teardown"] = serde_json::json!([{"quitApp": "Mail"}]))
            .unwrap_err();
        assert!(err.contains("requires.appsNotRunning"), "{err}");
        with(|v| {
            v["tasks"][0]["teardown"] = serde_json::json!([{"quitApp": "Calculator"}]);
            v["tasks"][0]["requires"] = serde_json::json!({"appsNotRunning": ["Calculator"]});
        })
        .expect("allowed");
        let err = with(|v| {
            v["tasks"][0]["requires"] =
                serde_json::json!({"appsNotRunning": ["Calc\" to do shell script \"x"]});
        })
        .unwrap_err();
        assert!(err.contains("must be letters"), "{err}");
    }

    #[test]
    fn every_problem_is_reported_and_unknown_keys_are_refused() {
        let err = with(|v| {
            v["format"] = 2.into();
            v["tasks"][0]["referenceSteps"] = 0.into();
            v["tasks"][0]["check"] = serde_json::json!([]);
            let copy = v["tasks"][0].clone();
            v["tasks"].as_array_mut().unwrap().push(copy);
        })
        .unwrap_err();
        for part in [
            "format 2",
            "referenceSteps",
            "at least one check",
            "duplicate id",
        ] {
            assert!(err.contains(part), "{part}: {err}");
        }
        let err = with(|v| v["tasks"][0]["stepBudegt"] = 3.into()).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn placeholders_name_the_work_directory() {
        let work = Path::new("/tmp/run-1/save-note-r1");
        let text = Placeholders { work }.apply("open {work}/a in {workName}");
        assert_eq!(text, "open /tmp/run-1/save-note-r1/a in save-note-r1");
    }
}
