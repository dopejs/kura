//! Resuming a turn from where an earlier attempt at it stood
//! (agent-runtime-program.md 2.7).
//!
//! A turn that runs tools leaves a checkpoint behind it as it goes (see
//! `kura_core::TurnCheckpoint`). The turn is identified by who asked, the
//! request id, the thread and the question: a client that retries a request
//! after the daemon restarted -- or after a provider failure -- sends the same
//! `X-Request-Id` and gets the same turn back, with its settled calls replayed
//! rather than run again. A request id the server generated is never sent
//! twice, so a turn nobody retries is never resumed.
//!
//! Only a turn that is not running can be resumed. A retry that arrives while
//! the first attempt is still running in this process (a client that timed
//! out and tried again) runs on its own, without a checkpoint: resuming the
//! first attempt's record beside the first attempt would run the same calls
//! twice at once.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use kura_core::{ResumeStats, TurnCheckpoint, TurnJournal};
use kura_store::TurnCheckpointRecord;
use sha2::{Digest, Sha256};

use crate::store::ChatStore;

/// How long a checkpoint waits for the retry that would resume it.
pub(crate) const CHECKPOINT_TTL_HOURS: i64 = 24;

/// What the checkpoint counter is labelled with.
const RESUMED: &str = "resumed";
const BUSY: &str = "busy";
const UNREADABLE: &str = "unreadable";
const WRITE_FAILED: &str = "write_failed";

/// Turn keys running in this process.
#[derive(Debug, Default)]
pub(crate) struct RunningTurns(parking_lot::Mutex<HashSet<String>>);

/// A turn this process is running, held for the whole of the turn; dropping
/// it lets the next attempt at the turn resume.
pub(crate) struct TurnResume {
    running: Arc<RunningTurns>,
    key: String,
    store: Arc<dyn ChatStore>,
    /// Where the earlier attempt stood; empty for a fresh turn.
    pub(crate) checkpoint: TurnCheckpoint,
    pub(crate) journal: Arc<StoreJournal>,
}

impl Drop for TurnResume {
    fn drop(&mut self) {
        self.running.0.lock().remove(&self.key);
    }
}

impl TurnResume {
    /// Claims the turn and loads what an earlier attempt left. `None` when
    /// there is no store, no request id to recognise a retry by, or the same
    /// turn is already running here.
    pub(crate) fn claim(
        running: &Arc<RunningTurns>,
        store: Option<&Arc<dyn ChatStore>>,
        tenant_id: &str,
        request_id: &str,
        thread_id: &str,
        question: &str,
    ) -> Option<Self> {
        let store = store?;
        let request_id = request_id.trim();
        if request_id.is_empty() {
            return None;
        }
        let key = turn_key(tenant_id, request_id, thread_id, question);
        if !running.0.lock().insert(key.clone()) {
            count(BUSY);
            return None;
        }
        let now = Utc::now();
        let mut resume = Self {
            running: Arc::clone(running),
            key: key.clone(),
            store: Arc::clone(store),
            checkpoint: TurnCheckpoint::default(),
            journal: Arc::new(StoreJournal {
                store: Arc::clone(store),
                template: TurnCheckpointRecord {
                    turn_key: key.clone(),
                    tenant_id: tenant_id.trim().to_string(),
                    thread_id: thread_id.trim().to_string(),
                    request_id: request_id.to_string(),
                    checkpoint_json: String::new(),
                    created_at: now,
                    updated_at: now,
                },
                written: AtomicBool::new(false),
                failed: AtomicBool::new(false),
            }),
        };
        match store.get_turn_checkpoint(&key) {
            Ok(Some(record)) if record.tenant_id == tenant_id.trim() => {
                match serde_json::from_str::<TurnCheckpoint>(&record.checkpoint_json) {
                    Ok(checkpoint)
                        if !checkpoint.rounds.is_empty() || !checkpoint.steered.is_empty() =>
                    {
                        count(RESUMED);
                        resume.checkpoint = checkpoint;
                    }
                    Ok(_) => {}
                    // Not something this build can read back. Starting over
                    // is what every turn did before checkpoints existed.
                    Err(_) => {
                        count(UNREADABLE);
                        let _ = store.delete_turn_checkpoint(&key);
                    }
                }
            }
            // A store that cannot be read is not a reason to refuse the
            // turn; it is run fresh.
            Ok(_) | Err(_) => {}
        }
        Some(resume)
    }

