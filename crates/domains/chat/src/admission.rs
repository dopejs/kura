//! Turn admission: per-tenant concurrency, turn-rate and token limits in
//! front of every turn, and a queue that hands free slots to tenants in
//! turn (agent-runtime-program.md 4.1; design: `docs/harness/turn-admission.md`).
//!
//! A turn takes a [`TurnPermit`] before it does anything and holds it until
//! it ends; dropping the permit frees the slot, so every exit path releases
//! it. Concurrency over a cap queues (bounded depth, bounded wait); turn rate
//! and token budget over their limits are refused at once, because they
//! limit sustained use and waiting would only hold a thread.
//!
//! Callers are synchronous (the chat service is blocking), so a queued turn
//! waits on its own thread. The number of such threads is bounded by
//! `global_concurrency + global_queue_depth`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

/// Retry hint for a full queue or a wait that ran out: there is no exact
/// time at which a slot frees, and a client retrying in a tight loop is the
/// thing to avoid.
pub const QUEUE_RETRY_AFTER: Duration = Duration::from_secs(5);

/// Tenant entries kept before idle ones are swept. An idle entry whose
/// buckets have refilled is identical to a fresh one, so sweeping it loses
/// nothing; the bound only keeps a daemon that has seen many tenants from
/// holding all of them.
const SWEEP_ABOVE: usize = 1024;

/// The limits. `0` is unbounded for every count and rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnLimits {
    /// Turns running at once, daemon-wide.
    pub global_concurrency: usize,
    /// Turns running at once for one tenant.
    pub tenant_concurrency: usize,
    /// Turn starts per tenant per minute; the burst is the same number.
    pub turns_per_minute: u64,
    /// Provider-reported tokens per tenant per minute. Charged after each
    /// dispatch settles and checked when a turn is admitted, so a running
    /// turn can take the budget into debt by its own usage.
    pub tokens_per_minute: u64,
    /// Turns one tenant may have waiting for a slot.
    pub tenant_queue_depth: usize,
    /// Turns waiting for a slot, daemon-wide.
    pub global_queue_depth: usize,
    /// Longest wait for a slot.
    pub queue_timeout: Duration,
    /// Tenants the per-tenant limits do not apply to (the operator). Work
    /// with no tenant is always exempt. Exempt turns still count toward the
    /// global cap and wait in the same queue.
    pub exempt_tenants: Vec<String>,
}

/// Why a turn was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleReason {
    TurnRateLimited,
    TokenBudgetExhausted,
    QueueFull,
    QueueTimeout,
}

impl ThrottleReason {
    /// Stable code: the HTTP error code and the metric outcome.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::TurnRateLimited => "turn_rate_limited",
            Self::TokenBudgetExhausted => "token_budget_exhausted",
            Self::QueueFull => "turn_queue_full",
            Self::QueueTimeout => "turn_queue_timeout",
        }
    }
}

/// A refused turn: why, and when trying again can succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throttled {
    pub reason: ThrottleReason,
    pub retry_after: Duration,
}

impl std::fmt::Display for Throttled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.reason {
            ThrottleReason::TurnRateLimited => "too many turns started in the last minute",
            ThrottleReason::TokenBudgetExhausted => "the per-minute token budget is spent",
            ThrottleReason::QueueFull => "too many turns are already waiting",
            ThrottleReason::QueueTimeout => "no turn slot freed in time",
        };
        write!(
            f,
            "{what}; retry in {}s",
            self.retry_after.as_secs_f64().ceil() as u64
        )
    }
}

/// The gate. Shared by every clone of the chat service.
pub struct Admission {
    limits: TurnLimits,
    state: Mutex<State>,
    granted: Condvar,
}

#[derive(Default)]
struct State {
    running: usize,
    waiting: usize,
    next_ticket: u64,
    /// Tickets handed a slot whose waiter has not picked it up yet.
    granted: HashSet<u64>,
    /// Tenants with waiters, in the order they get the next slot.
    turn_order: VecDeque<String>,
    tenants: HashMap<String, Tenant>,
}

#[derive(Default)]
struct Tenant {
    running: usize,
    queue: VecDeque<u64>,
    turns: Option<Bucket>,
    tokens: Option<Bucket>,
}

