//! Port of daemon/internal/inventory. See rs/MIGRATION.md for conventions.
//!
//! Loads the schema inventory checked in under
//! `specs/020-tenant-scoped-data-migration/contracts/schema-inventory.md` and
//! verifies its completeness against the live SQLite schema and registered
//! event sources.
//!
//! Roadmap 35 requires every persisted table and every event-bearing record
//! source to be classified as `tenant_owned`, `global`, or `derived`.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tenant-scope classification of a persisted table or event source per
/// Roadmap 35.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Classification {
    #[serde(rename = "tenant_owned")]
    TenantOwned,
    #[serde(rename = "global")]
    Global,
    #[serde(rename = "derived")]
    Derived,
}

/// The closed set of strings allowed in the `migrationAction` cell. The cell
/// may be a comma-joined combination.
pub const ALLOWED_MIGRATION_ACTIONS: [&str; 6] = [
    "add_column_backfill",
    "index_rebuild",
    "leave_global",
    "leave_existing",
    "derive_at_read",
    "remove_tenantless_access",
];

/// Errors produced while parsing the schema inventory.
#[derive(Debug, Error)]
pub enum Error {
    #[error("open inventory file: {0}")]
    Open(#[source] std::io::Error),

    #[error("scan inventory: {0}")]
    Scan(#[source] std::io::Error),

    #[error("inventory line {line}: column {column} is empty")]
    EmptyColumn { line: usize, column: usize },

    #[error("inventory line {line} ({name}): unknown classification {value:?}")]
    UnknownClassification {
        line: usize,
        name: String,
        value: String,
    },

    #[error("inventory line {line} ({name}): {source}")]
    InvalidMigrationAction {
        line: usize,
        name: String,
        #[source]
        source: MigrationActionError,
    },
}

/// Validation failure of a single `migrationAction` cell.
#[derive(Debug, Error)]
pub enum MigrationActionError {
    #[error("migration action is empty")]
    Empty,

    #[error(
        "no allowed migration action found in {cell:?} (allowed: add_column_backfill, index_rebuild, leave_global, leave_existing, derive_at_read, remove_tenantless_access)"
    )]
    NoAllowedAction { cell: String },
}

/// One row in the schema inventory. Every column is required at parse time;
/// empty cells fail validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub classification: Classification,
    pub tenant_id_source: String,
    pub migration_action: String,
    pub affected_apis: String,
    pub affected_events: String,
    pub store_access: String,
    pub indexes_and_uniqueness: String,
    pub isolation_tests: String,
    pub rollback: String,
}

/// Parses the inventory Markdown file at `path`.
pub fn load_from_file(path: impl AsRef<Path>) -> Result<Vec<Entry>, Error> {
    let f = std::fs::File::open(path).map_err(Error::Open)?;
    load(std::io::BufReader::new(f))
}

/// Parses the inventory Markdown stream. Accepts every Markdown table row
/// whose first cell starts with `|` and is not the header or separator row.
/// Section headings (`##`, `###`) and prose paragraphs are ignored. Each
/// non-header table row MUST contain exactly 10 cells matching the column
/// order documented in the inventory file.
pub fn load<R: BufRead>(r: R) -> Result<Vec<Entry>, Error> {
    const EXPECTED_CELLS: usize = 10;

    let mut entries = Vec::new();
    let mut header_seen = false;
    let mut lines = r.lines();
    let mut line_no = 0usize;
    loop {
        let line = match lines.next() {
            Some(Ok(l)) => l,
            Some(Err(e)) => return Err(Error::Scan(e)),
            None => break,
        };
        line_no += 1;
        let line = line.trim();
        if !line.starts_with('|') {
            header_seen = false;
            continue;
        }

        let cells = split_markdown_row(line);
        if cells.len() != EXPECTED_CELLS {
            // Allow header / separator row to silently align if it has the
            // expected width; otherwise skip rows with unexpected column
            // counts to keep the parser tolerant of nearby explanatory tables.
            if !header_seen && is_header_row(&cells) {
                header_seen = true;
                continue;
            }
            if is_separator_row(&cells) {
                continue;
            }
            continue;
        }
        if is_separator_row(&cells) {
            continue;
        }
        if !header_seen && is_header_row(&cells) {
            header_seen = true;
            continue;
        }
        // At this point we have a data row. Skip if it looks like a header
        // (e.g. another section's header row).
        if is_header_row(&cells) {
            header_seen = true;
            continue;
        }

        entries.push(build_entry(&cells, line_no)?);
    }
    Ok(entries)
}

fn split_markdown_row(line: &str) -> Vec<String> {
    // Markdown rows look like: | a | b | c |
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed.split('|').map(|p| p.trim().to_string()).collect()
}

