//! What the runner does to the computer itself, outside the agent: open the
//! task's file, ask which app is in front, and clean up after the run.
//!
//! Behind a trait so the harness is tested without a Mac. [`MacDesktop`]
//! uses `open`, `lsappinfo`, `pgrep` and `osascript`; every command has a
//! deadline, and only a child this process started is ever killed.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub trait Desktop: Send + Sync {
    /// Opens a file or folder, with `app` or its default handler.
    fn open(&self, path: &Path, app: Option<&str>) -> Result<(), String>;
    fn app_running(&self, app: &str) -> Result<bool, String>;
    /// The name of the app receiving keystrokes.
    fn frontmost_app(&self) -> Result<Option<String>, String>;
    /// Quits an app the run found closed (the suite enforces that).
    fn quit_app(&self, app: &str) -> Result<(), String>;
    /// Closes, without saving, the app's documents whose path contains
    /// `marker` (the run's root); other documents are not touched.
    fn close_documents(&self, app: &str, marker: &str) -> Result<(), String>;
    /// Closes Finder windows showing a folder whose path contains `marker`.
    fn close_finder_windows(&self, marker: &str) -> Result<(), String>;
}

/// The person's Mac, through the system's own command-line tools.
pub struct MacDesktop;

const COMMAND_DEADLINE: Duration = Duration::from_secs(15);

impl Desktop for MacDesktop {
    fn open(&self, path: &Path, app: Option<&str>) -> Result<(), String> {
        let mut args = Vec::new();
        if let Some(app) = app {
            args.push("-a".to_string());
            args.push(app.to_string());
        }
        args.push(path.to_string_lossy().into_owned());
        run("/usr/bin/open", &args).map(|_| ())
    }

    fn app_running(&self, app: &str) -> Result<bool, String> {
        // pgrep exits 1 when nothing matches; only an exact process name.
        let output = run_status("/usr/bin/pgrep", &["-x".to_string(), app.to_string()])?;
        match output.0 {
            0 => Ok(true),
            1 => Ok(false),
            code => Err(format!("pgrep -x {app}: exit {code}: {}", output.2.trim())),
        }
    }

    fn frontmost_app(&self) -> Result<Option<String>, String> {
        // lsappinfo asks the launch services server, which needs no
        // Automation permission and does not go stale like NSWorkspace
        // without a run loop.
        let front = run("/usr/bin/lsappinfo", &["front".to_string()])?;
        let front = front.trim();
        if front.is_empty() || front == "[ NULL ]" {
            return Ok(None);
        }
        let info = run(
            "/usr/bin/lsappinfo",
            &[
                "info".to_string(),
                "-only".to_string(),
                "name".to_string(),
                front.to_string(),
            ],
        )?;
        Ok(parse_lsappinfo_name(&info))
    }

    fn quit_app(&self, app: &str) -> Result<(), String> {
        osascript(&format!(
            "if application \"{app}\" is running then tell application \"{app}\" to quit"
        ))
    }

    fn close_documents(&self, app: &str, marker: &str) -> Result<(), String> {
        let marker = applescript_text(marker);
        // Back to front by index, so closing one does not shift the rest;
        // an untitled document has no path and is skipped by the `try`.
        osascript(&format!(
            "if application \"{app}\" is running then\n\
             tell application \"{app}\"\n\
             repeat with i from (count of documents) to 1 by -1\n\
             try\n\
             if (path of document i) contains \"{marker}\" then close document i saving no\n\
             end try\n\
             end repeat\n\
             end tell\n\
             end if"
        ))
    }

    fn close_finder_windows(&self, marker: &str) -> Result<(), String> {
        let marker = applescript_text(marker);
        osascript(&format!(
            "if application \"Finder\" is running then\n\
             tell application \"Finder\"\n\
             repeat with i from (count of Finder windows) to 1 by -1\n\
             try\n\
             if (POSIX path of ((target of Finder window i) as alias)) contains \"{marker}\" then close Finder window i\n\
             end try\n\
             end repeat\n\
             end tell\n\
             end if"
        ))
    }
}

/// The app's name from `lsappinfo info -only name <asn>`, which prints
/// either `"LSDisplayName"="Calculator"` or, depending on the macOS
/// release, a full record whose first line is
/// `"Calculator" ASN:0x0-0x1234: (in front)`.
#[must_use]
pub fn parse_lsappinfo_name(info: &str) -> Option<String> {
    let usable = |value: &str| {
        let value = value.trim();
        (!value.is_empty() && value != "[ NULL ]").then(|| value.to_string())
    };
    for line in info.lines() {
        let line = line.trim();
        for key in ["\"LSDisplayName\"=", "\"name\"="] {
            if let Some(value) = line.strip_prefix(key) {
                return usable(value.trim().trim_matches('"'));
            }
        }
    }
    let first = info.lines().map(str::trim).find(|line| !line.is_empty())?;
    let quoted = first.strip_prefix('"')?;
    usable(&quoted[..quoted.find('"')?])
}

/// The inside of an AppleScript string literal.
fn applescript_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn osascript(script: &str) -> Result<(), String> {
    run(
        "/usr/bin/osascript",
        &["-e".to_string(), script.to_string()],
    )
    .map(|_| ())
}

fn run(program: &str, args: &[String]) -> Result<String, String> {
    let (code, stdout, stderr) = run_status(program, args)?;
    if code == 0 {
        Ok(stdout)
    } else {
        Err(format!(
            "{program}: exit {code}: {}",
            stderr.trim().chars().take(300).collect::<String>()
        ))
    }
}

/// Runs a command with [`COMMAND_DEADLINE`]; on timeout the child (and only
/// it) is killed.
fn run_status(program: &str, args: &[String]) -> Result<(i32, String, String), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > COMMAND_DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program}: no answer in {}s",
                    COMMAND_DEADLINE.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(format!("{program}: {err}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("{program}: {err}"))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_name_lsappinfo_prints() {
        assert_eq!(
            parse_lsappinfo_name("\"LSDisplayName\"=\"Calculator\"\n").as_deref(),
            Some("Calculator")
        );
        assert_eq!(
            parse_lsappinfo_name("\"name\"=\"TextEdit\"").as_deref(),
            Some("TextEdit")
        );
        assert_eq!(parse_lsappinfo_name(""), None);
        assert_eq!(parse_lsappinfo_name("\"name\"=[ NULL ]"), None);
        // As printed on macOS 27 (captured 2026-11-09).
        let record = "\"Google Chrome\" ASN:0x0-0x33033: (in front) \n    \
                      bundleID=[ NULL ] \n    bundle path=[ NULL ] \n";
        assert_eq!(
            parse_lsappinfo_name(record).as_deref(),
            Some("Google Chrome")
        );
    }

    /// Read-only: asks which app is in front, quits and opens nothing.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a logged-in macOS session"]
    fn the_app_in_front_is_named_on_a_real_mac() {
        let front = MacDesktop.frontmost_app().expect("lsappinfo");
        assert!(front.is_some_and(|name| !name.is_empty()));
    }

    #[test]
    fn a_marker_cannot_end_the_applescript_string() {
        assert_eq!(applescript_text(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn a_command_that_fails_says_how() {
        let err = run("/bin/sh", &["-c".into(), "echo nope >&2; exit 3".into()]).unwrap_err();
        assert!(err.contains("exit 3") && err.contains("nope"), "{err}");
        assert_eq!(
            run("/bin/sh", &["-c".into(), "printf ok".into()]).unwrap(),
            "ok"
        );
    }
}