/// A token bucket refilled continuously. `level` may go negative (token
/// debt); nothing is admitted until it is above zero again.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    capacity: f64,
    per_second: f64,
    level: f64,
    at: Instant,
}

impl Bucket {
    fn full(per_minute: u64, now: Instant) -> Self {
        let capacity = per_minute as f64;
        Self {
            capacity,
            per_second: capacity / 60.0,
            level: capacity,
            at: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.level = (self.level + elapsed * self.per_second).min(self.capacity);
        self.at = now;
    }

    /// Time until the level reaches `needed`.
    fn time_to(&self, needed: f64) -> Duration {
        let short = (needed - self.level).max(0.0);
        Duration::from_secs_f64((short / self.per_second).max(1.0))
    }

    fn is_full(&self) -> bool {
        self.level >= self.capacity
    }
}

/// A running turn's slot. Dropping it frees the slot for the next waiter.
#[must_use = "the slot is released as soon as the permit is dropped"]
pub struct TurnPermit {
    admission: Arc<Admission>,
    tenant: String,
}

impl std::fmt::Debug for TurnPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnPermit")
            .field("tenant", &self.tenant)
            .finish()
    }
}

impl Drop for TurnPermit {
    fn drop(&mut self) {
        self.admission.release(&self.tenant);
    }
}

/// What [`crate::Service::admit`] hands back: the turn's slot, or nothing
/// when the service has no gate. Hold it for the whole turn.
#[derive(Debug)]
#[must_use = "the slot is released as soon as this is dropped"]
pub struct Admitted {
    /// Held for its `Drop`; never read.
    pub(crate) _permit: Option<TurnPermit>,
}

