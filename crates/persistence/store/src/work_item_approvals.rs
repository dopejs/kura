//! The approvals a work item waits on (docs/harness/work-items.md §7,
//! slice 5).
//!
//! Rows only, as in `work_items.rs`: what a state means is
//! `kura-workitems`' business. Every per-item method takes the caller's
//! tenant and filters on it. Two reads serve every tenant, because the
//! policy engine names an approval without its tenant: the lookup by
//! approval id and the sweep of pending rows. Each row carries its own
//! tenant, and the caller acts for that tenant only.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, parse_rfc3339};
use crate::work_items::WorkItemInputRecord;

/// One `work_item_approvals` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemApprovalRecord {
    pub approval_id: String,
    pub tenant_id: String,
    pub item_id: String,
    /// The round whose call last gave up waiting on it.
    pub round_seq: i64,
    /// Which call the approval is for: a hash of its action, resource and
    /// arguments. A later call runs under it only if it is the same.
    pub call_fingerprint: String,
    /// What the person is asked (the approval's reason).
    pub question: String,
    /// `pending` | `approved` | `rejected` | `gone` | `used`.
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub used_at: Option<DateTime<Utc>>,
}

/// What [`SQLiteStore::resolve_work_item_approval`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApprovalResolution {
    /// The row moved from `pending`.
    pub applied: bool,
    /// The seq of the input recorded for the item, if one was.
    pub input_seq: Option<i64>,
    /// The item's attention was this approval and is now clear.
    pub attention_cleared: bool,
}

const COLUMNS: &str = "approval_id, tenant_id, item_id, round_seq, call_fingerprint, question, \
     state, created_at, updated_at, resolved_at, used_at";

