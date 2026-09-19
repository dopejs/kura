use kura_artifacts::Service;
use kura_computeruse::{ArtifactCaptureRequest, ArtifactKind, ArtifactStatus};

#[test]
fn save_and_read_artifact() {
    let dir = std::env::temp_dir().join(format!("kura_artifacts_test_{}", std::process::id()));
    let service = Service::new(dir.to_str().unwrap());
    let request = ArtifactCaptureRequest {
        run_id: "r1".to_string(),
        computer_use_session_id: "s1".to_string(),
        computer_use_action_id: "a1".to_string(),
        kind: ArtifactKind::Screenshot,
        mime_type: "text/plain".to_string(),
        file_name: "shot.txt".to_string(),
        content: b"screenshot bytes".to_vec(),
        ..ArtifactCaptureRequest::default()
    };
    let artifact = service.save_computer_use_artifact(request).unwrap();
    assert_eq!(artifact.status, ArtifactStatus::Available);
    assert!(artifact.artifact_id.starts_with("cuart_"));
    assert_eq!(artifact.byte_size, 16);
    assert_eq!(artifact.sha256.len(), 64);

    let content = service
        .read_computer_use_artifact_content(&artifact.storage_key)
        .unwrap();
    assert_eq!(content, b"screenshot bytes".to_vec());
}

#[test]
fn artifact_id_is_deterministic() {
    let dir = std::env::temp_dir().join(format!("kura_artifacts_det_{}", std::process::id()));
    let service = Service::new(dir.to_str().unwrap());
    let make = |content: &[u8]| ArtifactCaptureRequest {
        run_id: "r1".to_string(),
        computer_use_session_id: "s1".to_string(),
        computer_use_action_id: "a1".to_string(),
        kind: ArtifactKind::PageSnapshot,
        content: content.to_vec(),
        ..ArtifactCaptureRequest::default()
    };
    let a = service.save_computer_use_artifact(make(b"same")).unwrap();
    let b = service.save_computer_use_artifact(make(b"same")).unwrap();
    assert_eq!(a.artifact_id, b.artifact_id);
}
