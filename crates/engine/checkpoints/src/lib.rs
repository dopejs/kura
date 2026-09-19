//! Port of `daemon/internal/checkpoints`: durable run recovery. The manager snapshots a run
//! from the in-memory runtime ledger, persists it through the SQLite store, and restores the
//! latest checkpoint for each run on startup.

use std::sync::Arc;

use kura_runtime::{Manager as RuntimeManager, RunCheckpoint};
use kura_store::SQLiteStore;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryStats {
    pub run_count: usize,
}

pub struct Manager {
    store: Arc<parking_lot::Mutex<SQLiteStore>>,
    runtime: Arc<RuntimeManager>,
}

impl Manager {
    #[must_use]
    pub fn new(store: Arc<parking_lot::Mutex<SQLiteStore>>, runtime: Arc<RuntimeManager>) -> Self {
        Manager { store, runtime }
    }

    pub fn save_run_checkpoint(&self, run_id: &str) -> Result<(), String> {
        let checkpoint = self
            .runtime
            .snapshot_run(run_id)
            .map_err(|e| format!("snapshot run {run_id}: {e}"))?;
        self.persist_snapshot_state(&checkpoint)?;
        self.store
            .lock()
            .save_checkpoint(&checkpoint)
            .map_err(|e| format!("save checkpoint for run {run_id}: {e}"))
    }

    fn persist_snapshot_state(&self, checkpoint: &RunCheckpoint) -> Result<(), String> {
        self.store
            .lock()
            .upsert_run(&checkpoint.run)
            .map_err(|e| format!("upsert checkpoint run {}: {e}", checkpoint.run.run_id))?;
        for step in &checkpoint.steps {
            self.store
                .lock()
                .upsert_step(step)
                .map_err(|e| format!("upsert checkpoint step {}: {e}", step.step_id))?;
        }
        for tool_call in &checkpoint.tool_calls {
            self.store.lock().upsert_tool_call(tool_call).map_err(|e| {
                format!(
                    "upsert checkpoint tool call {}: {e}",
                    tool_call.tool_call_id
                )
            })?;
        }
        Ok(())
    }

    pub fn restore_run_checkpoint(&self, checkpoint: RunCheckpoint) -> Result<(), String> {
        self.runtime.restore_run_checkpoint(checkpoint.clone());
        self.persist_snapshot_state(&checkpoint).map_err(|e| {
            format!(
                "persist restored checkpoint state for run {}: {e}",
                checkpoint.run.run_id
            )
        })?;
        self.store.lock().save_checkpoint(&checkpoint).map_err(|e| {
            format!(
                "save restored checkpoint for run {}: {e}",
                checkpoint.run.run_id
            )
        })
    }

    pub fn restore(&self) -> Result<RecoveryStats, String> {
        let checkpoints = self
            .store
            .lock()
            .list_latest_checkpoints()
            .map_err(|e| format!("load latest checkpoints: {e}"))?;
        self.runtime.restore_checkpoints(checkpoints.clone());
        Ok(RecoveryStats {
            run_count: checkpoints.len(),
        })
    }

    #[must_use]
    pub fn runtime(&self) -> &Arc<RuntimeManager> {
        &self.runtime
    }
}
