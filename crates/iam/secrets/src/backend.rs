//! Value backend abstraction and the local filesystem backend.
//!
//! The backend stores raw secret values outside the metadata store; metadata
//! rows only hold the opaque `backend_ref` returned by [`ValueBackend::put`].

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, SecretsError};
use crate::manager::BoxFuture;

/// Raw-value storage behind a secret version (Go `ValueBackend`).
///
/// `put` returns an opaque backend reference persisted on the version row;
/// `get`/`delete` resolve it. Implementations must never log the value.
pub trait ValueBackend: Send + Sync {
    fn put<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_id: &'a str,
        secret_version_id: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
    fn get<'a>(&'a self, backend_ref: &'a str) -> BoxFuture<'a, Result<String>>;
    fn delete<'a>(&'a self, backend_ref: &'a str) -> BoxFuture<'a, Result<()>>;
}

/// Filesystem-backed value store (Go `LocalBackend`). Values live in
/// `<root>/<tenant>/<secret>_<version>_<rand>` files with 0700 directory
/// and 0600 file permissions (unix).
#[derive(Debug)]
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if root.as_os_str().is_empty() {
            return Err(SecretsError::BackendRootRequired);
        }
        fs::create_dir_all(root)
            .map_err(|err| SecretsError::Backend(format!("create secret backend root: {err}")))?;
        set_dir_permissions(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lexically resolves `backend_ref` under the backend root, rejecting
    /// refs that escape it (Go: `filepath.Clean` + prefix check).
    fn resolve_ref(&self, backend_ref: &str) -> Result<PathBuf> {
        let path = clean_path(&self.root.join(backend_ref));
        let root = clean_path(&self.root);
        if path != root && !path.starts_with(&root) {
            return Err(SecretsError::BackendRefEscapesRoot);
        }
        Ok(path)
    }
}

impl ValueBackend for LocalBackend {
    fn put<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_id: &'a str,
        secret_version_id: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if tenant_id.is_empty() {
                return Err(SecretsError::TenantRequired);
            }
            if secret_id.is_empty() || secret_version_id.is_empty() {
                return Err(SecretsError::SecretIdsRequired);
            }
            if value.is_empty() {
                return Err(SecretsError::SecretValueRequired);
            }
            let tenant_segment = safe_path_segment(tenant_id);
            let tenant_dir = self.root.join(&tenant_segment);
            fs::create_dir_all(&tenant_dir)
                .map_err(|err| SecretsError::Backend(format!("create tenant secret dir: {err}")))?;
            #[cfg(unix)]
            set_dir_permissions(&tenant_dir)?;
            let name = format!(
                "{}_{}_{}",
                safe_path_segment(secret_id),
                safe_path_segment(secret_version_id),
                random_hex(8)
            );
            write_value_file(&tenant_dir.join(&name), value)?;
            // Backend refs use forward slashes (Go `filepath.ToSlash`).
            Ok(format!("{tenant_segment}/{name}"))
        })
    }

    fn get<'a>(&'a self, backend_ref: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if backend_ref.is_empty() {
                return Err(SecretsError::SecretVersionNotFound);
            }
            let path = self.resolve_ref(backend_ref)?;
            let data = fs::read(&path)
                .map_err(|err| SecretsError::Backend(format!("read secret value: {err}")))?;
            String::from_utf8(data)
                .map_err(|err| SecretsError::Backend(format!("read secret value: {err}")))
        })
    }

    fn delete<'a>(&'a self, backend_ref: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if backend_ref.is_empty() {
                return Ok(());
            }
            let path = self.resolve_ref(backend_ref)?;
            match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(SecretsError::Backend(format!("delete secret value: {err}"))),
            }
        })
    }
}

#[cfg(unix)]
fn set_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|err| SecretsError::Backend(format!("chmod secret backend root: {err}")))
}

#[cfg(not(unix))]
fn set_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_value_file(path: &Path, value: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| file.write_all(value.as_bytes()))
            .map_err(|err| SecretsError::Backend(format!("write secret value: {err}")))
    }
    #[cfg(not(unix))]
    {
        fs::write(path, value)
            .map_err(|err| SecretsError::Backend(format!("write secret value: {err}")))
    }
}

