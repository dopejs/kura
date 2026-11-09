//! Agent turn checkpoints (agent-runtime-program.md 2.7).
//!
//! Where an unfinished turn stands -- the rounds of tool calls it asked for
//! and what each call has done -- so a retry of the same request resumes
//! from its last settled call instead of running every call again. The
//! checkpoint itself is `kura-core`'s `TurnCheckpoint`, stored as JSON; this
//! table only knows whose it is and when it last changed.
//!
//! Rows are short-lived: written while a turn runs tools, deleted when the
//! turn is answered, and pruned by age when nothing came back for them.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, parse_rfc3339};

#[derive(Debug, Clone, PartialEq)]
pub struct TurnCheckpointRecord {
    /// Derived by the caller from everything that makes two attempts the
    /// same turn; opaque here.
    pub turn_key: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub request_id: String,
    pub checkpoint_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SQLiteStore {
    /// Writes a turn's checkpoint, keeping the row's first `created_at`.
    pub fn put_turn_checkpoint(&self, record: &TurnCheckpointRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO agent_turn_checkpoints
                    (turn_key, tenant_id, thread_id, request_id, checkpoint_json, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(turn_key) DO UPDATE SET
                    checkpoint_json = excluded.checkpoint_json,
                    updated_at = excluded.updated_at"#,
                params![
                    record.turn_key,
                    record.tenant_id,
                    record.thread_id,
                    record.request_id,
                    record.checkpoint_json,
                    now_rfc3339(&record.created_at),
                    now_rfc3339(&record.updated_at),
                ],
            )
            .map_err(|e| format!("put turn checkpoint {}: {e}", record.turn_key))?;
        Ok(())
    }

    /// The checkpoint for `turn_key`, if that turn left one.
    pub fn get_turn_checkpoint(
        &self,
        turn_key: &str,
    ) -> Result<Option<TurnCheckpointRecord>, String> {
        let row = self
            .conn
            .query_row(
                r#"SELECT turn_key, tenant_id, thread_id, request_id, checkpoint_json, created_at, updated_at
                FROM agent_turn_checkpoints WHERE turn_key = ?1"#,
                params![turn_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("get turn checkpoint {turn_key}: {e}"))?;
        let Some((turn_key, tenant_id, thread_id, request_id, checkpoint_json, created, updated)) =
            row
        else {
            return Ok(None);
        };
        Ok(Some(TurnCheckpointRecord {
            turn_key,
            tenant_id,
            thread_id,
            request_id,
            checkpoint_json,
            created_at: parse_rfc3339(&created)?,
            updated_at: parse_rfc3339(&updated)?,
        }))
    }

    /// The checkpoints a request left on a thread, most recently written
    /// first, for a caller that knows the request but not the turn key: the
    /// work-item boot scan, which reruns a round under its own request id
    /// and must know whether the rerun resumes (work-items.md §6).
    pub fn turn_checkpoints_for_request(
        &self,
        tenant_id: &str,
        thread_id: &str,
        request_id: &str,
    ) -> Result<Vec<TurnCheckpointRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT turn_key, tenant_id, thread_id, request_id, checkpoint_json, created_at, updated_at
                FROM agent_turn_checkpoints
                WHERE tenant_id = ?1 AND thread_id = ?2 AND request_id = ?3
                ORDER BY updated_at DESC"#,
            )
            .map_err(|e| format!("prepare turn checkpoints of {request_id}: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, thread_id, request_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| format!("turn checkpoints of {request_id}: {e}"))?;
        let mut records = Vec::new();
        for row in rows {
            let (turn_key, tenant_id, thread_id, request_id, checkpoint_json, created, updated) =
                row.map_err(|e| format!("read turn checkpoint: {e}"))?;
            records.push(TurnCheckpointRecord {
                turn_key,
                tenant_id,
                thread_id,
                request_id,
                checkpoint_json,
                created_at: parse_rfc3339(&created)?,
                updated_at: parse_rfc3339(&updated)?,
            });
        }
        Ok(records)
    }

    pub fn delete_turn_checkpoint(&self, turn_key: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM agent_turn_checkpoints WHERE turn_key = ?1",
                params![turn_key],
            )
            .map_err(|e| format!("delete turn checkpoint {turn_key}: {e}"))?;
        Ok(())
    }

    /// Deletes checkpoints not written since `cutoff`; returns how many.
    pub fn prune_turn_checkpoints(&self, cutoff: &DateTime<Utc>) -> Result<usize, String> {
        self.conn
            .execute(
                "DELETE FROM agent_turn_checkpoints WHERE updated_at < ?1",
                params![now_rfc3339(cutoff)],
            )
            .map_err(|e| format!("prune turn checkpoints: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "kura_store_turn_ckpt_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.to_string_lossy().to_string()
    }

    fn record(turn_key: &str, json: &str, at: DateTime<Utc>) -> TurnCheckpointRecord {
        TurnCheckpointRecord {
            turn_key: turn_key.to_string(),
            tenant_id: "tenant_a".to_string(),
            thread_id: "thread_1".to_string(),
            request_id: "req_1".to_string(),
            checkpoint_json: json.to_string(),
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn a_checkpoint_is_overwritten_in_place_and_keeps_its_creation_time() {
        let store = SQLiteStore::new(&temp_dir("put")).unwrap();
        let first = Utc::now() - chrono::Duration::minutes(5);
        store
            .put_turn_checkpoint(&record("k1", "{\"rounds\":[]}", first))
            .unwrap();
        let later = Utc::now();
        store
            .put_turn_checkpoint(&record("k1", "{\"rounds\":[{}]}", later))
            .unwrap();

        let got = store.get_turn_checkpoint("k1").unwrap().unwrap();
        assert_eq!(got.checkpoint_json, "{\"rounds\":[{}]}");
        assert_eq!(got.tenant_id, "tenant_a");
        assert_eq!(got.created_at.timestamp(), first.timestamp());
        assert_eq!(got.updated_at.timestamp(), later.timestamp());
        assert!(store.get_turn_checkpoint("k2").unwrap().is_none());

        store.delete_turn_checkpoint("k1").unwrap();
        assert!(store.get_turn_checkpoint("k1").unwrap().is_none());
    }

    #[test]
    fn pruning_removes_only_checkpoints_older_than_the_cutoff() {
        let store = SQLiteStore::new(&temp_dir("prune")).unwrap();
        let now = Utc::now();
        store
            .put_turn_checkpoint(&record("old", "{}", now - chrono::Duration::days(2)))
            .unwrap();
        store
            .put_turn_checkpoint(&record("new", "{}", now))
            .unwrap();

        let removed = store
            .prune_turn_checkpoints(&(now - chrono::Duration::days(1)))
            .unwrap();
        assert_eq!(removed, 1);
        assert!(store.get_turn_checkpoint("old").unwrap().is_none());
        assert!(store.get_turn_checkpoint("new").unwrap().is_some());
    }

    #[test]
    fn a_v6_database_gains_the_table_on_upgrade() {
        let dir = temp_dir("upgrade");
        {
            let store = SQLiteStore::new_at_version(&dir, 6).unwrap();
            assert_eq!(store.schema_version().unwrap(), 6);
            assert!(store.get_turn_checkpoint("k").is_err());
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        store
            .put_turn_checkpoint(&record("k", "{}", Utc::now()))
            .unwrap();
        assert!(store.get_turn_checkpoint("k").unwrap().is_some());
    }
}