    /// The turn was answered: nothing is left to resume.
    pub(crate) fn finish(&self) {
        self.journal.failed.store(true, Ordering::SeqCst);
        let _ = self.store.delete_turn_checkpoint(&self.key);
    }
}

/// Writes every checkpoint the loop reports to the store.
pub(crate) struct StoreJournal {
    store: Arc<dyn ChatStore>,
    template: TurnCheckpointRecord,
    /// The first write prunes checkpoints nobody came back for.
    written: AtomicBool,
    /// Once one write has failed, none are attempted: a later write
    /// succeeding would leave a record that skipped a state, and a call
    /// recorded as never started that did start would run twice.
    failed: AtomicBool,
}

impl TurnJournal for StoreJournal {
    fn record(&self, checkpoint: &TurnCheckpoint) {
        if self.failed.load(Ordering::SeqCst) {
            return;
        }
        let now = Utc::now();
        let written = serde_json::to_string(checkpoint)
            .map_err(|error| error.to_string())
            .and_then(|checkpoint_json| {
                self.store.put_turn_checkpoint(&TurnCheckpointRecord {
                    checkpoint_json,
                    updated_at: now,
                    ..self.template.clone()
                })
            });
        if written.is_err() {
            self.failed.store(true, Ordering::SeqCst);
            count(WRITE_FAILED);
            // What is stored is now behind the turn; resuming from it could
            // run a call again. Better none than a stale one.
            let _ = self.store.delete_turn_checkpoint(&self.template.turn_key);
            return;
        }
        if !self.written.swap(true, Ordering::SeqCst) {
            let cutoff = now - chrono::Duration::hours(CHECKPOINT_TTL_HOURS);
            if let Ok(pruned) = self.store.prune_turn_checkpoints(&cutoff) {
                if pruned > 0 {
                    kura_telemetry::metrics::registry().add(
                        kura_telemetry::metrics::CHAT_TURN_CHECKPOINTS_TOTAL,
                        &[("event", "pruned")],
                        pruned as u64,
                    );
                }
            }
        }
    }
}

/// Counts what resuming found, by kind of call.
pub(crate) fn record_resumed(stats: ResumeStats) {
    let metrics = kura_telemetry::metrics::registry();
    for (kind, calls) in [
        ("replayed", stats.replayed_calls),
        ("interrupted", stats.interrupted_calls),
        ("rerun", stats.rerun_calls),
    ] {
        if calls > 0 {
            metrics.add(
                kura_telemetry::metrics::CHAT_TURN_RESUMED_CALLS_TOTAL,
                &[("kind", kind)],
                calls as u64,
            );
        }
    }
}

fn count(event: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::CHAT_TURN_CHECKPOINTS_TOTAL,
        &[("event", event)],
    );
}