impl SQLiteStore {
    /// Records that a round of the item gave up waiting on the approval.
    /// The first wait writes the row; a later one (the item's next round
    /// waited on the same approval again) moves it to that round. `false`
    /// when the approval is already recorded for another item or tenant,
    /// or is no longer pending: then nothing changed.
    pub fn record_work_item_approval_wait(
        &self,
        record: &WorkItemApprovalRecord,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                &format!(
                    "INSERT INTO work_item_approvals ({COLUMNS}) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7, NULL, NULL \
                     WHERE EXISTS (SELECT 1 FROM work_items \
                        WHERE item_id = ?3 AND tenant_id = ?2) \
                     ON CONFLICT(approval_id) DO UPDATE SET \
                        round_seq = excluded.round_seq, updated_at = excluded.updated_at \
                     WHERE work_item_approvals.tenant_id = excluded.tenant_id \
                       AND work_item_approvals.item_id = excluded.item_id \
                       AND work_item_approvals.state = 'pending'"
                ),
                params![
                    record.approval_id,
                    record.tenant_id,
                    record.item_id,
                    record.round_seq,
                    record.call_fingerprint,
                    record.question,
                    now_rfc3339(&record.updated_at),
                ],
            )
            .map_err(|e| format!("record approval wait {}: {e}", record.approval_id))?;
        Ok(changed == 1)
    }

    /// The approval the item holds for the call `fingerprint` that is still
    /// to be waited on or run under (`pending` or `approved`), the latest
    /// first.
    pub fn held_work_item_approval(
        &self,
        tenant_id: &str,
        item_id: &str,
        fingerprint: &str,
    ) -> Result<Option<WorkItemApprovalRecord>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM work_item_approvals \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND call_fingerprint = ?3 \
                       AND state IN ('pending', 'approved') \
                     ORDER BY created_at DESC, approval_id DESC LIMIT 1"
                ),
                params![tenant_id, item_id, fingerprint],
                approval_from_row,
            )
            .optional()
            .map_err(|e| format!("held approval of {item_id}: {e}"))?
            .transpose()
    }

    /// The item's latest approval that a round of `round_seq` gave up on and
    /// that is still pending: what the round's attention links.
    pub fn pending_work_item_approval_of_round(
        &self,
        tenant_id: &str,
        item_id: &str,
        round_seq: i64,
    ) -> Result<Option<WorkItemApprovalRecord>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM work_item_approvals \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND round_seq = ?3 \
                       AND state = 'pending' \
                     ORDER BY updated_at DESC, approval_id DESC LIMIT 1"
                ),
                params![tenant_id, item_id, round_seq],
                approval_from_row,
            )
            .optional()
            .map_err(|e| format!("pending approval of {item_id}#{round_seq}: {e}"))?
            .transpose()
    }

    /// Every approval the item waited on, oldest first.
    pub fn list_work_item_approvals(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemApprovalRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM work_item_approvals \
                 WHERE tenant_id = ?1 AND item_id = ?2 \
                 ORDER BY created_at ASC, approval_id ASC"
            ))
            .map_err(|e| format!("prepare list work item approvals: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], approval_from_row)
            .map_err(|e| format!("list approvals of {item_id}: {e}"))?;
        collect(rows)
    }

    /// The row of an approval, whichever tenant's: the policy engine names
    /// an approval without its tenant. The caller acts for the row's.
    pub fn work_item_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<WorkItemApprovalRecord>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM work_item_approvals WHERE approval_id = ?1"),
                params![approval_id],
                approval_from_row,
            )
            .optional()
            .map_err(|e| format!("work item approval {approval_id}: {e}"))?
            .transpose()
    }

    /// Every tenant's pending rows, oldest first: the sweep that catches a
    /// resolution whose event was missed (the bus drops what a full
    /// subscriber cannot take) or that happened while the daemon was down.
    pub fn pending_work_item_approvals(
        &self,
        limit: usize,
    ) -> Result<Vec<WorkItemApprovalRecord>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM work_item_approvals WHERE state = 'pending' \
                 ORDER BY created_at ASC, approval_id ASC LIMIT ?1"
            ))
            .map_err(|e| format!("prepare pending work item approvals: {e}"))?;
        let rows = stmt
            .query_map(params![limit], approval_from_row)
            .map_err(|e| format!("pending work item approvals: {e}"))?;
        collect(rows)
    }

    /// Moves a pending row to `state` (`approved` | `rejected` | `gone`),
    /// once. In the same transaction, when `input` is given, it is recorded
    /// as the item's next input (`input.seq` is ignored) so the item's next
    /// round reads the outcome; and when the item's attention is exactly
    /// `attention_json`, the attention is cleared, because what it asked is
    /// answered. Nothing is written when the row is not pending.
    pub fn resolve_work_item_approval(
        &self,
        approval_id: &str,
        state: &str,
        at: DateTime<Utc>,
        input: Option<&WorkItemInputRecord>,
        attention_json: Option<&str>,
    ) -> Result<ApprovalResolution, String> {
        if !matches!(state, "approved" | "rejected" | "gone") {
            return Err(format!("{state:?} is not an approval outcome"));
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin resolve work item approval: {e}"))?;
        let when = now_rfc3339(&at);
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT tenant_id, item_id FROM work_item_approvals \
                 WHERE approval_id = ?1 AND state = 'pending'",
                params![approval_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("read work item approval {approval_id}: {e}"))?;
        let Some((tenant_id, item_id)) = row else {
            return Ok(ApprovalResolution::default());
        };
        tx.execute(
            "UPDATE work_item_approvals SET state = ?2, resolved_at = ?3, updated_at = ?3 \
             WHERE approval_id = ?1 AND state = 'pending'",
            params![approval_id, state, when],
        )
        .map_err(|e| format!("resolve work item approval {approval_id}: {e}"))?;
        let mut resolution = ApprovalResolution {
            applied: true,
            ..ApprovalResolution::default()
        };
        if let Some(attention_json) = attention_json {
            let cleared = tx
                .execute(
                    "UPDATE work_items SET attention_json = NULL, updated_at = ?4 \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND attention_json = ?3",
                    params![tenant_id, item_id, attention_json, when],
                )
                .map_err(|e| format!("clear attention of {item_id}: {e}"))?;
            resolution.attention_cleared = cleared == 1;
        }
        if let Some(input) = input {
            if input.tenant_id != tenant_id || input.item_id != item_id {
                return Err(format!(
                    "the input for approval {approval_id} names another item"
                ));
            }
            resolution.input_seq = Some(append_input(&tx, input, "pending", None)?);
            tx.execute(
                "UPDATE work_items SET updated_at = ?3 WHERE tenant_id = ?1 AND item_id = ?2",
                params![tenant_id, item_id, when],
            )
            .map_err(|e| format!("touch work item {item_id}: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("commit resolve work item approval {approval_id}: {e}"))?;
        Ok(resolution)
    }

    /// A call is about to run under the approval: the row is `used` and is
    /// never held again. An outcome input for it still waiting (recorded
    /// when it was approved) is consumed by the round running the call, so
    /// the item does not run another round to be told what it just did.
    /// `false` when the row is not the item's or is not pending or approved.
    pub fn use_work_item_approval(
        &self,
        tenant_id: &str,
        item_id: &str,
        approval_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin use work item approval: {e}"))?;
        let when = now_rfc3339(&at);
        let changed = tx
            .execute(
                "UPDATE work_item_approvals SET state = 'used', used_at = ?4, updated_at = ?4 \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND approval_id = ?3 \
                   AND state IN ('pending', 'approved')",
                params![tenant_id, item_id, approval_id, when],
            )
            .map_err(|e| format!("use work item approval {approval_id}: {e}"))?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE work_item_inputs SET state = 'consumed', \
                round_seq = (SELECT seq FROM work_items WHERE item_id = ?2) \
             WHERE tenant_id = ?1 AND item_id = ?2 AND kind = 'approval' \
               AND source_ref = ?3 AND state = 'pending'",
            params![tenant_id, item_id, approval_id],
        )
        .map_err(|e| format!("consume approval input of {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit use work item approval {approval_id}: {e}"))?;
        Ok(true)
    }
}

/// Appends `input` as the item's next input with `state` and `round_seq`,
/// returning its seq; `input.seq`, `input.state` and `input.round_seq` are
/// ignored. Fails when the item is not the tenant's.
pub(crate) fn append_input(
    conn: &rusqlite::Connection,
    input: &WorkItemInputRecord,
    state: &str,
    round_seq: Option<i64>,
) -> Result<i64, String> {
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM work_item_inputs WHERE item_id = ?1",
            params![input.item_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("next input seq of {}: {e}", input.item_id))?;
    let written = conn
        .execute(
            "INSERT INTO work_item_inputs (item_id, seq, tenant_id, kind, author_is_person, \
                body, source_ref, state, round_seq, created_at) \
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
             WHERE EXISTS (SELECT 1 FROM work_items WHERE item_id = ?1 AND tenant_id = ?3)",
            params![
                input.item_id,
                seq,
                input.tenant_id,
                input.kind,
                input.author_is_person,
                input.body,
                input.source_ref,
                state,
                round_seq,
                now_rfc3339(&input.created_at),
            ],
        )
        .map_err(|e| format!("append work item input {}: {e}", input.item_id))?;
    if written != 1 {
        return Err(format!(
            "append work item input {}: no such item for the tenant",
            input.item_id
        ));
    }
    Ok(seq)
}

type Parsed<T> = rusqlite::Result<Result<T, String>>;

fn collect(
    rows: impl Iterator<Item = rusqlite::Result<Result<WorkItemApprovalRecord, String>>>,
) -> Result<Vec<WorkItemApprovalRecord>, String> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read work item approval: {e}"))??);
    }
    Ok(out)
}

