//! Work items, their rounds and their inputs (docs/harness/work-items.md).
//!
//! Rows only: what a state, a report or a mandate means is `kura-workitems`'
//! business, and it hands this module strings and JSON. Every method takes
//! the caller's tenant and filters on it, so a row of another tenant is
//! indistinguishable from a row that does not exist.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, parse_rfc3339};

/// One `work_items` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRecord {
    pub item_id: String,
    pub tenant_id: String,
    pub title: String,
    /// `open` | `ended`.
    pub state: String,
    pub end_reason: Option<String>,
    pub thread_id: String,
    pub provider: String,
    pub model: String,
    /// The last round started; 0 before the first.
    pub seq: i64,
    /// Written once with the item; no method here updates it.
    pub mandate_json: String,
    pub last_report_json: Option<String>,
    pub attention_json: Option<String>,
    pub policy_hash_seen: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What the item's other rows say about it, read with the item: the facts
/// its stage is derived from besides its own columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkItemActivity {
    /// The state of the item's latest round, if it has one.
    pub last_round_state: Option<String>,
    pub pending_inputs: i64,
    /// Pending inputs a person wrote: a person's input since the last
    /// report, because a round consumes every input pending when it starts.
    pub pending_person_inputs: i64,
}

/// One `work_item_rounds` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRoundRecord {
    pub item_id: String,
    pub seq: i64,
    pub tenant_id: String,
    pub request_id: String,
    pub query_text: String,
    /// `running` | `settled` | `failed`.
    pub state: String,
    pub outcome: Option<String>,
    pub report_json: Option<String>,
    pub error_code: Option<String>,
    pub turn_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// One `work_item_inputs` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemInputRecord {
    pub item_id: String,
    pub seq: i64,
    pub tenant_id: String,
    pub kind: String,
    pub author_is_person: bool,
    pub body: String,
    pub source_ref: String,
    /// `pending` | `steered` | `consumed`.
    pub state: String,
    pub round_seq: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// How a round ended, and what that does to its item. Applied in one
/// transaction so an item never shows a settled round with a stale report.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRoundSettlement {
    /// `settled` (a report was read, or recorded as unreadable) or `failed`
    /// (the turn ended without one).
    pub round_state: String,
    pub outcome: String,
    pub report_json: Option<String>,
    pub error_code: Option<String>,
    pub turn_id: Option<String>,
    pub finished_at: DateTime<Utc>,
    /// The item's state after the round: `open` or `ended`.
    pub item_state: String,
    pub end_reason: Option<String>,
    /// Replaces the item's last report when set; a failed round leaves it.
    pub last_report_json: Option<String>,
    /// Replaces the item's attention when set; `None` leaves it as it was.
    pub attention_json: Option<String>,
}

const ITEM_COLUMNS: &str = "i.item_id, i.tenant_id, i.title, i.state, i.end_reason, \
     i.thread_id, i.provider, i.model, i.seq, i.mandate_json, i.last_report_json, \
     i.attention_json, i.policy_hash_seen, i.created_at, i.updated_at, \
     (SELECT r.state FROM work_item_rounds r WHERE r.item_id = i.item_id \
        ORDER BY r.seq DESC LIMIT 1), \
     (SELECT COUNT(*) FROM work_item_inputs n WHERE n.item_id = i.item_id \
        AND n.state = 'pending'), \
     (SELECT COUNT(*) FROM work_item_inputs n WHERE n.item_id = i.item_id \
        AND n.state = 'pending' AND n.author_is_person = 1)";

const ROUND_COLUMNS: &str = "item_id, seq, tenant_id, request_id, query_text, state, outcome, \
     report_json, error_code, turn_id, started_at, finished_at";

const INPUT_COLUMNS: &str = "item_id, seq, tenant_id, kind, author_is_person, body, source_ref, \
     state, round_seq, created_at";

