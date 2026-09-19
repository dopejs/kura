//! Upgrade rehearsal (Stage 7.2): run the candidate binary against an
//! isolated snapshot of the data directory **before** activation.
//!
//! The runbook's rollback rule is "old binaries must not be pointed at newer
//! schema versions", and migrations are forward-only. So the moment the new
//! binary opens the real database, the upgrade has happened and the only way
//! back is a backup. A rehearsal moves the risky step — migrating, loading the
//! plugin profile, building every manager — onto a throwaway copy, so the
//! operator learns whether the upgrade *would* work while the real state is
//! still exactly what the previous binary left.
//!
//! What "isolated candidate state" is here:
//!
//! - `daemon.sqlite` via `VACUUM INTO` — a consistent snapshot that reads
//!   through the WAL (see D9: `cp` does not);
//! - `plugins.json` and `plugins/` — the plugin profile and external plugin
//!   manifests, so a malformed profile fails the rehearsal rather than the
//!   real boot;
//! - `tenant-secret-values/` — the local secret backend, because `App::new`
//!   opens it. The scratch directory is created mode 0700 and removed unless
//!   the caller keeps it.
//!
//! What activation and rollback mean afterwards is written into the report,
//! because the answer changes at exactly one moment: before the real daemon
//! starts on the new binary, rollback is "do nothing"; after, it is "restore
//! from a snapshot taken with `.backup`/`VACUUM INTO`".

use std::path::{Path, PathBuf};

use kura_config::Config;
use serde::Serialize;

use crate::App;
use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RehearsalReport {
    pub scratch_dir: String,
    pub kept: bool,
    pub source_schema_version: i64,
    pub candidate_schema_version: i64,
    pub target_schema_version: i64,
    /// True when the rehearsal applied migrations to the snapshot.
    pub migrated: bool,
    pub table_count: i64,
    pub integrity: Vec<String>,
    pub plugin_warnings: Vec<String>,
    pub disabled_plugins: Vec<DisabledPlugin>,
    pub findings: Vec<String>,
    pub passed: bool,
    pub activation: String,
    pub rollback: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisabledPlugin {
    pub id: String,
    pub reason: String,
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), AppError> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to)
        .map_err(|e| AppError::Rehearsal(format!("create {}: {e}", to.display())))?;
    for entry in std::fs::read_dir(from)
        .map_err(|e| AppError::Rehearsal(format!("read {}: {e}", from.display())))?
    {
        let entry = entry.map_err(|e| AppError::Rehearsal(e.to_string()))?;
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| AppError::Rehearsal(e.to_string()))?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|e| {
                AppError::Rehearsal(format!("copy {}: {e}", entry.path().display()))
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(dir: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| AppError::Rehearsal(format!("chmod {}: {e}", dir.display())))
}

#[cfg(not(unix))]
fn restrict_permissions(_dir: &Path) -> Result<(), AppError> {
    Ok(())
}