fn approval_from_row(row: &Row<'_>) -> Parsed<WorkItemApprovalRecord> {
    let time = |index: usize| -> rusqlite::Result<Result<DateTime<Utc>, String>> {
        Ok(parse_rfc3339(&row.get::<_, String>(index)?))
    };
    let optional = |index: usize| -> rusqlite::Result<Result<Option<DateTime<Utc>>, String>> {
        Ok(row
            .get::<_, Option<String>>(index)?
            .as_deref()
            .map(parse_rfc3339)
            .transpose())
    };
    let (created_at, updated_at, resolved_at, used_at) =
        match (time(7)?, time(8)?, optional(9)?, optional(10)?) {
            (Ok(created), Ok(updated), Ok(resolved), Ok(used)) => {
                (created, updated, resolved, used)
            }
            (Err(err), ..) | (_, Err(err), ..) | (_, _, Err(err), _) | (.., Err(err)) => {
                return Ok(Err(err));
            }
        };
    Ok(Ok(WorkItemApprovalRecord {
        approval_id: row.get(0)?,
        tenant_id: row.get(1)?,
        item_id: row.get(2)?,
        round_seq: row.get(3)?,
        call_fingerprint: row.get(4)?,
        question: row.get(5)?,
        state: row.get(6)?,
        created_at,
        updated_at,
        resolved_at,
        used_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_items::{WorkItemRecord, WorkItemRoundRecord};

    fn store(name: &str) -> SQLiteStore {
        let dir = std::env::temp_dir().join(format!(
            "kura_store_work_item_approvals_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SQLiteStore::new(dir.to_str().unwrap()).unwrap()
    }

    fn open_item(store: &SQLiteStore, tenant_id: &str, item_id: &str, attention: Option<&str>) {
        let at = Utc::now();
        store
            .create_work_item(
                &WorkItemRecord {
                    item_id: item_id.to_string(),
                    tenant_id: tenant_id.to_string(),
                    title: "t".to_string(),
                    state: "open".to_string(),
                    end_reason: None,
                    thread_id: format!("thr_{item_id}"),
                    provider: String::new(),
                    model: String::new(),
                    seq: 1,
                    mandate_json: "{}".to_string(),
                    last_report_json: None,
                    attention_json: attention.map(str::to_string),
                    policy_hash_seen: String::new(),
                    created_at: at,
                    updated_at: at,
                },
                &WorkItemRoundRecord {
                    item_id: item_id.to_string(),
                    seq: 1,
                    tenant_id: tenant_id.to_string(),
                    request_id: format!("{item_id}_r1"),
                    query_text: "q".to_string(),
                    state: "running".to_string(),
                    outcome: None,
                    report_json: None,
                    error_code: None,
                    turn_id: None,
                    started_at: at,
                    finished_at: None,
                },
            )
            .unwrap();
    }

    fn wait(
        tenant_id: &str,
        item_id: &str,
        approval_id: &str,
        round_seq: i64,
    ) -> WorkItemApprovalRecord {
        WorkItemApprovalRecord {
            approval_id: approval_id.to_string(),
            tenant_id: tenant_id.to_string(),
            item_id: item_id.to_string(),
            round_seq,
            call_fingerprint: "fp_1".to_string(),
            question: "run advance".to_string(),
            state: "pending".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            resolved_at: None,
            used_at: None,
        }
    }

    fn outcome_input(tenant_id: &str, item_id: &str, approval_id: &str) -> WorkItemInputRecord {
        WorkItemInputRecord {
            item_id: item_id.to_string(),
            seq: 0,
            tenant_id: tenant_id.to_string(),
            kind: "approval".to_string(),
            author_is_person: false,
            body: "approved".to_string(),
            source_ref: approval_id.to_string(),
            state: "pending".to_string(),
            round_seq: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn a_wait_is_recorded_for_one_item_and_held_until_used() {
        let store = store("held");
        open_item(&store, "ten_a", "wi_1", None);
        open_item(&store, "ten_b", "wi_2", None);
        assert!(
            store
                .record_work_item_approval_wait(&wait("ten_a", "wi_1", "ap_1", 1))
                .unwrap()
        );
        // A second wait moves it to the later round.
        assert!(
            store
                .record_work_item_approval_wait(&wait("ten_a", "wi_1", "ap_1", 2))
                .unwrap()
        );
        // Another item or tenant cannot take it over, nor record for an
        // item it does not hold.
        assert!(
            !store
                .record_work_item_approval_wait(&wait("ten_b", "wi_2", "ap_1", 1))
                .unwrap()
        );
        assert!(
            !store
                .record_work_item_approval_wait(&wait("ten_b", "wi_1", "ap_9", 1))
                .unwrap()
        );
        assert!(store.work_item_approval("ap_9").unwrap().is_none());

        let held = store
            .held_work_item_approval("ten_a", "wi_1", "fp_1")
            .unwrap()
            .unwrap();
        assert_eq!((held.approval_id.as_str(), held.round_seq), ("ap_1", 2));
        assert!(
            store
                .held_work_item_approval("ten_b", "wi_1", "fp_1")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .held_work_item_approval("ten_a", "wi_1", "fp_2")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .pending_work_item_approval_of_round("ten_a", "wi_1", 2)
                .unwrap()
                .map(|r| r.approval_id),
            Some("ap_1".to_string())
        );
        assert!(
            store
                .pending_work_item_approval_of_round("ten_a", "wi_1", 1)
                .unwrap()
                .is_none()
        );

        assert!(
            !store
                .use_work_item_approval("ten_b", "wi_1", "ap_1", Utc::now())
                .unwrap()
        );
        assert!(
            store
                .use_work_item_approval("ten_a", "wi_1", "ap_1", Utc::now())
                .unwrap()
        );
        assert!(
            !store
                .use_work_item_approval("ten_a", "wi_1", "ap_1", Utc::now())
                .unwrap(),
            "used once"
        );
        assert!(
            store
                .held_work_item_approval("ten_a", "wi_1", "fp_1")
                .unwrap()
                .is_none()
        );
        // A used approval is no longer pending: no wait moves it back.
        assert!(
            !store
                .record_work_item_approval_wait(&wait("ten_a", "wi_1", "ap_1", 3))
                .unwrap()
        );
    }

    #[test]
    fn a_resolution_applies_once_records_the_outcome_and_clears_its_attention() {
        let store = store("resolve");
        let attention = r#"{"kind":"approval","question":"run advance","approvalId":"ap_1"}"#;
        open_item(&store, "ten_a", "wi_1", Some(attention));
        store
            .record_work_item_approval_wait(&wait("ten_a", "wi_1", "ap_1", 1))
            .unwrap();
        assert_eq!(store.pending_work_item_approvals(10).unwrap().len(), 1);
        assert!(
            store
                .resolve_work_item_approval("ap_1", "used", Utc::now(), None, None)
                .is_err()
        );
        // Another item's input is refused and nothing is written.
        let foreign = outcome_input("ten_a", "wi_2", "ap_1");
        assert!(
            store
                .resolve_work_item_approval("ap_1", "approved", Utc::now(), Some(&foreign), None)
                .is_err()
        );
        assert_eq!(
            store.work_item_approval("ap_1").unwrap().unwrap().state,
            "pending"
        );

        let own = outcome_input("ten_a", "wi_1", "ap_1");
        let resolved = store
            .resolve_work_item_approval("ap_1", "approved", Utc::now(), Some(&own), Some(attention))
            .unwrap();
        assert_eq!(
            resolved,
            ApprovalResolution {
                applied: true,
                input_seq: Some(1),
                attention_cleared: true
            }
        );
        let (item, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert!(item.attention_json.is_none());
        assert_eq!(activity.pending_inputs, 1);
        assert_eq!(activity.pending_person_inputs, 0);
        let again = store
            .resolve_work_item_approval("ap_1", "rejected", Utc::now(), Some(&own), None)
            .unwrap();
        assert_eq!(again, ApprovalResolution::default());
        let row = store.work_item_approval("ap_1").unwrap().unwrap();
        assert_eq!(row.state, "approved");
        assert!(row.resolved_at.is_some());
        assert!(store.pending_work_item_approvals(10).unwrap().is_empty());

        // Running the call under it consumes the outcome input it left.
        assert!(
            store
                .use_work_item_approval("ten_a", "wi_1", "ap_1", Utc::now())
                .unwrap()
        );
        let inputs = store.list_work_item_inputs("ten_a", "wi_1").unwrap();
        assert_eq!(inputs[0].state, "consumed");
        assert_eq!(inputs[0].round_seq, Some(1));
    }

    #[test]
    fn a_resolution_leaves_another_attention_alone() {
        let store = store("other_attention");
        let question = r#"{"kind":"question","question":"Which one?"}"#;
        open_item(&store, "ten_a", "wi_1", Some(question));
        store
            .record_work_item_approval_wait(&wait("ten_a", "wi_1", "ap_1", 1))
            .unwrap();
        let theirs = r#"{"kind":"approval","question":"run advance","approvalId":"ap_1"}"#;
        let resolved = store
            .resolve_work_item_approval("ap_1", "rejected", Utc::now(), None, Some(theirs))
            .unwrap();
        assert!(resolved.applied && !resolved.attention_cleared);
        let (item, _) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(item.attention_json.as_deref(), Some(question));
    }

    #[test]
    fn an_answer_clears_the_question_it_answers_once() {
        let store = store("answer");
        let question = r#"{"kind":"question","question":"Which one?"}"#;
        open_item(&store, "ten_a", "wi_1", Some(question));
        let mut answer = outcome_input("ten_a", "wi_1", "");
        answer.kind = "text".to_string();
        answer.author_is_person = true;
        assert!(
            store
                .answer_work_item("ten_b", "wi_1", question, &answer)
                .is_err()
        );
        assert_eq!(
            store
                .answer_work_item("ten_a", "wi_1", "{}", &answer)
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .answer_work_item("ten_a", "wi_1", question, &answer)
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            store
                .answer_work_item("ten_a", "wi_1", question, &answer)
                .unwrap(),
            None
        );
        let (item, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert!(item.attention_json.is_none());
        assert_eq!(activity.pending_person_inputs, 1);
    }

    /// Slice 5 upgrade: a v12 database keeps its items and gains the
    /// approvals table.
    #[test]
    fn a_v12_database_gains_the_approvals_table_on_upgrade() {
        let dir = std::env::temp_dir().join(format!(
            "kura_store_work_item_approvals_upgrade_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = dir.to_str().unwrap().to_string();
        {
            let store = SQLiteStore::new_at_version(&dir, 12).unwrap();
            open_item(&store, "ten_a", "wi_1", None);
            assert!(store.list_work_item_approvals("ten_a", "wi_1").is_err());
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        assert!(store.get_work_item("ten_a", "wi_1").unwrap().is_some());
        assert!(
            store
                .list_work_item_approvals("ten_a", "wi_1")
                .unwrap()
                .is_empty()
        );
    }
}
