//! External plugin process host (tier 2, pluginization phase 3).
//!
//! Each enabled external plugin (discovered from
//! `<data_dir>/plugins/<dir>/manifest.json`) runs as a supervised stdio
//! child speaking a line-JSON protocol:
//!
//! - request (one line):  `{"point": "<hook point>", "payload": {...}}`
//! - response (one line): `{"outcome": "continue"|"halt", "reason": "...",
//!   "payload": {...}}` — `payload` is optional; when present it replaces
//!   the hook payload (that is how an external plugin rewrites context).
//!
//! The child is spawned lazily on the first hook call and respawned once per
//! call if it died. Calls are serialized per process and bounded by the
//! manifest's `timeoutMs`. Failures follow the hook's declared `onError`
//! policy: `continue` (availability first) or `veto` (fail closed, for
//! policy plugins).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use kura_plugin::{ExternalPlugin, Hook, HookErrorPolicy, HookOutcome};

/// Default per-hook-call timeout when the manifest leaves `timeoutMs` unset.
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_millis(2000);

/// One external plugin's process handle.
pub(crate) struct ExternalProcessHost {
    pub id: String,
    command: String,
    args: Vec<String>,
    dir: std::path::PathBuf,
    timeout: Duration,
    inner: parking_lot::Mutex<Option<Running>>,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

/// The child's parsed response line.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Reply {
    outcome: String,
    reason: String,
    payload: Option<serde_json::Value>,
}

impl Default for Reply {
    fn default() -> Self {
        Reply {
            outcome: "continue".to_string(),
            reason: String::new(),
            payload: None,
        }
    }
}

impl ExternalProcessHost {
    pub fn new(plugin: &ExternalPlugin) -> Arc<Self> {
        let timeout = if plugin.manifest.entry.timeout_ms == 0 {
            DEFAULT_CALL_TIMEOUT
        } else {
            Duration::from_millis(plugin.manifest.entry.timeout_ms)
        };
        Arc::new(ExternalProcessHost {
            id: plugin.manifest.id.trim().to_string(),
            command: plugin.manifest.entry.command.clone(),
            args: plugin.manifest.entry.args.clone(),
            dir: plugin.dir.clone(),
            timeout,
            inner: parking_lot::Mutex::new(None),
        })
    }

