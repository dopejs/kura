//! Port of daemon/internal/artifacts: computer-use artifact storage (sha256 addressing,
//! filesystem write/read, and the Artifact record). Billing quota enforcement (reserve/commit/
//! release) is a follow-up: the Rust billing manager is async, so the sync storage path here
//! defers the quota bridge.

use std::path::Path;

use chrono::Utc;
use sha2::{Digest, Sha256};

use kura_computeruse::{Artifact, ArtifactCaptureRequest, ArtifactStatus};

pub struct Service {
    data_dir: String,
}

impl Service {
    pub fn new(data_dir: &str) -> Self {
        Service {
            data_dir: data_dir.trim().to_string(),
        }
    }

    /// Persists a computer-use artifact and returns its record. The artifact id is derived from
    /// the first 8 bytes of the content sha256 (16 hex chars); the full 32-byte digest is stored
    /// on the record for integrity verification.
    pub fn save_computer_use_artifact(
        &self,
        input: ArtifactCaptureRequest,
    ) -> Result<Artifact, String> {
        let now = Utc::now();
        let digest = Sha256::digest(&input.content);
        let artifact_id = format!("cuart_{}", hex_encode(&digest[..8]));
        let storage_key = format!(
            "computer-use/{}/{}",
            input.computer_use_session_id, artifact_id
        );

        if !self.data_dir.trim().is_empty() {
            let full_path = Path::new(&self.data_dir)
                .join("artifacts")
                .join(&storage_key);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create artifact directory: {e}"))?;
            }
            std::fs::write(&full_path, &input.content)
                .map_err(|e| format!("write artifact content: {e}"))?;
        }

        Ok(Artifact {
            artifact_id: artifact_id.clone(),
            computer_use_session_id: input.computer_use_session_id,
            computer_use_action_id: input.computer_use_action_id,
            run_id: input.run_id,
            kind: input.kind,
            status: ArtifactStatus::Available,
            mime_type: input.mime_type.trim().to_string(),
            file_name: input.file_name.trim().to_string(),
            byte_size: input.content.len() as i64,
            storage_key,
            sha256: hex_encode(&digest),
            created_at: now,
            available_at: Some(now),
            ..Artifact::default()
        })
    }

    pub fn read_computer_use_artifact_content(&self, storage_key: &str) -> Result<Vec<u8>, String> {
        if self.data_dir.trim().is_empty() {
            return Ok(Vec::new());
        }
        let full_path = Path::new(&self.data_dir)
            .join("artifacts")
            .join(storage_key);
        std::fs::read(&full_path).map_err(|e| format!("read artifact content: {e}"))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
