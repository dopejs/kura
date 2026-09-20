use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use kura_computeruse::{
    Action, Artifact, CreateSessionInput, Dependencies, Manager, Session, SessionStatus, Store,
};
use kura_runtime::{CreateRunInput, RunStatus};

#[derive(Default)]
struct MemStore {
    sessions: Mutex<HashMap<String, Session>>,
    actions: Mutex<HashMap<String, Action>>,
    artifacts: Mutex<HashMap<String, Artifact>>,
}

impl Store for MemStore {
    fn upsert_computer_use_session(&self, session: &Session) -> Result<(), String> {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.computer_use_session_id.clone(), session.clone());
        Ok(())
    }
    fn list_computer_use_sessions(&self, _env: &str, run_id: &str) -> Result<Vec<Session>, String> {
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
    ) -> Result<Option<Session>, String> {
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
    fn upsert_computer_use_artifact(&self, artifact: &Artifact) -> Result<(), String> {
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
    ) -> Result<Vec<Artifact>, String> {
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
    ) -> Result<Option<Artifact>, String> {
        Ok(self.artifacts.lock().unwrap().get(artifact_id).cloned())
    }
    fn mark_in_flight_computer_use_interrupted(
        &self,
        _env: &str,
        _now: DateTime<Utc>,
    ) -> Result<(Vec<Session>, Vec<Action>), String> {
        Ok((Vec::new(), Vec::new()))
    }
}

#[test]
fn create_and_close_session() {
    let runtime = std::sync::Arc::new(kura_runtime::Manager::new());
    let run = runtime
        .create_run(CreateRunInput {
            entrypoint: "browse".to_string(),
            ..CreateRunInput::default()
        })
        .unwrap();
    assert_eq!(run.status, RunStatus::Queued);

    let store = std::sync::Arc::new(MemStore::default());
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        runtime: Some(runtime.clone()),
        policy: None,
        store,
        driver: None,
        artifacts: None,
    });

    let session = manager
        .create_session(
            &run.run_id,
            &CreateSessionInput {
                initial_url: "https://example.com".to_string(),
                ..CreateSessionInput::default()
            },
        )
        .unwrap();
    assert_eq!(session.status, SessionStatus::Active);
    assert!(session.current_page.is_some());

    let closed = manager
        .close_session(&run.run_id, &session.computer_use_session_id)
        .unwrap();
    assert_eq!(closed.status, SessionStatus::Closed);
}
