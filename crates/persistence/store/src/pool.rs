//! Stage 10.2 (D4): the connection pool.
//!
//! Before this, every database access in the API serialised through one
//! `Mutex<SQLiteStore>` over one connection, so `PRAGMA journal_mode = WAL`
//! bought nothing: WAL lets readers run concurrently with one writer, but
//! there was only ever one connection to run anything.
//!
//! The pool keeps that one connection as **the writer** (every mutation
//! still goes through it, so write ordering is unchanged) and adds N
//! **reader** connections opened with `PRAGMA query_only = ON`. A reader
//! cannot write even if handed a mutating method — SQLite refuses — so
//! misrouting a write to `read()` fails loudly instead of racing the writer.
//!
//! `readers = 0` is the pre-pool behaviour exactly: `read()` hands out the
//! writer. That is the rollback posture recorded in the program: the pool
//! ships dark and is enabled by `store.readers` in the config.
//!
//! Lock wait time is measured on both roles and exposed through
//! [`StorePool::stats`] (and the `kura_store_lock_wait_seconds` metric),
//! which is how the pool's effect is observed rather than assumed.

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use parking_lot::{Mutex, MutexGuard};

use crate::SQLiteStore;

/// Aggregate wait accounting for one role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitStats {
    pub acquisitions: u64,
    pub total_wait_ns: u64,
    pub max_wait_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolStats {
    pub readers: usize,
    pub writer: WaitStats,
    pub reader: WaitStats,
    /// Reads served by a reader connection (pool enabled) vs. by the
    /// writer (pool disabled, or all readers busy is *not* counted here:
    /// a busy reader is still waited for, never bypassed).
    pub reads_via_readers: u64,
    pub reads_via_writer: u64,
}

#[derive(Default)]
struct Counters {
    acquisitions: AtomicU64,
    total_wait_ns: AtomicU64,
    max_wait_ns: AtomicU64,
}

impl Counters {
    fn record(&self, role: &'static str, waited: std::time::Duration) {
        kura_telemetry::metrics::registry().observe(
            kura_telemetry::metrics::STORE_LOCK_WAIT_SECONDS,
            &[("role", role)],
            waited.as_secs_f64(),
        );
        let ns = waited.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.total_wait_ns.fetch_add(ns, Ordering::Relaxed);
        self.max_wait_ns.fetch_max(ns, Ordering::Relaxed);
    }
    fn snapshot(&self) -> WaitStats {
        WaitStats {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            total_wait_ns: self.total_wait_ns.load(Ordering::Relaxed),
            max_wait_ns: self.max_wait_ns.load(Ordering::Relaxed),
        }
    }
}

/// The pool. Clone the `Arc` freely; all handles share the same connections.
pub struct StorePool {
    writer: Arc<Mutex<SQLiteStore>>,
    readers: Vec<Mutex<SQLiteStore>>,
    cursor: AtomicUsize,
    writer_waits: Counters,
    reader_waits: Counters,
    reads_via_readers: AtomicU64,
    reads_via_writer: AtomicU64,
}

/// A borrowed store for reading. Derefs to [`SQLiteStore`]; the underlying
/// connection is either a query-only reader or (pool disabled) the writer.
pub struct ReadGuard<'a> {
    guard: MutexGuard<'a, SQLiteStore>,
}

impl Deref for ReadGuard<'_> {
    type Target = SQLiteStore;
    fn deref(&self) -> &SQLiteStore {
        &self.guard
    }
}

impl StorePool {
    /// Builds a pool around an already-open writer. `readers` extra
    /// query-only connections are opened against the same database.
    pub fn new(writer: Arc<Mutex<SQLiteStore>>, readers: usize) -> Result<Self, String> {
        let data_dir = writer.lock().data_dir().to_string();
        let mut pool = Vec::with_capacity(readers);
        for _ in 0..readers {
            pool.push(Mutex::new(SQLiteStore::open_reader(&data_dir)?));
        }
        Ok(StorePool {
            writer,
            readers: pool,
            cursor: AtomicUsize::new(0),
            writer_waits: Counters::default(),
            reader_waits: Counters::default(),
            reads_via_readers: AtomicU64::new(0),
            reads_via_writer: AtomicU64::new(0),
        })
    }