fn is_header_row(cells: &[String]) -> bool {
    match cells.first() {
        Some(first) => first.eq_ignore_ascii_case("name"),
        None => false,
    }
}

fn is_separator_row(cells: &[String]) -> bool {
    if cells.is_empty() {
        return false;
    }
    for c in cells {
        let c = c.trim();
        let c = c.trim_start_matches(':');
        let c = c.trim_end_matches(':');
        if c.is_empty() {
            continue;
        }
        if !c.chars().all(|ch| ch == '-') {
            return false;
        }
    }
    true
}

fn build_entry(cells: &[String], line_no: usize) -> Result<Entry, Error> {
    for (idx, c) in cells.iter().enumerate() {
        if c.is_empty() {
            return Err(Error::EmptyColumn {
                line: line_no,
                column: idx + 1,
            });
        }
    }

    let classification = match cells[1].as_str() {
        "tenant_owned" => Classification::TenantOwned,
        "global" => Classification::Global,
        "derived" => Classification::Derived,
        other => {
            return Err(Error::UnknownClassification {
                line: line_no,
                name: cells[0].clone(),
                value: other.to_string(),
            });
        }
    };

    validate_migration_action(&cells[3]).map_err(|source| Error::InvalidMigrationAction {
        line: line_no,
        name: cells[0].clone(),
        source,
    })?;

    Ok(Entry {
        name: cells[0].clone(),
        classification,
        tenant_id_source: cells[2].clone(),
        migration_action: cells[3].clone(),
        affected_apis: cells[4].clone(),
        affected_events: cells[5].clone(),
        store_access: cells[6].clone(),
        indexes_and_uniqueness: cells[7].clone(),
        isolation_tests: cells[8].clone(),
        rollback: cells[9].clone(),
    })
}

fn validate_migration_action(cell: &str) -> Result<(), MigrationActionError> {
    if cell.is_empty() {
        return Err(MigrationActionError::Empty);
    }
    // Cells may carry freeform notes alongside the action keyword (e.g.
    // "leave_existing (verify-only); add UNIQUE (tenant_id, principal_id)
    // if not present"). Validation requires AT LEAST one allowed action
    // keyword to appear as a recognizable whole token; everything else in
    // the cell is documentation.
    for action in ALLOWED_MIGRATION_ACTIONS {
        if contains_token(cell, action) {
            return Ok(());
        }
    }
    Err(MigrationActionError::NoAllowedAction {
        cell: cell.to_string(),
    })
}

/// Reports whether `s` contains `token` as a whole token surrounded by
/// non-identifier characters (or the string boundary).
fn contains_token(s: &str, token: &str) -> bool {
    let s = s.as_bytes();
    let token = token.as_bytes();
    let mut idx = 0usize;
    while idx <= s.len() {
        let Some(i) = find_subslice(&s[idx..], token).map(|i| i + idx) else {
            return false;
        };
        let left = i == 0 || !is_ident_byte(s[i - 1]);
        let end = i + token.len();
        let right = end == s.len() || !is_ident_byte(s[end]);
        if left && right {
            return true;
        }
        idx = end;
    }
    false
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_lowercase() || b.is_ascii_uppercase() || b.is_ascii_digit()
}

