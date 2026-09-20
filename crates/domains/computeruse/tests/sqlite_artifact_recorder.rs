//! SQLite-backed artifact recorder tests (wave 8 parity): records persist
//! through the `kura-store` computeruse DAOs and content round-trips through
//! the artifacts directory.

use std::sync::Arc;

use kura_computeruse::{
    Action, ActionKind, ActionStatus, ArtifactCaptureRequest, ArtifactKind, ArtifactRecorder,
    ArtifactStatus, CreateActionInput, CreateSessionInput, Dependencies, Manager, RiskLevel,
    Session, SessionStatus, SqliteArtifactRecorder, Store,
};
use kura_runtime::{CreateRunInput, RunStatus};
use kura_store::{ComputerUseStoreHandle, SQLiteStore};

fn seed_run(handle: &ComputerUseStoreHandle, run_id: &str) {
    let conn = rusqlite::Connection::open(handle.0.lock().db_path()).expect("open connection");
    conn.execute(
        "INSERT INTO runs (run_id, session_id, entrypoint, status, goal, created_at, updated_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![
            run_id,
            "browse",
            "completed",
            "browse the web",
            chrono::Utc::now().to_rfc3339(),
        ],
    )
    .expect("seed run row");
}

fn seed_session_and_action(
    handle: &ComputerUseStoreHandle,
    run_id: &str,
    session_id: &str,
    action_id: &str,
) {
    let now = chrono::Utc::now();
    handle
        .upsert_computer_use_session(&Session {
            computer_use_session_id: session_id.to_string(),
            environment_scope: "test".to_string(),
            run_id: run_id.to_string(),
            status: SessionStatus::Active,
            driver_kind: "browser".to_string(),
            started_at: now,
            updated_at: now,
            ..Session::default()
        })
        .expect("upsert session");
    handle
        .upsert_computer_use_action(&Action {
            computer_use_action_id: action_id.to_string(),
            environment_scope: "test".to_string(),
            computer_use_session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            action_kind: ActionKind::Snapshot,
            status: ActionStatus::Completed,
            risk_level: RiskLevel::Low,
            requested_at: now,
            updated_at: now,
            ..Action::default()
        })
        .expect("upsert action");
}

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "kura_computeruse_artifacts_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn capture(
    session_id: &str,
    action_id: &str,
    kind: ArtifactKind,
    content: &[u8],
) -> ArtifactCaptureRequest {
    ArtifactCaptureRequest {
        run_id: "r1".to_string(),
        computer_use_session_id: session_id.to_string(),
        computer_use_action_id: action_id.to_string(),
        kind,
        mime_type: "text/plain".to_string(),
        file_name: "shot.txt".to_string(),
        content: content.to_vec(),
        ..ArtifactCaptureRequest::default()
    }
}

#[test]
fn recorder_persists_record_and_content() {
    let dir = temp_dir("persist");
    let store = Arc::new(ComputerUseStoreHandle::new(
        SQLiteStore::new(&dir).expect("open store"),
    ));
    seed_run(&store, "r1");
    seed_session_and_action(&store, "r1", "s1", "a1");
    let recorder = SqliteArtifactRecorder::new(
        store.clone() as Arc<dyn kura_computeruse::Store>,
        &dir,
        "test",
    );

    let artifact = recorder
        .save_computer_use_artifact(capture(
            "s1",
            "a1",
            ArtifactKind::Screenshot,
            b"screenshot bytes",
        ))
        .expect("save artifact");

    assert!(artifact.artifact_id.starts_with("cuart_"));
    assert_eq!(artifact.status, ArtifactStatus::Available);
    assert_eq!(artifact.byte_size, 16);
    assert_eq!(artifact.sha256.len(), 64);
    assert_eq!(artifact.kind, ArtifactKind::Screenshot);

    // The artifact record is durable through the SQLite computeruse DAO.
    let persisted = store
        .get_computer_use_artifact("test", &artifact.artifact_id)
        .expect("get artifact")
        .expect("artifact persisted");
    assert_eq!(persisted.artifact_id, artifact.artifact_id);
    assert_eq!(persisted.run_id, "r1");
    assert_eq!(persisted.computer_use_session_id, "s1");
    assert_eq!(persisted.computer_use_action_id, "a1");

    // Content round-trips through the artifacts directory by storage key.
    let content = recorder
        .read_computer_use_artifact_content(&artifact.storage_key)
        .expect("read content");
    assert_eq!(content, b"screenshot bytes".to_vec());
}