/// Everything that makes two attempts the same turn, hashed so the key has
/// a fixed size and does not carry the question.
fn turn_key(tenant_id: &str, request_id: &str, thread_id: &str, question: &str) -> String {
    let mut hash = Sha256::new();
    for part in [tenant_id, request_id, thread_id, question] {
        hash.update(part.trim().as_bytes());
        hash.update([0]);
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_core::{CallState, CheckpointCall, CheckpointRound};

    fn sqlite() -> (tempfile::TempDir, Arc<dyn ChatStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = kura_store::SQLiteStore::new(dir.path().to_str().unwrap()).unwrap();
        (dir, Arc::new(std::sync::Mutex::new(store)))
    }

    fn claim(running: &Arc<RunningTurns>, store: &Arc<dyn ChatStore>) -> Option<TurnResume> {
        TurnResume::claim(running, Some(store), "tenant_a", "req_1", "thread_1", "q")
    }

    fn one_round() -> TurnCheckpoint {
        TurnCheckpoint {
            rounds: vec![CheckpointRound {
                calls: vec![CheckpointCall {
                    call_id: "c1".into(),
                    name: "send_mail".into(),
                    arguments: "{}".into(),
                    state: CallState::Started,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_turn_running_here_is_not_claimed_twice() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        let first = claim(&running, &store).expect("first attempt");
        first.journal.record(&one_round());
        assert!(claim(&running, &store).is_none(), "still running");
        drop(first);

        let retry = claim(&running, &store).expect("after the first ended");
        assert_eq!(retry.checkpoint, one_round());
    }

    /// A message sent to the turn before it ran any tool is all the record
    /// holds; it is still resumed, or the message would be lost.
    #[test]
    fn a_checkpoint_holding_only_a_steered_message_is_resumed() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        let steered = TurnCheckpoint {
            steered: vec!["in French, please".to_string()],
            ..TurnCheckpoint::default()
        };
        claim(&running, &store).unwrap().journal.record(&steered);
        assert_eq!(claim(&running, &store).unwrap().checkpoint, steered);
    }

    #[test]
    fn another_tenant_with_the_same_request_id_resumes_nothing() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        claim(&running, &store)
            .unwrap()
            .journal
            .record(&one_round());

        let other = TurnResume::claim(&running, Some(&store), "tenant_b", "req_1", "thread_1", "q")
            .unwrap();
        assert!(other.checkpoint.rounds.is_empty());
    }

    #[test]
    fn a_finished_turn_leaves_nothing_and_writes_nothing_after() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        let turn = claim(&running, &store).unwrap();
        turn.journal.record(&one_round());
        turn.finish();
        turn.journal.record(&one_round());
        drop(turn);
        assert!(
            claim(&running, &store)
                .unwrap()
                .checkpoint
                .rounds
                .is_empty()
        );
    }

    #[test]
    fn an_unreadable_checkpoint_is_discarded_and_the_turn_starts_over() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        let key = turn_key("tenant_a", "req_1", "thread_1", "q");
        let now = Utc::now();
        store
            .put_turn_checkpoint(&TurnCheckpointRecord {
                turn_key: key.clone(),
                tenant_id: "tenant_a".into(),
                thread_id: "thread_1".into(),
                request_id: "req_1".into(),
                checkpoint_json: "{\"rounds\": 3}".into(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        let turn = claim(&running, &store).unwrap();
        assert!(turn.checkpoint.rounds.is_empty());
        assert!(store.get_turn_checkpoint(&key).unwrap().is_none());
    }

    #[test]
    fn no_request_id_means_no_claim() {
        let (_dir, store) = sqlite();
        let running = Arc::new(RunningTurns::default());
        assert!(
            TurnResume::claim(&running, Some(&store), "tenant_a", " ", "thread_1", "q").is_none()
        );
        assert!(TurnResume::claim(&running, None, "tenant_a", "req_1", "thread_1", "q").is_none());
    }

    #[test]
    fn the_key_separates_every_part() {
        let key = turn_key("t", "r", "th", "q");
        assert_eq!(key.len(), 64);
        assert_eq!(key, turn_key(" t", "r ", "th", "q"));
        assert_ne!(key, turn_key("t2", "r", "th", "q"));
        assert_ne!(key, turn_key("t", "r2", "th", "q"));
        assert_ne!(key, turn_key("t", "r", "th2", "q"));
        assert_ne!(key, turn_key("t", "r", "th", "q2"));
        // A boundary moved between fields is a different turn.
        assert_ne!(turn_key("ab", "c", "", ""), turn_key("a", "bc", "", ""));
    }
}