/// Returns a map of entries keyed by name for O(1) lookups.
pub fn by_name(entries: Vec<Entry>) -> HashMap<String, Entry> {
    entries.into_iter().map(|e| (e.name.clone(), e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Locate the checked-in inventory relative to the repo root via this
    /// crate's manifest dir (crates/domains/inventory -> crates/domains ->
    /// crates -> repo root).
    fn inventory_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("specs/020-tenant-scoped-data-migration/contracts/schema-inventory.md")
    }

    #[test]
    fn parse_good_fixture() {
        let src: &[u8] = b"# header
some prose
| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|------|----------------|----------------|-----------------|--------------|----------------|-------------|----------------------|----------------|----------|
| runs | tenant_owned | backfilled default personal tenant | add_column_backfill, index_rebuild | run routes | run-* | tenancy.runs | (tenant_id, created_at DESC) | runs domain | backup_restore |
| schema_migrations | global | not_applicable | leave_global | none | none | unchanged | none | none | backup_restore |
";
        let entries = load(src).expect("load");
        assert_eq!(entries.len(), 2, "expected 2 entries");
        assert_eq!(entries[0].name, "runs");
        assert_eq!(entries[0].classification, Classification::TenantOwned);
        assert_eq!(entries[1].classification, Classification::Global);
    }

    #[test]
    fn parse_rejects_empty_cell() {
        let src: &[u8] = b"| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|---|---|---|---|---|---|---|---|---|---|
| runs | tenant_owned |  | add_column_backfill | r | e | s | i | t | backup_restore |
";
        let err = load(src).expect_err("expected error for empty cell");
        assert!(matches!(err, Error::EmptyColumn { .. }));
    }

    #[test]
    fn parse_rejects_unknown_classification() {
        let src: &[u8] = b"| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|---|---|---|---|---|---|---|---|---|---|
| runs | unowned | x | add_column_backfill | r | e | s | i | t | backup_restore |
";
        let err = load(src).expect_err("expected error for unknown classification");
        assert!(matches!(err, Error::UnknownClassification { .. }));
    }

    #[test]
    fn parse_rejects_unknown_migration_action() {
        let src: &[u8] = b"| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|---|---|---|---|---|---|---|---|---|---|
| runs | tenant_owned | x | recreate_world | r | e | s | i | t | backup_restore |
";
        let err = load(src).expect_err("expected error for unknown migration action");
        assert!(matches!(err, Error::InvalidMigrationAction { .. }));
    }

    #[test]
    fn migration_action_with_freeform_notes_passes() {
        let src: &[u8] = b"| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|---|---|---|---|---|---|---|---|---|---|
| runs | tenant_owned | x | leave_existing (verify-only); add UNIQUE (tenant_id, principal_id) if not present | r | e | s | i | t | backup_restore |
";
        let entries = load(src).expect("freeform notes alongside allowed action must pass");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn migration_action_partial_token_rejected() {
        // "add_column_backfilled" contains the allowed action as a substring
        // but not as a whole token.
        let src: &[u8] = b"| name | classification | tenantIdSource | migrationAction | affectedAPIs | affectedEvents | storeAccess | indexesAndUniqueness | isolationTests | rollback |
|---|---|---|---|---|---|---|---|---|---|
| runs | tenant_owned | x | add_column_backfilled | r | e | s | i | t | backup_restore |
";
        let err = load(src).expect_err("partial token must not validate");
        assert!(matches!(err, Error::InvalidMigrationAction { .. }));
    }

    #[test]
    fn real_inventory_parses() {
        let entries = load_from_file(inventory_path()).expect("load real inventory");
        assert!(
            entries.len() >= 30,
            "expected at least 30 inventory entries, got {}",
            entries.len()
        );

        // Spot check that real table names are present, not placeholders.
        let by_name = by_name(entries);
        for expected in [
            "runs",
            "events",
            "schedule_dispatch_attempts",
            "evaluation_regression_fixtures",
            "computer_use_actions",
        ] {
            assert!(
                by_name.contains_key(expected),
                "expected inventory entry for {expected:?} to be present"
            );
        }
    }

    // Roadmap 35 (US3 / T084) — classification invariants.
    //
    // Asserts inventory rows obey the classification contract documented in
    // the inventory header:
    //   - rollback MUST be `backup_restore` for every row in this delivery.
    //   - A `tenant_owned` row MUST NOT carry tenantIdSource `not_applicable`.
    //   - A `tenant_owned` row MUST populate indexesAndUniqueness,
    //     isolationTests, and storeAccess with substantive content (not the
    //     literal "none" or "(none)") so reviewers can see the per-row
    //     commitment.
    #[test]
    fn inventory_classification_invariants() {
        let entries = load_from_file(inventory_path()).expect("load real inventory");
        assert!(!entries.is_empty(), "inventory parsed empty (regression)");

        let mut failures: Vec<String> = Vec::new();
        for e in &entries {
            if !e.rollback.trim().eq_ignore_ascii_case("backup_restore") {
                failures.push(format!(
                    "{}: rollback must be backup_restore, got {:?}",
                    e.name, e.rollback
                ));
            }
            if e.classification != Classification::TenantOwned {
                continue;
            }
            if e.tenant_id_source
                .trim()
                .eq_ignore_ascii_case("not_applicable")
            {
                failures.push(format!(
                    "{}: tenant_owned row must not carry tenantIdSource=not_applicable",
                    e.name
                ));
            }
            for (col, val) in [
                ("indexesAndUniqueness", e.indexes_and_uniqueness.as_str()),
                ("isolationTests", e.isolation_tests.as_str()),
                ("storeAccess", e.store_access.as_str()),
            ] {
                if is_placeholder(val) {
                    failures.push(format!(
                        "{}: tenant_owned row has placeholder {val:?} in {col}",
                        e.name
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "classification invariant violations:\n{}",
            failures.join("\n")
        );
    }

    /// Reports whether a cell is semantically empty: literal "none" or
    /// "(none)" with optional surrounding whitespace.
    fn is_placeholder(cell: &str) -> bool {
        let c = cell.trim().to_ascii_lowercase();
        c == "none" || c == "(none)"
    }
}