#[test]
fn recorder_artifact_ids_are_deterministic() {
    let dir = temp_dir("deterministic");
    let store = Arc::new(ComputerUseStoreHandle::new(
        SQLiteStore::new(&dir).expect("open store"),
    ));
    seed_run(&store, "r1");
    seed_session_and_action(&store, "r1", "s1", "a1");
    seed_session_and_action(&store, "r1", "s2", "a2");
    let recorder = SqliteArtifactRecorder::new(
        store.clone() as Arc<dyn kura_computeruse::Store>,
        &dir,
        "test",
    );

    let a = recorder
        .save_computer_use_artifact(capture("s1", "a1", ArtifactKind::PageSnapshot, b"same"))
        .expect("save a");
    let b = recorder
        .save_computer_use_artifact(capture("s2", "a2", ArtifactKind::PageSnapshot, b"same"))
        .expect("save b");
    assert_eq!(
        a.artifact_id, b.artifact_id,
        "content-addressed artifact ids must match"
    );

    // Same content addresses one artifact row; the second save upserts it and
    // points it at the latest action.
    let listed = store
        .list_computer_use_artifacts_for_action("test", "r1", "a2")
        .expect("list artifacts");
    assert_eq!(listed.len(), 1, "same content addresses one artifact row");
    assert_eq!(listed[0].computer_use_action_id, "a2");
    assert_eq!(
        store
            .get_computer_use_artifact("test", &a.artifact_id)
            .expect("get artifact")
            .expect("present")
            .artifact_id,
        a.artifact_id
    );
}

#[test]
fn recorder_reads_missing_content_as_error() {
    let dir = temp_dir("missing");
    let store = Arc::new(ComputerUseStoreHandle::new(
        SQLiteStore::new(&dir).expect("open store"),
    ));
    let recorder = SqliteArtifactRecorder::new(store, &dir, "test");
    let err = recorder
        .read_computer_use_artifact_content("computer-use/s1/cuart_nope")
        .expect_err("missing content must error");
    assert!(err.contains("read artifact content"), "{err}");
}

#[test]
fn recorder_without_data_dir_returns_empty_content() {
    let dir = temp_dir("nodir");
    let store = Arc::new(ComputerUseStoreHandle::new(
        SQLiteStore::new(&dir).expect("open store"),
    ));
    let recorder = SqliteArtifactRecorder::new(store, "", "test");
    assert_eq!(
        recorder
            .read_computer_use_artifact_content("computer-use/s1/cuart_abc")
            .expect("no data dir reads empty"),
        Vec::<u8>::new()
    );
}

#[test]
fn manager_records_artifacts_through_the_seam() {
    let runtime = Arc::new(kura_runtime::Manager::new());
    let run = runtime
        .create_run(CreateRunInput {
            entrypoint: "browse".to_string(),
            ..CreateRunInput::default()
        })
        .unwrap();
    assert_eq!(run.status, RunStatus::Queued);

    let dir = temp_dir("manager");
    let recorder = SqliteArtifactRecorder::new(Arc::new(MemStore::default()), &dir, "test");

    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        runtime: Some(runtime.clone()),
        policy: None,
        store: Arc::new(MemStore::default()),
        driver: None,
        artifacts: Some(Arc::new(recorder)),
    });
    let session = manager
        .create_session(
            &run.run_id,
            &CreateSessionInput {
                initial_url: "https://example.com".to_string(),
                ..CreateSessionInput::default()
            },
        )
        .expect("create session");

    let (result, _approval, _decision) = manager
        .create_action(
            &run.run_id,
            &session.computer_use_session_id,
            "tester",
            CreateActionInput {
                action_kind: ActionKind::Snapshot,
                ..CreateActionInput::default()
            },
        )
        .expect("create action");

    assert_eq!(result.action.status, ActionStatus::Completed);
    assert_eq!(
        result.action.artifacts.len(),
        1,
        "snapshot capture must be recorded"
    );
    assert_eq!(result.action.artifacts[0].kind, ArtifactKind::PageSnapshot);
    assert_eq!(result.action.artifacts[0].status, ArtifactStatus::Available);
    assert_eq!(result.action.artifacts[0].environment_scope, "test");
}