    fn spawn(&self) -> Result<Running, String> {
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("spawn {}: {err}", self.command))?;
        let stdin = child.stdin.take().ok_or("child stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
        let (tx, lines) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("plugin-{}", self.id))
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| format!("spawn reader thread: {err}"))?;
        Ok(Running {
            child,
            stdin,
            lines,
        })
    }

    /// One hook round-trip. Serializes calls per process; a dead child is
    /// respawned once for this call.
    fn call(&self, point: &str, payload: &serde_json::Value) -> Result<Reply, String> {
        let mut guard = self.inner.lock();
        // Drop a dead child so this call respawns.
        if let Some(running) = guard.as_mut() {
            if running.child.try_wait().ok().flatten().is_some() {
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        let running = guard.as_mut().expect("spawned above");
        let request = serde_json::json!({ "point": point, "payload": payload }).to_string();
        if let Err(err) = writeln!(running.stdin, "{request}").and_then(|()| running.stdin.flush())
        {
            let _ = running.child.kill();
            *guard = None;
            return Err(format!("write request: {err}"));
        }
        match running.lines.recv_timeout(self.timeout) {
            Ok(line) => {
                serde_json::from_str::<Reply>(&line).map_err(|err| format!("parse response: {err}"))
            }
            Err(_) => {
                let _ = running.child.kill();
                *guard = None;
                Err(format!("no response within {:?}", self.timeout))
            }
        }
    }

    /// Kills the child if running. Idempotent.
    pub fn close(&self) {
        if let Some(mut running) = self.inner.lock().take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
    }
}

/// One registered hook proxying to an external plugin process.
pub(crate) struct ExternalHook {
    host: Arc<ExternalProcessHost>,
    point: String,
    on_error: HookErrorPolicy,
}

impl ExternalHook {
    pub fn new(host: Arc<ExternalProcessHost>, point: &str, on_error: HookErrorPolicy) -> Self {
        ExternalHook {
            host,
            point: point.to_string(),
            on_error,
        }
    }
}

impl Hook for ExternalHook {
    fn handle(&self, payload: &mut serde_json::Value) -> HookOutcome {
        match self.host.call(&self.point, payload) {
            Ok(reply) => {
                if let Some(new_payload) = reply.payload {
                    *payload = new_payload;
                }
                if reply.outcome == "halt" {
                    let reason = if reply.reason.trim().is_empty() {
                        "vetoed by external plugin".to_string()
                    } else {
                        reply.reason
                    };
                    HookOutcome::Halt(reason)
                } else {
                    HookOutcome::Continue
                }
            }
            Err(err) => match self.on_error {
                HookErrorPolicy::Continue => {
                    eprintln!(
                        "[kura] external plugin {} hook {} failed (continuing): {err}",
                        self.host.id, self.point
                    );
                    HookOutcome::Continue
                }
                HookErrorPolicy::Veto => HookOutcome::Halt(format!(
                    "external plugin {} unavailable: {err}",
                    self.host.id
                )),
            },
        }
    }
}

/// The `context.embedder` seam served by an external plugin process:
/// `embed` round-trips over the line-JSON channel (point
/// `seam:context.embedder:embed`, payload `{text}` → `{payload: {vector}}`).
/// Failures fall back to the deterministic in-process embedder — retrieval
/// quality degrades, availability does not (the fallback is logged).
pub(crate) struct ExternalEmbedder {
    host: Arc<ExternalProcessHost>,
    fallback: kura_context::HashedNgramEmbedder,
}

impl ExternalEmbedder {
    pub fn new(host: Arc<ExternalProcessHost>) -> Self {
        ExternalEmbedder {
            host,
            fallback: kura_context::HashedNgramEmbedder::default(),
        }
    }
}

impl kura_context::Embedder for ExternalEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        match self.host.call(
            "seam:context.embedder:embed",
            &serde_json::json!({ "text": text }),
        ) {
            Ok(reply) => {
                let vector = reply
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("vector"))
                    .and_then(|v| serde_json::from_value::<Vec<f32>>(v.clone()).ok());
                match vector {
                    Some(vector) if !vector.is_empty() => vector,
                    _ => {
                        eprintln!(
                            "[kura] external embedder {} returned no vector; using fallback",
                            self.host.id
                        );
                        kura_context::Embedder::embed(&self.fallback, text)
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "[kura] external embedder {} failed ({err}); using fallback",
                    self.host.id
                );
                kura_context::Embedder::embed(&self.fallback, text)
            }
        }
    }

    fn name(&self) -> &str {
        &self.host.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_with_script(
        script: &str,
        timeout_ms: u64,
    ) -> (tempfile_dir::TempDirGuard, ExternalPlugin) {
        let dir = tempfile_dir::tempdir();
        std::fs::write(dir.path.join("run.sh"), script).expect("write script");
        let plugin = ExternalPlugin {
            manifest: kura_plugin::PluginManifest {
                id: "ext-test".to_string(),
                entry: kura_plugin::ManifestEntry {
                    kind: "process".to_string(),
                    command: "/bin/sh".to_string(),
                    args: vec!["run.sh".to_string()],
                    timeout_ms,
                },
                ..Default::default()
            },
            dir: dir.path.clone(),
        };
        (dir, plugin)
    }

    /// Minimal temp-dir guard (avoids a dev-dependency): unique dir removed
    /// on drop.
    mod tempfile_dir {
        pub struct TempDirGuard {
            pub path: std::path::PathBuf,
        }
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
        pub fn tempdir() -> TempDirGuard {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kura-external-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDirGuard { path }
        }
    }

    #[test]
    fn round_trip_mutation_and_repeated_calls() {
        let (_guard, plugin) = plugin_with_script(
            "while read line; do printf '%s\\n' '{\"outcome\":\"continue\",\"payload\":{\"query\":\"rewritten\"}}'; done\n",
            2000,
        );
        let host = ExternalProcessHost::new(&plugin);
        let hook = ExternalHook::new(host.clone(), "chat/turn-start", HookErrorPolicy::Continue);
        for _ in 0..3 {
            let mut payload = serde_json::json!({ "query": "original" });
            assert!(matches!(hook.handle(&mut payload), HookOutcome::Continue));
            assert_eq!(payload["query"], "rewritten");
        }
        host.close();
    }

    #[test]
    fn halt_reply_vetoes() {
        let (_guard, plugin) = plugin_with_script(
            "while read line; do printf '%s\\n' '{\"outcome\":\"halt\",\"reason\":\"policy says no\"}'; done\n",
            2000,
        );
        let host = ExternalProcessHost::new(&plugin);
        let hook = ExternalHook::new(host.clone(), "chat/pre-dispatch", HookErrorPolicy::Continue);
        let mut payload = serde_json::json!({});
        match hook.handle(&mut payload) {
            HookOutcome::Halt(reason) => assert_eq!(reason, "policy says no"),
            HookOutcome::Continue => panic!("expected halt"),
        }
        host.close();
    }

    #[test]
    fn timeout_follows_on_error_policy() {
        // The child reads but never answers: every call times out.
        let (_guard, plugin) = plugin_with_script("while read line; do :; done\n", 150);
        let host = ExternalProcessHost::new(&plugin);
        let lenient = ExternalHook::new(host.clone(), "chat/turn-end", HookErrorPolicy::Continue);
        let mut payload = serde_json::json!({});
        assert!(matches!(
            lenient.handle(&mut payload),
            HookOutcome::Continue
        ));

        let strict = ExternalHook::new(host.clone(), "chat/pre-dispatch", HookErrorPolicy::Veto);
        match strict.handle(&mut payload) {
            HookOutcome::Halt(reason) => assert!(reason.contains("unavailable"), "{reason}"),
            HookOutcome::Continue => panic!("veto policy must fail closed"),
        }
        host.close();
    }

    #[test]
    fn spawn_failure_follows_policy() {
        let dir = tempfile_dir::tempdir();
        let plugin = ExternalPlugin {
            manifest: kura_plugin::PluginManifest {
                id: "ghost".to_string(),
                entry: kura_plugin::ManifestEntry {
                    kind: "process".to_string(),
                    command: "/nonexistent/binary".to_string(),
                    args: vec![],
                    timeout_ms: 200,
                },
                ..Default::default()
            },
            dir: dir.path.clone(),
        };
        let host = ExternalProcessHost::new(&plugin);
        let strict = ExternalHook::new(host, "chat/pre-dispatch", HookErrorPolicy::Veto);
        let mut payload = serde_json::json!({});
        assert!(matches!(strict.handle(&mut payload), HookOutcome::Halt(_)));
    }
}