/// Maps any byte outside `[a-zA-Z0-9-_]` to `_` (Go `safePathSegment`).
pub fn safe_path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_".to_string() } else { out }
}

/// Cryptographically random lowercase hex of `byte_len` bytes (Go
/// `randomHex`). Backed by UUIDv4 randomness; unlike Go it has no degraded
/// all-zeros fallback — the OS RNG failure would panic inside `getrandom`,
/// which is preferable to silently emitting weak secret IDs.
pub fn random_hex(byte_len: usize) -> String {
    let mut out = String::with_capacity(byte_len * 2);
    while out.len() < byte_len * 2 {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(byte_len * 2);
    out
}

/// Lexical path cleanup equivalent to Go's `filepath.Clean` for absolute
/// paths: removes `.` and resolves `..` against prior components without
/// touching the filesystem.
fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    #[tokio::test]
    async fn put_get_delete_round_trip() {
        let dir = TestDir::new("backend-roundtrip");
        let backend = LocalBackend::new(dir.path()).expect("backend");
        let backend_ref = backend
            .put("ten/a", "sec_1", "secver_1", "value-1")
            .await
            .expect("put");
        assert!(backend_ref.starts_with("ten_a/sec_1_secver_1_"));
        assert_eq!(backend_ref.len(), "ten_a/sec_1_secver_1_".len() + 16);
        let value = backend.get(&backend_ref).await.expect("get");
        assert_eq!(value, "value-1");
        backend.delete(&backend_ref).await.expect("delete");
        assert!(backend.get(&backend_ref).await.is_err());
        // Deleting a missing ref is a no-op (Go: os.ErrNotExist tolerated).
        backend.delete(&backend_ref).await.expect("delete again");
        backend.delete("").await.expect("empty delete is a no-op");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn value_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("backend-perms");
        let backend = LocalBackend::new(dir.path()).expect("backend");
        let backend_ref = backend
            .put("ten_1", "sec_1", "secver_1", "value-1")
            .await
            .expect("put");
        let mode = fs::metadata(dir.path().join(&backend_ref))
            .expect("stat value file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = fs::metadata(dir.path().join("ten_1"))
            .expect("stat tenant dir")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }

    #[tokio::test]
    async fn get_rejects_refs_escaping_root() {
        let dir = TestDir::new("backend-escape");
        let backend = LocalBackend::new(dir.path()).expect("backend");
        for bad in ["../outside", "a/../../outside", ".."] {
            let err = backend.get(bad).await.expect_err("escape must fail");
            assert_eq!(err, SecretsError::BackendRefEscapesRoot, "ref {bad}");
            let err = backend.delete(bad).await.expect_err("escape must fail");
            assert_eq!(err, SecretsError::BackendRefEscapesRoot, "ref {bad}");
        }
        let err = backend.get("").await.expect_err("empty ref must fail");
        assert_eq!(err, SecretsError::SecretVersionNotFound);
    }

    #[tokio::test]
    async fn put_validates_inputs() {
        let dir = TestDir::new("backend-validate");
        let backend = LocalBackend::new(dir.path()).expect("backend");
        assert_eq!(
            backend
                .put("", "sec_1", "secver_1", "v")
                .await
                .expect_err("tenant"),
            SecretsError::TenantRequired
        );
        assert_eq!(
            backend
                .put("ten_1", "", "secver_1", "v")
                .await
                .expect_err("secret id"),
            SecretsError::SecretIdsRequired
        );
        assert_eq!(
            backend
                .put("ten_1", "sec_1", "secver_1", "")
                .await
                .expect_err("value"),
            SecretsError::SecretValueRequired
        );
    }

    #[test]
    fn safe_path_segment_sanitizes() {
        assert_eq!(safe_path_segment("ten_a/b.c:d e"), "ten_a_b_c_d_e");
        assert_eq!(safe_path_segment(""), "_");
        assert_eq!(safe_path_segment("ok-VALUE_123"), "ok-VALUE_123");
    }

    #[test]
    fn random_hex_has_expected_shape() {
        let value = random_hex(12);
        assert_eq!(value.len(), 24);
        assert!(
            value
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(random_hex(8), random_hex(8));
    }

    #[test]
    fn new_requires_root() {
        let err = LocalBackend::new("").expect_err("empty root must fail");
        assert_eq!(err, SecretsError::BackendRootRequired);
    }
}