impl Admission {
    #[must_use]
    pub fn new(limits: TurnLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::new(State::default()),
            granted: Condvar::new(),
        })
    }

    #[must_use]
    pub fn limits(&self) -> &TurnLimits {
        &self.limits
    }

    /// Admits a turn for `tenant_id`, waiting for a slot when the caps are
    /// reached. Blocking.
    pub fn admit(self: &Arc<Self>, tenant_id: &str) -> Result<TurnPermit, Throttled> {
        let tenant = tenant_id.trim().to_string();
        let exempt = self.is_exempt(&tenant);
        let now = Instant::now();
        let mut state = self.state.lock();
        if state.tenants.len() > SWEEP_ABOVE {
            self.sweep(&mut state, now);
        }

        if !exempt {
            let entry = state.tenants.entry(tenant.clone()).or_default();
            if let Some(denied) = self.check_buckets(entry, now) {
                drop(state);
                return Err(self.deny(&tenant, denied));
            }
        }

        // Join the queue and let the scheduler decide: a turn that can run
        // now is granted at once, and one that cannot does not jump ahead of
        // tenants already waiting.
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting += 1;
        let entry = state.tenants.entry(tenant.clone()).or_default();
        entry.queue.push_back(ticket);
        if entry.queue.len() == 1 {
            state.turn_order.push_back(tenant.clone());
        }
        self.schedule(&mut state);
        if state.granted.remove(&ticket) {
            self.take_turn_token(&mut state, &tenant, exempt);
            drop(state);
            record_outcome("admitted");
            return Ok(self.permit(tenant));
        }

        let entry = &state.tenants[&tenant];
        let tenant_full = !exempt
            && self.limits.tenant_queue_depth > 0
            && entry.queue.len() > self.limits.tenant_queue_depth;
        let global_full =
            self.limits.global_queue_depth > 0 && state.waiting > self.limits.global_queue_depth;
        if tenant_full || global_full {
            self.withdraw(&mut state, &tenant, ticket);
            drop(state);
            return Err(self.deny(
                &tenant,
                Throttled {
                    reason: ThrottleReason::QueueFull,
                    retry_after: QUEUE_RETRY_AFTER,
                },
            ));
        }
        // Queued: the turn counts against the rate from here, so a tenant
        // cannot fill the queue faster than its rate allows.
        self.take_turn_token(&mut state, &tenant, exempt);

        let deadline = now + self.limits.queue_timeout;
        loop {
            if state.granted.remove(&ticket) {
                drop(state);
                kura_telemetry::metrics::registry().observe(
                    kura_telemetry::metrics::CHAT_TURN_QUEUE_WAIT_SECONDS,
                    &[],
                    now.elapsed().as_secs_f64(),
                );
                record_outcome("queued");
                return Ok(self.permit(tenant));
            }
            if self.granted.wait_until(&mut state, deadline).timed_out() {
                // A grant that raced the deadline wins.
                if state.granted.remove(&ticket) {
                    drop(state);
                    record_outcome("queued");
                    return Ok(self.permit(tenant));
                }
                self.withdraw(&mut state, &tenant, ticket);
                drop(state);
                return Err(self.deny(
                    &tenant,
                    Throttled {
                        reason: ThrottleReason::QueueTimeout,
                        retry_after: QUEUE_RETRY_AFTER,
                    },
                ));
            }
        }
    }

    /// Charges a settled dispatch's tokens to the tenant's budget. A no-op
    /// without a token limit or for exempt tenants.
    pub fn charge_tokens(&self, tenant_id: &str, tokens: i64) {
        if self.limits.tokens_per_minute == 0 || tokens <= 0 {
            return;
        }
        let tenant = tenant_id.trim();
        if self.is_exempt(tenant) {
            return;
        }
        let now = Instant::now();
        let mut state = self.state.lock();
        let entry = state.tenants.entry(tenant.to_string()).or_default();
        let bucket = entry
            .tokens
            .get_or_insert_with(|| Bucket::full(self.limits.tokens_per_minute, now));
        bucket.refill(now);
        bucket.level -= tokens as f64;
    }

    fn is_exempt(&self, tenant: &str) -> bool {
        tenant.is_empty() || self.limits.exempt_tenants.iter().any(|t| t == tenant)
    }

    fn permit(self: &Arc<Self>, tenant: String) -> TurnPermit {
        TurnPermit {
            admission: Arc::clone(self),
            tenant,
        }
    }

    /// Rate and token checks, without consuming anything.
    fn check_buckets(&self, entry: &mut Tenant, now: Instant) -> Option<Throttled> {
        if self.limits.tokens_per_minute > 0 {
            if let Some(bucket) = entry.tokens.as_mut() {
                bucket.refill(now);
                if bucket.level <= 0.0 {
                    return Some(Throttled {
                        reason: ThrottleReason::TokenBudgetExhausted,
                        retry_after: bucket.time_to(1.0),
                    });
                }
            }
        }
        if self.limits.turns_per_minute > 0 {
            let bucket = entry
                .turns
                .get_or_insert_with(|| Bucket::full(self.limits.turns_per_minute, now));
            bucket.refill(now);
            if bucket.level < 1.0 {
                return Some(Throttled {
                    reason: ThrottleReason::TurnRateLimited,
                    retry_after: bucket.time_to(1.0),
                });
            }
        }
        None
    }

    fn take_turn_token(&self, state: &mut State, tenant: &str, exempt: bool) {
        if exempt || self.limits.turns_per_minute == 0 {
            return;
        }
        if let Some(bucket) = state
            .tenants
            .get_mut(tenant)
            .and_then(|entry| entry.turns.as_mut())
        {
            bucket.level -= 1.0;
        }
    }

    /// Hands free slots to waiters: tenants in turn, each tenant's turns in
    /// arrival order, skipping a tenant at its own cap.
    fn schedule(&self, state: &mut State) {
        let mut granted_any = false;
        let mut passes_without_grant = 0;
        while self.has_global_room(state) && passes_without_grant < state.turn_order.len() {
            let Some(tenant) = state.turn_order.pop_front() else {
                break;
            };
            let exempt = self.is_exempt(&tenant);
            let cap = self.limits.tenant_concurrency;
            let Some(entry) = state.tenants.get_mut(&tenant) else {
                continue;
            };
            if entry.queue.is_empty() {
                // Withdrawn while it waited for its turn; drop it from the order.
                continue;
            }
            if !exempt && cap > 0 && entry.running >= cap {
                state.turn_order.push_back(tenant);
                passes_without_grant += 1;
                continue;
            }
            let ticket = entry.queue.pop_front().expect("checked non-empty");
            entry.running += 1;
            let more = !entry.queue.is_empty();
            state.running += 1;
            state.waiting -= 1;
            state.granted.insert(ticket);
            if more {
                state.turn_order.push_back(tenant);
            }
            granted_any = true;
            passes_without_grant = 0;
        }
        if granted_any {
            self.granted.notify_all();
        }
    }

    fn has_global_room(&self, state: &State) -> bool {
        self.limits.global_concurrency == 0 || state.running < self.limits.global_concurrency
    }

    /// Takes a waiting ticket out of the queue (timed out, or over depth).
    fn withdraw(&self, state: &mut State, tenant: &str, ticket: u64) {
        if let Some(entry) = state.tenants.get_mut(tenant) {
            if let Some(at) = entry.queue.iter().position(|t| *t == ticket) {
                entry.queue.remove(at);
                state.waiting -= 1;
                // Out of the order too: queueing again re-adds it, and two
                // entries would give it two turns per rotation.
                if entry.queue.is_empty() {
                    state.turn_order.retain(|t| t != tenant);
                }
            }
        }
    }

    fn release(&self, tenant: &str) {
        let mut state = self.state.lock();
        state.running = state.running.saturating_sub(1);
        if let Some(entry) = state.tenants.get_mut(tenant) {
            entry.running = entry.running.saturating_sub(1);
        }
        self.schedule(&mut state);
    }

    /// Drops idle tenants whose buckets are full: identical to a fresh entry.
    fn sweep(&self, state: &mut State, now: Instant) {
        state.tenants.retain(|_, entry| {
            if entry.running > 0 || !entry.queue.is_empty() {
                return true;
            }
            let refilled = |bucket: &mut Option<Bucket>| {
                bucket.as_mut().is_none_or(|b| {
                    b.refill(now);
                    b.is_full()
                })
            };
            !(refilled(&mut entry.turns) && refilled(&mut entry.tokens))
        });
    }

    fn deny(&self, tenant: &str, throttled: Throttled) -> Throttled {
        record_outcome(throttled.reason.code());
        eprintln!(
            "[kura] turn admission: tenant={} reason={} retry_after={}s",
            if tenant.is_empty() { "-" } else { tenant },
            throttled.reason.code(),
            throttled.retry_after.as_secs_f64().ceil() as u64
        );
        throttled
    }
}

