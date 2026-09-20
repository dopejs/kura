//! SQLite-backed artifact recorder (wave 8 parity): persists computer-use
//! artifact records through the store's computeruse DAOs and writes content
//! bytes to the artifacts directory using sha256 addressing, mirroring
//! `daemon/internal/artifacts/service.go` (the content itself has no store
//! column, so it lives under `<data_dir>/artifacts` keyed by storage key).

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::Artifact;
use crate::ArtifactCaptureRequest;
use crate::ArtifactRecorder;
use crate::ArtifactStatus;
use crate::Store;

/// SQLite-backed [`ArtifactRecorder`]: artifact records are persisted through
/// the `kura-store` computeruse DAOs ([`Store::upsert_computer_use_artifact`]),
/// and content bytes are written/read from the artifacts directory.
pub struct SqliteArtifactRecorder {
    store: Arc<dyn Store>,
    data_dir: String,
    environment_scope: String,
}

impl SqliteArtifactRecorder {
    /// `store` is the same computer-use store handle the manager uses (in the
    /// app wiring, an `Arc<kura_store::ComputerUseStoreHandle>`), so artifact
    /// records land in the same SQLite database. `environment_scope` stamps the
    /// record so the DAO's environment-scoped reads find it.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, data_dir: &str, environment_scope: &str) -> Self {
        Self {
            store,
            data_dir: data_dir.trim().to_string(),
            environment_scope: environment_scope.trim().to_string(),
        }
    }
}

impl ArtifactRecorder for SqliteArtifactRecorder {
    fn save_computer_use_artifact(
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

        if !self.data_dir.is_empty() {
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

        let artifact = Artifact {
            artifact_id: artifact_id.clone(),
            environment_scope: self.environment_scope.clone(),
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
        };
        // Persist the artifact record through the store computeruse DAO so it is
        // durable even before the manager's own upsert (idempotent by artifact id).
        self.store.upsert_computer_use_artifact(&artifact)?;
        Ok(artifact)
    }

    fn read_computer_use_artifact_content(&self, storage_key: &str) -> Result<Vec<u8>, String> {
        if self.data_dir.is_empty() {
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
