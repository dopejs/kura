//! Shared helpers for the migrationfixture integration tests.

#![allow(dead_code)]

use rusqlite::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn temp_dir(name: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "kura_migrationfixture_{name}_{}_{}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

pub fn open_conn(db_path: &str) -> Connection {
    Connection::open(db_path).unwrap()
}

pub fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// Builds the pre-tenant v21 fixture and applies the head migrations, returning
/// the store and its data dir.
pub fn head_store(name: &str) -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir(name);
    let store = kura_migrationfixture::build_pre_tenant_v21_fixture(&dir).unwrap();
    kura_migrationfixture::apply_head_migrations(&store).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );
    (store, dir)
}

/// Builds the full fixture (pre-tenant v21 + head + all roadmap seeds).
pub fn full_fixture(name: &str) -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir(name);
    let output = kura_migrationfixture::FixtureBuilder::new()
        .build(&dir)
        .unwrap();
    (output.store, dir)
}