fn record_outcome(outcome: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::CHAT_TURN_ADMISSIONS_TOTAL,
        &[("outcome", outcome)],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    fn limits() -> TurnLimits {
        TurnLimits {
            global_concurrency: 0,
            tenant_concurrency: 0,
            turns_per_minute: 0,
            tokens_per_minute: 0,
            tenant_queue_depth: 0,
            global_queue_depth: 0,
            queue_timeout: Duration::from_secs(5),
            exempt_tenants: vec!["ten_operator".to_string()],
        }
    }

    fn running(admission: &Admission) -> usize {
        admission.state.lock().running
    }

    fn waiting(admission: &Admission) -> usize {
        admission.state.lock().waiting
    }

    /// Starts a thread that admits a turn for `tenant` and reports the
    /// order it was admitted in; the permit is held until `hold` drops.
    fn waiter(
        admission: &Arc<Admission>,
        tenant: &'static str,
        order: &mpsc::Sender<&'static str>,
        hold: &Arc<Mutex<()>>,
    ) -> thread::JoinHandle<()> {
        let admission = Arc::clone(admission);
        let order = order.clone();
        let hold = Arc::clone(hold);
        thread::spawn(move || {
            let permit = admission.admit(tenant).expect("admitted");
            order.send(tenant).unwrap();
            let _held = hold.lock();
            drop(permit);
        })
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "condition never held");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn unbounded_limits_admit_everything_at_once() {
        let admission = Admission::new(limits());
        let permits: Vec<_> = (0..100)
            .map(|_| admission.admit("ten_a").unwrap())
            .collect();
        assert_eq!(running(&admission), 100);
        drop(permits);
        assert_eq!(running(&admission), 0);
    }

    #[test]
    fn a_tenant_over_its_cap_waits_and_runs_when_a_slot_frees() {
        let admission = Admission::new(TurnLimits {
            tenant_concurrency: 1,
            ..limits()
        });
        let first = admission.admit("ten_a").unwrap();
        let second = thread::spawn({
            let admission = Arc::clone(&admission);
            move || admission.admit("ten_a").map(|_| ())
        });
        wait_for(|| waiting(&admission) == 1);
        // Another tenant is not held up by ten_a's cap.
        let other = admission.admit("ten_b").unwrap();
        drop(first);
        second.join().unwrap().expect("ran after the slot freed");
        drop(other);
        assert_eq!(running(&admission), 0);
        assert_eq!(waiting(&admission), 0);
    }

    #[test]
    fn freed_slots_go_to_tenants_in_turn_not_in_arrival_order() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 1,
            ..limits()
        });
        let hold = Arc::new(Mutex::new(()));
        let gate = hold.lock();
        let first = admission.admit("ten_a").unwrap();
        let (order_tx, order_rx) = mpsc::channel();
        // ten_a queues three turns before ten_b queues one.
        let mut threads = Vec::new();
        for queued in 1..=3 {
            threads.push(waiter(&admission, "ten_a", &order_tx, &hold));
            wait_for(|| waiting(&admission) == queued);
        }
        threads.push(waiter(&admission, "ten_b", &order_tx, &hold));
        wait_for(|| waiting(&admission) == 4);
        drop(first);
        drop(gate);
        let order: Vec<_> = (0..4).map(|_| order_rx.recv().unwrap()).collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(order, ["ten_a", "ten_b", "ten_a", "ten_a"]);
    }

    #[test]
    fn a_freed_slot_is_handed_to_the_waiter_not_to_a_newcomer() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 1,
            queue_timeout: Duration::from_millis(50),
            ..limits()
        });
        let first = admission.admit("ten_a").unwrap();
        let queued = thread::spawn({
            let admission = Arc::clone(&admission);
            move || admission.admit("ten_b")
        });
        wait_for(|| waiting(&admission) == 1);
        // The slot is ten_b's from the moment it frees, whether or not its
        // thread has woken yet.
        drop(first);
        let newcomer = admission.admit("ten_c").unwrap_err();
        assert_eq!(newcomer.reason, ThrottleReason::QueueTimeout);
        let permit = queued.join().unwrap().expect("the waiter got the slot");
        drop(permit);
        assert_eq!(running(&admission), 0);
    }

    #[test]
    fn a_full_tenant_queue_is_refused_and_takes_no_rate_token() {
        let admission = Admission::new(TurnLimits {
            tenant_concurrency: 1,
            tenant_queue_depth: 1,
            turns_per_minute: 3,
            ..limits()
        });
        let holder = admission.admit("ten_a").unwrap();
        let queued = thread::spawn({
            let admission = Arc::clone(&admission);
            move || admission.admit("ten_a").map(|_| ())
        });
        wait_for(|| waiting(&admission) == 1);
        let refused = admission.admit("ten_a").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::QueueFull);
        assert_eq!(refused.retry_after, QUEUE_RETRY_AFTER);
        // Two tokens spent (running + queued), the refused one returned:
        // one turn is still allowed this minute.
        let level = admission.state.lock().tenants["ten_a"].turns.unwrap().level;
        assert!((level - 1.0).abs() < 0.01, "level {level}");
        drop(holder);
        queued.join().unwrap().unwrap();
    }

    #[test]
    fn the_global_queue_bound_applies_across_tenants() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 1,
            global_queue_depth: 1,
            ..limits()
        });
        let holder = admission.admit("ten_a").unwrap();
        let queued = thread::spawn({
            let admission = Arc::clone(&admission);
            move || admission.admit("ten_b").map(|_| ())
        });
        wait_for(|| waiting(&admission) == 1);
        let refused = admission.admit("ten_c").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::QueueFull);
        drop(holder);
        queued.join().unwrap().unwrap();
    }

    #[test]
    fn a_wait_that_runs_out_is_refused_and_leaves_no_trace() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 1,
            queue_timeout: Duration::from_millis(30),
            ..limits()
        });
        let holder = admission.admit("ten_a").unwrap();
        let refused = admission.admit("ten_b").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::QueueTimeout);
        assert_eq!(waiting(&admission), 0);
        drop(holder);
        // The withdrawn ticket is not handed the freed slot.
        assert_eq!(running(&admission), 0);
        let _again = admission.admit("ten_b").unwrap();
    }

    #[test]
    fn a_tenant_that_left_the_queue_comes_back_once_in_the_order() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 1,
            queue_timeout: Duration::from_millis(200),
            ..limits()
        });
        let holder = admission.admit("ten_x").unwrap();
        // ten_a's wait runs out, then it queues again.
        admission.admit("ten_a").unwrap_err();
        let again = thread::spawn({
            let admission = Arc::clone(&admission);
            move || admission.admit("ten_a").map(|_| ())
        });
        wait_for(|| waiting(&admission) == 1);
        assert_eq!(
            admission.state.lock().turn_order,
            ["ten_a"],
            "one entry, not two"
        );
        drop(holder);
        again.join().unwrap().unwrap();
    }

    #[test]
    fn the_turn_rate_refuses_past_the_burst_with_the_time_to_the_next_turn() {
        let admission = Admission::new(TurnLimits {
            turns_per_minute: 2,
            ..limits()
        });
        let _a = admission.admit("ten_a").unwrap();
        let _b = admission.admit("ten_a").unwrap();
        let refused = admission.admit("ten_a").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::TurnRateLimited);
        // 2 a minute: one turn every 30 s.
        let secs = refused.retry_after.as_secs_f64();
        assert!((29.0..=30.0).contains(&secs), "retry after {secs}");
        // Rates are per tenant.
        let _c = admission.admit("ten_b").unwrap();
    }

    #[test]
    fn the_rate_bucket_refills_with_time() {
        let now = Instant::now();
        let mut bucket = Bucket::full(60, now);
        bucket.level = 0.0;
        bucket.refill(now + Duration::from_secs(2));
        assert!((bucket.level - 2.0).abs() < 1e-9);
        bucket.refill(now + Duration::from_secs(600));
        assert!(bucket.is_full());
    }

    #[test]
    fn token_debt_refuses_new_turns_until_it_clears() {
        let admission = Admission::new(TurnLimits {
            tokens_per_minute: 600,
            ..limits()
        });
        let permit = admission.admit("ten_a").unwrap();
        // The running turn spends past the budget; it is not interrupted.
        admission.charge_tokens("ten_a", 900);
        drop(permit);
        let refused = admission.admit("ten_a").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::TokenBudgetExhausted);
        // 300 tokens of debt at 10 a second.
        let secs = refused.retry_after.as_secs_f64();
        assert!((29.0..=31.0).contains(&secs), "retry after {secs}");
        let _other = admission.admit("ten_b").unwrap();
    }

    #[test]
    fn exempt_work_skips_tenant_limits_but_not_the_global_cap() {
        let admission = Admission::new(TurnLimits {
            global_concurrency: 2,
            tenant_concurrency: 1,
            turns_per_minute: 1,
            tokens_per_minute: 1,
            queue_timeout: Duration::from_millis(30),
            ..limits()
        });
        admission.charge_tokens("ten_operator", 1_000_000);
        let a = admission.admit("ten_operator").unwrap();
        let b = admission.admit("").unwrap();
        let refused = admission.admit("ten_operator").unwrap_err();
        assert_eq!(refused.reason, ThrottleReason::QueueTimeout);
        drop((a, b));
    }

    #[test]
    fn idle_tenants_with_full_buckets_are_swept() {
        let admission = Admission::new(TurnLimits {
            turns_per_minute: 60,
            ..limits()
        });
        let now = Instant::now();
        {
            let mut state = admission.state.lock();
            for i in 0..10 {
                let mut entry = Tenant::default();
                let mut bucket = Bucket::full(60, now);
                if i % 2 == 0 {
                    bucket.level = 0.0;
                }
                entry.turns = Some(bucket);
                state.tenants.insert(format!("ten_{i}"), entry);
            }
            admission.sweep(&mut state, now);
            assert_eq!(state.tenants.len(), 5, "only spent buckets are kept");
        }
    }
}