/// In-memory store mirroring tests/manager.rs.
#[derive(Default)]
struct MemStore {
    sessions: std::sync::Mutex<std::collections::HashMap<String, kura_computeruse::Session>>,
    actions: std::sync::Mutex<std::collections::HashMap<String, Action>>,
    artifacts: std::sync::Mutex<std::collections::HashMap<String, kura_computeruse::Artifact>>,
}

impl kura_computeruse::Store for MemStore {
    fn upsert_computer_use_session(
        &self,
        session: &kura_computeruse::Session,
    ) -> Result<(), String> {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.computer_use_session_id.clone(), session.clone());
        Ok(())
    }
    fn list_computer_use_sessions(
        &self,
        _env: &str,
        run_id: &str,
    ) -> Result<Vec<kura_computeruse::Session>, String> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.run_id == run_id)
            .cloned()
            .collect())
    }
    fn get_computer_use_session(
        &self,
        _env: &str,
        _run_id: &str,
        session_id: &str,
    ) -> Result<Option<kura_computeruse::Session>, String> {
        Ok(self.sessions.lock().unwrap().get(session_id).cloned())
    }
    fn upsert_computer_use_action(&self, action: &Action) -> Result<(), String> {
        self.actions
            .lock()
            .unwrap()
            .insert(action.computer_use_action_id.clone(), action.clone());
        Ok(())
    }
    fn list_computer_use_actions(
        &self,
        _env: &str,
        _run_id: &str,
        session_id: &str,
    ) -> Result<Vec<Action>, String> {
        Ok(self
            .actions
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.computer_use_session_id == session_id)
            .cloned()
            .collect())
    }
    fn get_computer_use_action(
        &self,
        _env: &str,
        _run_id: &str,
        _session_id: &str,
        action_id: &str,
    ) -> Result<Option<Action>, String> {
        Ok(self.actions.lock().unwrap().get(action_id).cloned())
    }
    fn find_pending_computer_use_action_by_approval(
        &self,
        _env: &str,
        _approval_id: &str,
    ) -> Result<Option<Action>, String> {
        Ok(None)
    }
    fn upsert_computer_use_artifact(
        &self,
        artifact: &kura_computeruse::Artifact,
    ) -> Result<(), String> {
        self.artifacts
            .lock()
            .unwrap()
            .insert(artifact.artifact_id.clone(), artifact.clone());
        Ok(())
    }
    fn list_computer_use_artifacts_for_action(
        &self,
        _env: &str,
        _run_id: &str,
        action_id: &str,
    ) -> Result<Vec<kura_computeruse::Artifact>, String> {
        Ok(self
            .artifacts
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.computer_use_action_id == action_id)
            .cloned()
            .collect())
    }
    fn get_computer_use_artifact(
        &self,
        _env: &str,
        artifact_id: &str,
    ) -> Result<Option<kura_computeruse::Artifact>, String> {
        Ok(self.artifacts.lock().unwrap().get(artifact_id).cloned())
    }
    fn mark_in_flight_computer_use_interrupted(
        &self,
        _env: &str,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(Vec<kura_computeruse::Session>, Vec<Action>), String> {
        Ok((Vec::new(), Vec::new()))
    }
}