impl SQLiteStore {
    /// Writes a new item together with its first round, so an item is never
    /// visible without the round that was started for it. Fails if the id
    /// is taken.
    pub fn create_work_item(
        &self,
        item: &WorkItemRecord,
        round: &WorkItemRoundRecord,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create work item: {e}"))?;
        tx.execute(
            r#"INSERT INTO work_items
                (item_id, tenant_id, title, state, end_reason, thread_id, provider, model, seq,
                 mandate_json, last_report_json, attention_json, policy_hash_seen,
                 created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
            params![
                item.item_id,
                item.tenant_id,
                item.title,
                item.state,
                item.end_reason,
                item.thread_id,
                item.provider,
                item.model,
                item.seq,
                item.mandate_json,
                item.last_report_json,
                item.attention_json,
                item.policy_hash_seen,
                now_rfc3339(&item.created_at),
                now_rfc3339(&item.updated_at),
            ],
        )
        .map_err(|e| format!("insert work item {}: {e}", item.item_id))?;
        insert_round(&tx, round)?;
        tx.commit()
            .map_err(|e| format!("commit work item {}: {e}", item.item_id))
    }

    /// The item, if `tenant_id` owns it.
    pub fn get_work_item(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Option<(WorkItemRecord, WorkItemActivity)>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items i \
                     WHERE i.tenant_id = ?1 AND i.item_id = ?2"
                ),
                params![tenant_id, item_id],
                item_from_row,
            )
            .optional()
            .map_err(|e| format!("get work item {item_id}: {e}"))?
            .transpose()
    }

    /// The tenant's items, most recently updated first; `state` narrows to
    /// `open` or `ended`.
    pub fn list_work_items(
        &self,
        tenant_id: &str,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(WorkItemRecord, WorkItemActivity)>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM work_items i \
                 WHERE i.tenant_id = ?1 AND (?2 IS NULL OR i.state = ?2) \
                 ORDER BY i.updated_at DESC, i.item_id DESC LIMIT ?3"
            ))
            .map_err(|e| format!("prepare list work items: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, state, limit], item_from_row)
            .map_err(|e| format!("list work items: {e}"))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("read work item: {e}"))??);
        }
        Ok(items)
    }

    /// Open items the tenant holds, for the per-tenant cap.
    pub fn count_open_work_items(&self, tenant_id: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM work_items WHERE tenant_id = ?1 AND state = 'open'",
                params![tenant_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count open work items: {e}"))
    }

    /// The item's rounds, oldest first.
    pub fn list_work_item_rounds(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemRoundRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ROUND_COLUMNS} FROM work_item_rounds \
                 WHERE tenant_id = ?1 AND item_id = ?2 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("prepare list work item rounds: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], round_from_row)
            .map_err(|e| format!("list work item rounds {item_id}: {e}"))?;
        let mut rounds = Vec::new();
        for row in rows {
            rounds.push(row.map_err(|e| format!("read work item round: {e}"))??);
        }
        Ok(rounds)
    }

    /// Records an input. Its `seq` must be the item's next; a taken one
    /// fails rather than overwriting what was said.
    pub fn insert_work_item_input(&self, input: &WorkItemInputRecord) -> Result<(), String> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO work_item_inputs ({INPUT_COLUMNS}) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
                     WHERE EXISTS (SELECT 1 FROM work_items \
                        WHERE item_id = ?1 AND tenant_id = ?3)"
                ),
                params![
                    input.item_id,
                    input.seq,
                    input.tenant_id,
                    input.kind,
                    input.author_is_person,
                    input.body,
                    input.source_ref,
                    input.state,
                    input.round_seq,
                    now_rfc3339(&input.created_at),
                ],
            )
            .map_err(|e| {
                format!(
                    "insert work item input {}#{}: {e}",
                    input.item_id, input.seq
                )
            })
            .and_then(|written| match written {
                1 => Ok(()),
                _ => Err(format!(
                    "insert work item input {}#{}: no such item for the tenant",
                    input.item_id, input.seq
                )),
            })
    }

    /// The item's inputs, oldest first.
    pub fn list_work_item_inputs(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemInputRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {INPUT_COLUMNS} FROM work_item_inputs \
                 WHERE tenant_id = ?1 AND item_id = ?2 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("prepare list work item inputs: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], input_from_row)
            .map_err(|e| format!("list work item inputs {item_id}: {e}"))?;
        let mut inputs = Vec::new();
        for row in rows {
            inputs.push(row.map_err(|e| format!("read work item input: {e}"))??);
        }
        Ok(inputs)
    }

    /// Ends a running round and applies what it did to its item, in one
    /// transaction. `false` when the round is not running (already settled,
    /// or not the tenant's): a settlement is applied at most once.
    pub fn settle_work_item_round(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        settlement: &WorkItemRoundSettlement,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin settle work item round: {e}"))?;
        let finished = now_rfc3339(&settlement.finished_at);
        let changed = tx
            .execute(
                r#"UPDATE work_item_rounds SET
                    state = ?4, outcome = ?5, report_json = ?6, error_code = ?7,
                    turn_id = ?8, finished_at = ?9
                WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 AND state = 'running'"#,
                params![
                    tenant_id,
                    item_id,
                    seq,
                    settlement.round_state,
                    settlement.outcome,
                    settlement.report_json,
                    settlement.error_code,
                    settlement.turn_id,
                    finished,
                ],
            )
            .map_err(|e| format!("settle work item round {item_id}#{seq}: {e}"))?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            r#"UPDATE work_items SET
                state = ?3,
                end_reason = ?4,
                last_report_json = COALESCE(?5, last_report_json),
                attention_json = COALESCE(?6, attention_json),
                updated_at = ?7
            WHERE tenant_id = ?1 AND item_id = ?2"#,
            params![
                tenant_id,
                item_id,
                settlement.item_state,
                settlement.end_reason,
                settlement.last_report_json,
                settlement.attention_json,
                finished,
            ],
        )
        .map_err(|e| format!("apply round to work item {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit settle work item round {item_id}#{seq}: {e}"))?;
        Ok(true)
    }
}

