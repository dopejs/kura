use kura_computeruse::{
    Action, ActionKind, ActionStatus, Artifact, ArtifactKind, CreateSessionInput, Driver,
    MemoryDriver, Session, SessionStatus,
};

#[test]
fn memory_driver_start_session() {
    let driver = MemoryDriver::new();
    let session = Session {
        computer_use_session_id: "s1".to_string(),
        run_id: "r1".to_string(),
        ..Session::default()
    };
    let started = driver
        .start_session(
            session,
            CreateSessionInput {
                initial_url: "https://example.com".to_string(),
                ..CreateSessionInput::default()
            },
        )
        .unwrap();
    assert_eq!(started.status, SessionStatus::Active);
    assert_eq!(started.driver_kind, "browser");
    let page = started.current_page.expect("current page");
    assert_eq!(page.url, "https://example.com");
    assert_eq!(page.title, "example.com");
}

#[test]
fn memory_driver_navigate_completes() {
    let driver = MemoryDriver::new();
    let session = Session {
        computer_use_session_id: "s1".to_string(),
        run_id: "r1".to_string(),
        ..Session::default()
    };
    let started = driver
        .start_session(session, CreateSessionInput::default())
        .unwrap();
    let mut input = serde_json::Map::new();
    input.insert(
        "url".to_string(),
        serde_json::Value::String("https://foo.com".to_string()),
    );
    let action = Action {
        computer_use_action_id: "a1".to_string(),
        computer_use_session_id: "s1".to_string(),
        run_id: "r1".to_string(),
        action_kind: ActionKind::Navigate,
        status: ActionStatus::Requested,
        input,
        ..Action::default()
    };
    let (_s, executed, captures) = driver.execute_action(started, action).unwrap();
    assert_eq!(executed.status, ActionStatus::Completed);
    assert_eq!(executed.page_after.unwrap().url, "https://foo.com");
    assert!(captures.is_empty());
}

#[test]
fn snapshot_produces_capture() {
    let driver = MemoryDriver::new();
    let session = Session {
        computer_use_session_id: "s1".to_string(),
        run_id: "r1".to_string(),
        ..Session::default()
    };
    let started = driver
        .start_session(session, CreateSessionInput::default())
        .unwrap();
    let action = Action {
        computer_use_action_id: "a1".to_string(),
        computer_use_session_id: "s1".to_string(),
        run_id: "r1".to_string(),
        action_kind: ActionKind::Snapshot,
        status: ActionStatus::Requested,
        ..Action::default()
    };
    let (_s, _executed, captures) = driver.execute_action(started, action).unwrap();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].kind, ArtifactKind::PageSnapshot);
}

#[test]
fn artifact_roundtrips_camel_case() {
    let artifact = Artifact {
        artifact_id: "a1".to_string(),
        run_id: "r1".to_string(),
        kind: ArtifactKind::Screenshot,
        ..Artifact::default()
    };
    let json = serde_json::to_string(&artifact).unwrap();
    assert!(json.contains("artifactId"));
    let back: Artifact = serde_json::from_str(&json).unwrap();
    assert_eq!(back.artifact_id, "a1");
    assert_eq!(back.kind, ArtifactKind::Screenshot);
}