/// Rehearses starting this binary against a snapshot of `config.data_dir`.
///
/// Never migrates, writes to, or otherwise touches the real data directory:
/// the source database is opened with `open_unmigrated` and read only for its
/// version and its snapshot.
pub fn rehearse_upgrade(config: &Config, keep: bool) -> Result<RehearsalReport, AppError> {
    let source_dir = Path::new(&config.data_dir);
    let source =
        kura_store::SQLiteStore::open_unmigrated(&config.data_dir).map_err(AppError::Rehearsal)?;
    let source_schema_version = source.schema_version().map_err(AppError::Rehearsal)?;

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let scratch: PathBuf = source_dir
        .join("rehearsals")
        .join(format!("{stamp}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| AppError::Rehearsal(format!("create {}: {e}", scratch.display())))?;
    restrict_permissions(&scratch)?;

    // Consistent snapshot through the WAL. `cp` would not be (D9).
    source
        .snapshot_to(&scratch.join(kura_store::DEFAULT_DATABASE_FILE))
        .map_err(AppError::Rehearsal)?;
    drop(source);

    for name in [
        kura_plugin::PROFILE_FILE_NAME,
        kura_config::DEFAULT_CONFIG_FILE_NAME,
    ] {
        let from = source_dir.join(name);
        if from.exists() {
            std::fs::copy(&from, scratch.join(name))
                .map_err(|e| AppError::Rehearsal(format!("copy {}: {e}", from.display())))?;
        }
    }
    copy_tree(&source_dir.join("plugins"), &scratch.join("plugins"))?;
    copy_tree(
        &source_dir.join("tenant-secret-values"),
        &scratch.join("tenant-secret-values"),
    )?;

    let mut candidate = config.clone();
    candidate.data_dir = scratch.to_string_lossy().into_owned();

    let mut findings = Vec::new();
    let mut plugin_warnings = Vec::new();
    let mut disabled_plugins = Vec::new();
    let mut candidate_schema_version = source_schema_version;
    let mut table_count = 0;
    let mut integrity = Vec::new();

    // The candidate boot: migrations, plugin profile, every manager — exactly
    // what the real start would do, against the copy.
    match App::new(candidate) {
        Ok(app) => {
            {
                let store = app.state.store.lock();
                candidate_schema_version = store.schema_version().unwrap_or(-1);
                table_count = store.table_count().unwrap_or(0);
                integrity = store.integrity_check().unwrap_or_default();
            }
            if let Some(report) = app.state.plugins.as_ref() {
                plugin_warnings = report.warnings.clone();
                disabled_plugins = report
                    .plugins
                    .iter()
                    .filter(|p| !p.enabled)
                    .map(|p| DisabledPlugin {
                        id: p.id.clone(),
                        reason: p.reason.clone().unwrap_or_default(),
                    })
                    .collect();
            }
            app.close();
        }
        Err(err) => findings.push(format!("candidate boot failed: {err}")),
    }

    let target_schema_version = kura_store::CURRENT_SCHEMA_VERSION;
    if candidate_schema_version != target_schema_version {
        findings.push(format!(
            "candidate schema version is {candidate_schema_version}, expected {target_schema_version}"
        ));
    }
    if integrity != ["ok"] {
        findings.push(format!("integrity check: {}", integrity.join("; ")));
    }
    // An intact, empty database is what a lost snapshot looks like (D9).
    if table_count == 0 {
        findings.push("snapshot has no tables".to_string());
    }
    // The source must be exactly as found: a rehearsal that migrated the real
    // database is an upgrade, not a rehearsal.
    match kura_store::SQLiteStore::open_unmigrated(&config.data_dir)
        .and_then(|s| s.schema_version())
    {
        Ok(v) if v == source_schema_version => {}
        Ok(v) => findings.push(format!(
            "source schema version changed during rehearsal ({source_schema_version} -> {v})"
        )),
        Err(e) => findings.push(format!("re-reading source: {e}")),
    }

    let passed = findings.is_empty();
    let kept = keep || !passed;
    if !kept {
        let _ = std::fs::remove_dir_all(&scratch);
    }

    Ok(RehearsalReport {
        scratch_dir: scratch.to_string_lossy().into_owned(),
        kept,
        source_schema_version,
        candidate_schema_version,
        target_schema_version,
        migrated: candidate_schema_version != source_schema_version,
        table_count,
        integrity,
        plugin_warnings,
        disabled_plugins,
        findings,
        passed,
        activation: if passed {
            "start the daemon on this binary; it will apply the same migrations to the real database".to_string()
        } else {
            "do not start the daemon on this binary until the findings are resolved".to_string()
        },
        rollback: "before activation: nothing to roll back (the real data directory was not touched). \
                   After activation: stop the daemon and restore a snapshot taken with `.backup` or \
                   `VACUUM INTO` — `cp daemon.sqlite` is not a backup in WAL mode."
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_config::Environment;

    fn source_at_version(version: i64) -> Config {
        let dir = std::env::temp_dir().join(format!("kura-rehearse-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = kura_store::SQLiteStore::new_at_version(dir.to_str().expect("path"), version)
            .expect("store at version");
        assert_eq!(store.schema_version().expect("v"), version);
        drop(store);
        Config {
            store: Default::default(),
            environment: Environment::Test,
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.to_string_lossy().into_owned(),
            log_level: "info".to_string(),
            version: "dev".to_string(),
            llm: Default::default(),
            connectors: Default::default(),
            egress: Default::default(),
        }
    }

    /// The whole point: the candidate migrates the snapshot to head while the
    /// source stays at the version the previous binary left it.
    #[test]
    fn rehearsal_migrates_the_snapshot_and_leaves_the_source_untouched() {
        let previous = kura_store::CURRENT_SCHEMA_VERSION - 1;
        let config = source_at_version(previous);

        let report = rehearse_upgrade(&config, false).expect("rehearse");
        assert!(report.passed, "{report:?}");
        assert_eq!(report.source_schema_version, previous);
        assert_eq!(
            report.candidate_schema_version,
            kura_store::CURRENT_SCHEMA_VERSION
        );
        assert!(report.migrated);
        assert_eq!(report.integrity, ["ok"]);
        assert!(report.table_count > 0);
        assert!(!report.kept, "a passing rehearsal cleans up");
        assert!(!Path::new(&report.scratch_dir).exists());

        let source = kura_store::SQLiteStore::open_unmigrated(&config.data_dir).expect("open");
        assert_eq!(
            source.schema_version().expect("v"),
            previous,
            "the real database was not migrated"
        );
    }

    /// A malformed plugin profile is exactly the failure a rehearsal exists to
    /// catch before it fails the real boot. The scratch dir is kept for
    /// inspection.
    #[test]
    fn a_broken_plugin_profile_fails_the_rehearsal_not_the_real_boot() {
        let config = source_at_version(kura_store::CURRENT_SCHEMA_VERSION);
        std::fs::write(
            Path::new(&config.data_dir).join(kura_plugin::PROFILE_FILE_NAME),
            "{ this is not json",
        )
        .expect("write");

        let report = rehearse_upgrade(&config, false).expect("rehearse runs");
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.contains("candidate boot failed")),
            "{report:?}"
        );
        assert!(
            report.kept,
            "a failed rehearsal keeps the scratch dir for inspection"
        );
        assert!(report.activation.contains("do not start"));
    }
}