fn insert_round(conn: &rusqlite::Connection, round: &WorkItemRoundRecord) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO work_item_rounds ({ROUND_COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        ),
        params![
            round.item_id,
            round.seq,
            round.tenant_id,
            round.request_id,
            round.query_text,
            round.state,
            round.outcome,
            round.report_json,
            round.error_code,
            round.turn_id,
            now_rfc3339(&round.started_at),
            round.finished_at.as_ref().map(now_rfc3339),
        ],
    )
    .map_err(|e| {
        format!(
            "insert work item round {}#{}: {e}",
            round.item_id, round.seq
        )
    })?;
    Ok(())
}

/// A row read, then its timestamps parsed: the outer `Result` is SQLite's,
/// the inner one says a stored timestamp did not parse.
type Parsed<T> = rusqlite::Result<Result<T, String>>;

/// Reads a stored timestamp column, leaving a parse failure for the caller
/// to report with the row's context.
fn time_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Result<DateTime<Utc>, String>> {
    Ok(parse_rfc3339(&row.get::<_, String>(index)?))
}

fn item_from_row(row: &Row<'_>) -> Parsed<(WorkItemRecord, WorkItemActivity)> {
    let (created_at, updated_at) = match (time_at(row, 13)?, time_at(row, 14)?) {
        (Ok(created), Ok(updated)) => (created, updated),
        (Err(err), _) | (_, Err(err)) => return Ok(Err(err)),
    };
    let record = WorkItemRecord {
        item_id: row.get(0)?,
        tenant_id: row.get(1)?,
        title: row.get(2)?,
        state: row.get(3)?,
        end_reason: row.get(4)?,
        thread_id: row.get(5)?,
        provider: row.get(6)?,
        model: row.get(7)?,
        seq: row.get(8)?,
        mandate_json: row.get(9)?,
        last_report_json: row.get(10)?,
        attention_json: row.get(11)?,
        policy_hash_seen: row.get(12)?,
        created_at,
        updated_at,
    };
    let activity = WorkItemActivity {
        last_round_state: row.get(15)?,
        pending_inputs: row.get(16)?,
        pending_person_inputs: row.get(17)?,
    };
    Ok(Ok((record, activity)))
}

