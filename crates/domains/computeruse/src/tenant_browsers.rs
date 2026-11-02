//! A throwaway browser per tenant (agent-runtime-program.md 3.3; design:
//! `docs/harness/tenant-browser.md`).
//!
//! Each tenant has at most one live browser: a worker (PROTOCOL.md) that a
//! [`BrowserLauncher`] started somewhere isolated, in production the
//! tenant's sandboxed container. The pool caps how many run at once, ends
//! the ones left idle, and keeps nothing between sessions. Every call is
//! blocking; callers on an async runtime run it on a blocking thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;

use crate::{
    Action, ActionKind, ActionStatus, ArtifactKind, AttachedWorker, CreateSessionInput, Driver,
    Session, SessionStatus,
};

/// Starts and stops browser workers. The sandbox in production; a fake
/// process in tests.
pub trait BrowserLauncher: Send + Sync {
    /// Starts a worker for `tenant_id`.
    fn launch(&self, tenant_id: &str) -> Result<LaunchedBrowser, String>;
    /// Ends what `launch` started, by its handle. Best effort and idempotent.
    fn stop(&self, handle: &str);
}

/// A started worker: its pipes as a driver, and the handle to end it by
/// (the sandbox execution id).
pub struct LaunchedBrowser {
    pub handle: String,
    pub worker: AttachedWorker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserLimits {
    /// Browsers running at once, daemon-wide.
    pub max_sessions: usize,
    /// A browser not used for this long is closed.
    pub idle: Duration,
}

/// What the model sees of a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
}

/// An action on the open page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserOp {
    Read,
    Click { selector: String },
    Type { selector: String, text: String },
    Back,
}

pub struct TenantBrowsers {
    launcher: Arc<dyn BrowserLauncher>,
    limits: BrowserLimits,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    active: AtomicUsize,
    next_action: AtomicU64,
}

/// One tenant's browser. The mutex serializes the tenant's calls, so one
/// page is never driven by two turns at once.
#[derive(Default)]
struct Slot {
    live: Mutex<Option<Live>>,
}

struct Live {
    handle: String,
    worker: AttachedWorker,
    session: Session,
    last_used: Instant,
}

impl TenantBrowsers {
    #[must_use]
    pub fn new(launcher: Arc<dyn BrowserLauncher>, limits: BrowserLimits) -> Arc<Self> {
        Arc::new(Self {
            launcher,
            limits,
            slots: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            next_action: AtomicU64::new(1),
        })
    }

    #[must_use]
    pub fn limits(&self) -> BrowserLimits {
        self.limits
    }

    /// Browsers running now.
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Loads `url` in the tenant's browser, starting one if it has none.
    pub fn open(&self, tenant_id: &str, url: &str) -> Result<Page, String> {
        let url = checked_url(url)?;
        let tenant = tenant_key(tenant_id)?;
        let slot = self.slot(&tenant);
        let mut live = slot.live.lock();
        if live.as_ref().is_some_and(|l| !l.worker.alive()) {
            self.teardown(&tenant, live.take().expect("checked"), "failed");
        }
        if live.is_none() {
            // Started blank, then navigated: a page that fails to load is
            // then a failed action on a live browser, not a browser thrown
            // away and started again on the next call.
            *live = Some(self.start(&tenant)?);
        }
        self.run(&tenant, &mut live, ActionKind::Navigate, &[("url", &url)])
    }

    /// Acts on the tenant's open page.
    pub fn act(&self, tenant_id: &str, op: &BrowserOp) -> Result<Page, String> {
        let tenant = tenant_key(tenant_id)?;
        let slot = self.slot(&tenant);
        let mut live = slot.live.lock();
        match live.as_ref() {
            None => return Err("no browser is open; call browser.open first".to_string()),
            Some(l) if !l.worker.alive() => {
                self.teardown(&tenant, live.take().expect("checked"), "failed");
                return Err("the browser stopped; open the page again".to_string());
            }
            Some(_) => {}
        }
        match op {
            BrowserOp::Read => {
                let session = live.as_ref().expect("checked").session.clone();
                self.page(&tenant, &mut live, session)
            }
            BrowserOp::Back => self.run(&tenant, &mut live, ActionKind::Back, &[]),
            BrowserOp::Click { selector } => self.run(
                &tenant,
                &mut live,
                ActionKind::Click,
                &[("selector", selector)],
            ),
            BrowserOp::Type { selector, text } => self.run(
                &tenant,
                &mut live,
                ActionKind::Input,
                &[("selector", selector), ("value", text)],
            ),
        }
    }