    /// The pre-pool shape: the writer alone.
    #[must_use]
    pub fn writer_only(writer: Arc<Mutex<SQLiteStore>>) -> Self {
        Self::new(writer, 0).expect("zero readers cannot fail to open")
    }

    #[must_use]
    pub fn reader_count(&self) -> usize {
        self.readers.len()
    }

    /// The writer connection. Every mutation goes through here.
    pub fn write(&self) -> MutexGuard<'_, SQLiteStore> {
        let started = Instant::now();
        let guard = self.writer.lock();
        self.writer_waits.record("writer", started.elapsed());
        guard
    }

    /// A connection for reading. Prefers an idle reader; when every reader
    /// is busy, waits on the next one round-robin rather than falling back
    /// to the writer (which would re-serialise reads behind writes).
    pub fn read(&self) -> ReadGuard<'_> {
        if self.readers.is_empty() {
            self.reads_via_writer.fetch_add(1, Ordering::Relaxed);
            let started = Instant::now();
            let guard = self.writer.lock();
            self.writer_waits.record("writer", started.elapsed());
            return ReadGuard { guard };
        }
        self.reads_via_readers.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        for offset in 0..self.readers.len() {
            let idx = (start + offset) % self.readers.len();
            if let Some(guard) = self.readers[idx].try_lock() {
                self.reader_waits.record("reader", started.elapsed());
                return ReadGuard { guard };
            }
        }
        let guard = self.readers[start].lock();
        self.reader_waits.record("reader", started.elapsed());
        ReadGuard { guard }
    }

    #[must_use]
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            readers: self.readers.len(),
            writer: self.writer_waits.snapshot(),
            reader: self.reader_waits.snapshot(),
            reads_via_readers: self.reads_via_readers.load(Ordering::Relaxed),
            reads_via_writer: self.reads_via_writer.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!("kura-pool-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn readers_see_the_writers_commits_and_cannot_write() {
        let dir = temp_dir("rw");
        let writer = Arc::new(Mutex::new(SQLiteStore::new(&dir).unwrap()));
        let pool = StorePool::new(writer.clone(), 2).unwrap();
        assert_eq!(pool.reader_count(), 2);

        let event = kura_events::Event {
            category: "pool".to_string(),
            name: "pool.test".to_string(),
            ..kura_events::Event::default()
        };
        let stored = pool.write().append_event(&event).unwrap();
        let seen = pool
            .read()
            .list_events(&kura_events::Filter::default())
            .unwrap();
        assert!(
            seen.iter().any(|e| e.event_id == stored.event_id),
            "reader sees committed write"
        );

        // A reader connection refuses mutation at the engine level.
        let err = pool
            .read()
            .append_event(&event)
            .expect_err("query_only reader");
        assert!(
            err.contains("readonly") || err.contains("read-only") || err.contains("query_only"),
            "{err}"
        );

        let stats = pool.stats();
        assert_eq!(stats.reads_via_readers, 2);
        assert_eq!(stats.reads_via_writer, 0);
        assert_eq!(stats.writer.acquisitions, 1);
        assert_eq!(stats.reader.acquisitions, 2);
    }

    #[test]
    fn zero_readers_is_the_pre_pool_behaviour() {
        let dir = temp_dir("zero");
        let writer = Arc::new(Mutex::new(SQLiteStore::new(&dir).unwrap()));
        let pool = StorePool::writer_only(writer);
        let _ = pool
            .read()
            .list_events(&kura_events::Filter::default())
            .unwrap();
        let stats = pool.stats();
        assert_eq!(stats.readers, 0);
        assert_eq!(stats.reads_via_writer, 1);
        assert_eq!(stats.reads_via_readers, 0);
    }

    #[test]
    fn concurrent_readers_do_not_wait_on_the_writer() {
        let dir = temp_dir("conc");
        let writer = Arc::new(Mutex::new(SQLiteStore::new(&dir).unwrap()));
        let pool = Arc::new(StorePool::new(writer.clone(), 2).unwrap());
        // Hold the writer for a while; reads must still complete promptly.
        let held = pool.clone();
        let hold = std::thread::spawn(move || {
            let _w = held.write();
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let started = Instant::now();
        let _ = pool
            .read()
            .list_events(&kura_events::Filter::default())
            .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "read did not queue behind the writer"
        );
        hold.join().unwrap();
    }
}