fn round_from_row(row: &Row<'_>) -> Parsed<WorkItemRoundRecord> {
    let started_at = match time_at(row, 10)? {
        Ok(at) => at,
        Err(err) => return Ok(Err(err)),
    };
    let finished_at = match row
        .get::<_, Option<String>>(11)?
        .as_deref()
        .map(parse_rfc3339)
    {
        Some(Err(err)) => return Ok(Err(err)),
        Some(Ok(at)) => Some(at),
        None => None,
    };
    Ok(Ok(WorkItemRoundRecord {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        tenant_id: row.get(2)?,
        request_id: row.get(3)?,
        query_text: row.get(4)?,
        state: row.get(5)?,
        outcome: row.get(6)?,
        report_json: row.get(7)?,
        error_code: row.get(8)?,
        turn_id: row.get(9)?,
        started_at,
        finished_at,
    }))
}

fn input_from_row(row: &Row<'_>) -> Parsed<WorkItemInputRecord> {
    let created_at = match time_at(row, 9)? {
        Ok(at) => at,
        Err(err) => return Ok(Err(err)),
    };
    Ok(Ok(WorkItemInputRecord {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        tenant_id: row.get(2)?,
        kind: row.get(3)?,
        author_is_person: row.get(4)?,
        body: row.get(5)?,
        source_ref: row.get(6)?,
        state: row.get(7)?,
        round_seq: row.get(8)?,
        created_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "kura_store_work_items_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.to_string_lossy().to_string()
    }

    fn item(tenant_id: &str, item_id: &str, at: DateTime<Utc>) -> WorkItemRecord {
        WorkItemRecord {
            item_id: item_id.to_string(),
            tenant_id: tenant_id.to_string(),
            title: "Fix the build".to_string(),
            state: "open".to_string(),
            end_reason: None,
            thread_id: format!("thr_{item_id}"),
            provider: "echo".to_string(),
            model: String::new(),
            seq: 1,
            mandate_json: r#"{"basis":"api"}"#.to_string(),
            last_report_json: None,
            attention_json: None,
            policy_hash_seen: String::new(),
            created_at: at,
            updated_at: at,
        }
    }

    fn round(tenant_id: &str, item_id: &str, at: DateTime<Utc>) -> WorkItemRoundRecord {
        WorkItemRoundRecord {
            item_id: item_id.to_string(),
            seq: 1,
            tenant_id: tenant_id.to_string(),
            request_id: format!("{item_id}_r1"),
            query_text: "do the work".to_string(),
            state: "running".to_string(),
            outcome: None,
            report_json: None,
            error_code: None,
            turn_id: None,
            started_at: at,
            finished_at: None,
        }
    }

    fn input(tenant_id: &str, item_id: &str, seq: i64, person: bool) -> WorkItemInputRecord {
        WorkItemInputRecord {
            item_id: item_id.to_string(),
            seq,
            tenant_id: tenant_id.to_string(),
            kind: "text".to_string(),
            author_is_person: person,
            body: format!("input {seq}"),
            source_ref: String::new(),
            state: "pending".to_string(),
            round_seq: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn an_item_its_round_and_its_inputs_read_back_as_written() {
        let store = SQLiteStore::new(&temp_dir("roundtrip")).unwrap();
        let at = Utc::now();
        let written = item("ten_a", "wi_1", at);
        store
            .create_work_item(&written, &round("ten_a", "wi_1", at))
            .unwrap();
        store
            .insert_work_item_input(&input("ten_a", "wi_1", 1, true))
            .unwrap();
        store
            .insert_work_item_input(&input("ten_a", "wi_1", 2, false))
            .unwrap();

        let (read, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.item_id, written.item_id);
        assert_eq!(read.mandate_json, written.mandate_json);
        assert_eq!(read.provider, "echo");
        assert_eq!(read.created_at.timestamp_micros(), at.timestamp_micros());
        assert_eq!(
            activity,
            WorkItemActivity {
                last_round_state: Some("running".to_string()),
                pending_inputs: 2,
                pending_person_inputs: 1,
            }
        );
        let rounds = store.list_work_item_rounds("ten_a", "wi_1").unwrap();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].request_id, "wi_1_r1");
        assert_eq!(rounds[0].query_text, "do the work");
        let inputs = store.list_work_item_inputs("ten_a", "wi_1").unwrap();
        assert_eq!(inputs.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert!(inputs[0].author_is_person);
        assert!(!inputs[1].author_is_person);
        // A seq is said once.
        assert!(
            store
                .insert_work_item_input(&input("ten_a", "wi_1", 2, true))
                .is_err()
        );
        assert_eq!(store.count_open_work_items("ten_a").unwrap(), 1);
    }

    #[test]
    fn a_settlement_applies_once_and_keeps_the_mandate() {
        let store = SQLiteStore::new(&temp_dir("settle")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        let settlement = WorkItemRoundSettlement {
            round_state: "settled".to_string(),
            outcome: "ready".to_string(),
            report_json: Some(r#"{"status":"ready","summary":"done"}"#.to_string()),
            error_code: None,
            turn_id: Some("trn_1".to_string()),
            finished_at: at + chrono::Duration::seconds(5),
            item_state: "ended".to_string(),
            end_reason: Some("completed".to_string()),
            last_report_json: Some(r#"{"status":"ready","summary":"done"}"#.to_string()),
            attention_json: None,
        };
        assert!(
            store
                .settle_work_item_round("ten_a", "wi_1", 1, &settlement)
                .unwrap()
        );
        assert!(
            !store
                .settle_work_item_round("ten_a", "wi_1", 1, &settlement)
                .unwrap(),
            "a settled round settled again"
        );
        let (read, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.state, "ended");
        assert_eq!(read.end_reason.as_deref(), Some("completed"));
        assert_eq!(read.mandate_json, r#"{"basis":"api"}"#);
        assert!(read.attention_json.is_none());
        assert_eq!(activity.last_round_state.as_deref(), Some("settled"));
        let rounds = store.list_work_item_rounds("ten_a", "wi_1").unwrap();
        assert_eq!(rounds[0].outcome.as_deref(), Some("ready"));
        assert_eq!(rounds[0].turn_id.as_deref(), Some("trn_1"));
        assert!(rounds[0].finished_at.is_some());
        assert_eq!(store.count_open_work_items("ten_a").unwrap(), 0);
        assert_eq!(
            store
                .list_work_items("ten_a", Some("ended"), 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_work_items("ten_a", Some("open"), 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn another_tenants_item_reads_as_absent() {
        let store = SQLiteStore::new(&temp_dir("tenant")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(store.get_work_item("ten_b", "wi_1").unwrap().is_none());
        assert!(store.list_work_items("ten_b", None, 10).unwrap().is_empty());
        assert!(
            store
                .list_work_item_rounds("ten_b", "wi_1")
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .insert_work_item_input(&input("ten_b", "wi_1", 1, true))
                .is_err(),
            "an input was written to another tenant's item"
        );
        let settlement = WorkItemRoundSettlement {
            round_state: "failed".to_string(),
            outcome: "provider".to_string(),
            report_json: None,
            error_code: Some("provider".to_string()),
            turn_id: None,
            finished_at: at,
            item_state: "open".to_string(),
            end_reason: None,
            last_report_json: None,
            attention_json: None,
        };
        assert!(
            !store
                .settle_work_item_round("ten_b", "wi_1", 1, &settlement)
                .unwrap(),
            "another tenant settled the round"
        );
        assert_eq!(store.list_work_items("ten_a", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_v9_database_gains_the_tables_on_upgrade() {
        let dir = temp_dir("upgrade");
        {
            let store = SQLiteStore::new_at_version(&dir, 9).unwrap();
            assert!(store.get_work_item("ten_a", "wi_1").is_err());
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(store.get_work_item("ten_a", "wi_1").unwrap().is_some());
    }
}