    /// Ends the tenant's browser. False when it had none.
    pub fn close(&self, tenant_id: &str) -> Result<bool, String> {
        let tenant = tenant_key(tenant_id)?;
        let slot = self.slot(&tenant);
        let mut live = slot.live.lock();
        match live.take() {
            Some(l) => {
                self.teardown(&tenant, l, "closed");
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Closes browsers idle for longer than the limit. A browser in use is
    /// skipped: it is not idle. Returns how many were closed.
    pub fn reap_idle(&self, now: Instant) -> usize {
        let slots: Vec<(String, Arc<Slot>)> = self
            .slots
            .lock()
            .iter()
            .map(|(tenant, slot)| (tenant.clone(), Arc::clone(slot)))
            .collect();
        let mut closed = 0;
        for (tenant, slot) in slots {
            let Some(mut live) = slot.live.try_lock() else {
                continue;
            };
            let idle = live
                .as_ref()
                .is_some_and(|l| now.saturating_duration_since(l.last_used) >= self.limits.idle);
            if idle {
                self.teardown(&tenant, live.take().expect("checked"), "idle_closed");
                closed += 1;
            }
        }
        self.prune();
        closed
    }

    /// Ends every browser (daemon shutdown).
    pub fn close_all(&self) {
        let slots: Vec<(String, Arc<Slot>)> = self
            .slots
            .lock()
            .iter()
            .map(|(tenant, slot)| (tenant.clone(), Arc::clone(slot)))
            .collect();
        for (tenant, slot) in slots {
            if let Some(l) = slot.live.lock().take() {
                self.teardown(&tenant, l, "closed");
            }
        }
    }

    /// Runs [`Self::reap_idle`] on its own thread until the pool is dropped.
    pub fn spawn_reaper(self: &Arc<Self>) -> std::io::Result<()> {
        let pool: Weak<Self> = Arc::downgrade(self);
        let every = (self.limits.idle / 4).clamp(Duration::from_secs(1), Duration::from_secs(30));
        std::thread::Builder::new()
            .name("kura-browser-reaper".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(every);
                    let Some(pool) = pool.upgrade() else {
                        return;
                    };
                    pool.reap_idle(Instant::now());
                }
            })
            .map(|_| ())
    }

    fn slot(&self, tenant: &str) -> Arc<Slot> {
        Arc::clone(self.slots.lock().entry(tenant.to_string()).or_default())
    }

    /// Drops slots nobody holds and nothing runs in, so the map does not
    /// grow with every tenant that ever browsed. A slot only the map holds
    /// cannot be in use: callers take their reference under the same lock.
    fn prune(&self) {
        self.slots.lock().retain(|_, slot| {
            Arc::strong_count(slot) > 1 || slot.live.try_lock().is_none_or(|l| l.is_some())
        });
    }

    fn start(&self, tenant: &str) -> Result<Live, String> {
        // Reserve a place first, so concurrent starts cannot overshoot.
        let reserved = self
            .active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.limits.max_sessions).then_some(n + 1)
            })
            .is_ok();
        if !reserved {
            record("refused_capacity");
            return Err(format!(
                "no browser is free right now ({} in use); try again in a few minutes",
                self.limits.max_sessions
            ));
        }
        let launched = match self.launcher.launch(tenant) {
            Ok(launched) => launched,
            Err(err) => {
                self.active.fetch_sub(1, Ordering::SeqCst);
                record("failed");
                eprintln!("[kura] browser: tenant={tenant} launch failed: {err}");
                return Err(format!("the browser could not start: {err}"));
            }
        };
        let now = Utc::now();
        let session = Session {
            computer_use_session_id: format!("cus_{}", uuid::Uuid::new_v4().simple()),
            run_id: "tenant-browser".to_string(),
            status: SessionStatus::Starting,
            driver_kind: "browser".to_string(),
            started_at: now,
            updated_at: now,
            ..Session::default()
        };
        let input = CreateSessionInput {
            driver_kind: "browser".to_string(),
            ..CreateSessionInput::default()
        };
        match launched.worker.start_session(session, input) {
            Ok(session) => {
                record("started");
                eprintln!(
                    "[kura] browser: tenant={tenant} started (execution {})",
                    launched.handle
                );
                Ok(Live {
                    handle: launched.handle,
                    worker: launched.worker,
                    session,
                    last_used: Instant::now(),
                })
            }
            Err(err) => {
                drop(launched.worker);
                self.launcher.stop(&launched.handle);
                self.active.fetch_sub(1, Ordering::SeqCst);
                record("failed");
                eprintln!("[kura] browser: tenant={tenant} start failed: {err}");
                Err(format!("the browser could not start: {err}"))
            }
        }
    }

    /// Runs one action, then reads the page.
    fn run(
        &self,
        tenant: &str,
        live: &mut Option<Live>,
        kind: ActionKind,
        input: &[(&str, &str)],
    ) -> Result<Page, String> {
        let l = live.as_mut().expect("caller checked");
        let (session, action, captures) = l
            .worker
            .execute_action(l.session.clone(), self.action(&l.session, kind, input))
            .inspect_err(|_| self.drop_if_dead(tenant, live))?;
        let l = live.as_mut().expect("still live");
        l.session = session.clone();
        l.last_used = Instant::now();
        if action.status != ActionStatus::Completed {
            return Err(first_non_empty(&action.failure_reason, "the action failed").to_string());
        }
        // Click and input already answer with the page.
        if let Some(page) = captures
            .iter()
            .find(|c| c.kind == ArtifactKind::PageSnapshot)
            .and_then(|c| page_from_snapshot(&c.content))
        {
            return Ok(page);
        }
        self.page(tenant, live, session)
    }

    /// Reads the page (a snapshot action).
    fn page(
        &self,
        tenant: &str,
        live: &mut Option<Live>,
        session: Session,
    ) -> Result<Page, String> {
        let l = live.as_mut().expect("caller checked");
        let (session, action, captures) = l
            .worker
            .execute_action(
                session.clone(),
                self.action(&session, ActionKind::Snapshot, &[]),
            )
            .inspect_err(|_| self.drop_if_dead(tenant, live))?;
        let l = live.as_mut().expect("still live");
        l.session = session;
        l.last_used = Instant::now();
        if action.status != ActionStatus::Completed {
            return Err(
                first_non_empty(&action.failure_reason, "the page could not be read").to_string(),
            );
        }
        captures
            .iter()
            .find(|c| c.kind == ArtifactKind::PageSnapshot)
            .and_then(|c| page_from_snapshot(&c.content))
            .ok_or_else(|| "the browser returned no page".to_string())
    }

    fn action(&self, session: &Session, kind: ActionKind, input: &[(&str, &str)]) -> Action {
        let now = Utc::now();
        Action {
            computer_use_action_id: format!(
                "cua_{}",
                self.next_action.fetch_add(1, Ordering::SeqCst)
            ),
            computer_use_session_id: session.computer_use_session_id.clone(),
            run_id: session.run_id.clone(),
            action_kind: kind,
            status: ActionStatus::Requested,
            requested_at: now,
            updated_at: now,
            input: input
                .iter()
                .map(|(k, v)| {
                    (
                        (*k).to_string(),
                        serde_json::Value::String((*v).to_string()),
                    )
                })
                .collect(),
            ..Action::default()
        }
    }

    /// After a failed exchange: a worker that stopped answering is ended now
    /// rather than at the next call.
    fn drop_if_dead(&self, tenant: &str, live: &mut Option<Live>) {
        if live.as_ref().is_some_and(|l| !l.worker.alive()) {
            self.teardown(tenant, live.take().expect("checked"), "failed");
        }
    }

    fn teardown(&self, tenant: &str, live: Live, outcome: &str) {
        if live.worker.alive() {
            // A polite close; the container is removed either way.
            let _ = live.worker.close_session(live.session.clone());
        }
        drop(live.worker);
        self.launcher.stop(&live.handle);
        self.active.fetch_sub(1, Ordering::SeqCst);
        record(outcome);
        eprintln!(
            "[kura] browser: tenant={tenant} {outcome} (execution {})",
            live.handle
        );
    }
}

/// Only web pages: `file:`, `data:`, `javascript:` and browser-internal
/// schemes would read the container or run script outside any site.
fn checked_url(raw: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw.trim()).map_err(|_| format!("{raw:?} is not a URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return Err("only http and https URLs can be opened".to_string());
    }
    Ok(parsed.to_string())
}

fn tenant_key(tenant_id: &str) -> Result<String, String> {
    let tenant = tenant_id.trim();
    if tenant.is_empty() {
        return Err("a browser needs a tenant".to_string());
    }
    Ok(tenant.to_string())
}

fn page_from_snapshot(content: &[u8]) -> Option<Page> {
    let value: serde_json::Value = serde_json::from_slice(content).ok()?;
    let field = |k: &str| {
        value
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Some(Page {
        url: field("url"),
        title: field("title"),
        text: field("text"),
    })
}

fn first_non_empty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn record(outcome: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::BROWSER_SESSIONS_TOTAL,
        &[("outcome", outcome)],
    );
}
