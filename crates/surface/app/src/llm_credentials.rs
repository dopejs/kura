//! Per-tenant LLM credentials on a shared daemon (shared-hosting.md §7).
//!
//! Installed only in shared hosting; a single-owner daemon keeps using each
//! provider's configured credential. On a shared daemon:
//!
//! - the platform's own work (no tenant) and the operator tenant use the
//!   configured credentials, as before;
//! - any other tenant reaches `openai_compatible` only with its own key,
//!   stored as the tenant secret [`TENANT_LLM_KEY_REF`];
//! - `echo` needs no credential and stays open;
//! - every other provider (subscription accounts, CLI bridges) is the
//!   operator's and is refused.
//!
//! Every refusal is a stable error code on the dispatch; nothing falls back
//! to the operator's key.

use std::sync::Arc;

use futures::future::BoxFuture;
use kura_api::hosting::Hosting;
use kura_llm::{ApiKey, CredentialDecision, CredentialPolicy};
use kura_secrets::SecretsError;

use crate::error::AppError;

/// The tenant secret holding a tenant's own OpenAI-compatible API key.
pub const TENANT_LLM_KEY_REF: &str = "llm.openai_compatible.api_key";

/// The provider a tenant can bring its own key to.
const BYOK_PROVIDER: &str = "openai_compatible";

/// Providers that hold no credential at all.
const CREDENTIAL_FREE_PROVIDERS: [&str; 1] = ["echo"];

pub const KEY_NOT_CONFIGURED_CODE: &str = "tenant_llm_key_not_configured";
pub const KEY_DISABLED_CODE: &str = "tenant_llm_key_disabled";
pub const KEY_UNAVAILABLE_CODE: &str = "tenant_llm_key_unavailable";
pub const PROVIDER_NOT_AVAILABLE_CODE: &str = "provider_not_available_to_tenant";

/// Installs the shared-hosting policy on `llm`. Single mode installs nothing
/// and is unchanged.
pub(crate) fn install(
    llm: &kura_llm::Dispatcher,
    hosting: &Hosting,
    secrets: Option<&Arc<kura_secrets::Manager>>,
) -> Result<(), AppError> {
    let Hosting::Shared { operator_tenant_id } = hosting else {
        return Ok(());
    };
    // Fail the boot rather than run shared without the policy: without it
    // every tenant would dispatch on the operator's credentials.
    let secrets = secrets.cloned().ok_or_else(|| {
        AppError::Hosting("shared hosting needs the tenant secret manager".to_string())
    })?;
    llm.set_credential_policy(Arc::new(SharedHostingCredentials::new(
        operator_tenant_id,
        secrets,
    )));
    Ok(())
}

pub(crate) struct SharedHostingCredentials {
    operator_tenant_id: String,
    secrets: Arc<kura_secrets::Manager>,
}

impl SharedHostingCredentials {
    pub(crate) fn new(operator_tenant_id: &str, secrets: Arc<kura_secrets::Manager>) -> Self {
        Self {
            operator_tenant_id: operator_tenant_id.trim().to_string(),
            secrets,
        }
    }

    async fn tenant_key(&self, tenant_id: &str) -> CredentialDecision {
        let resolved = self
            .secrets
            .resolve(kura_secrets::ResolveInput {
                tenant_id: tenant_id.to_string(),
                secret_ref: TENANT_LLM_KEY_REF.to_string(),
            })
            .await;
        match resolved {
            Ok(resolved) => match ApiKey::new(resolved.value) {
                Some(key) => CredentialDecision::Key(key),
                None => not_configured(),
            },
            Err(SecretsError::SecretNotFound | SecretsError::SecretVersionNotFound) => {
                not_configured()
            }
            Err(SecretsError::SecretDisabled) => refuse(
                KEY_DISABLED_CODE,
                format!(
                    "the {TENANT_LLM_KEY_REF} tenant secret is disabled; rotate it to re-enable"
                ),
            ),
            Err(err) => {
                // The error text stays in the daemon log; the caller gets the
                // code only, in case the backend's message names a path.
                eprintln!("[kura] llm credential: resolve tenant key for {tenant_id}: {err}");
                refuse(
                    KEY_UNAVAILABLE_CODE,
                    "the tenant's model API key could not be read".to_string(),
                )
            }
        }
    }
}

impl CredentialPolicy for SharedHostingCredentials {
    fn decide<'a>(
        &'a self,
        tenant_id: &'a str,
        provider: &'a str,
    ) -> BoxFuture<'a, CredentialDecision> {
        Box::pin(async move {
            let tenant_id = tenant_id.trim();
            if tenant_id.is_empty() || tenant_id == self.operator_tenant_id {
                return CredentialDecision::ProviderDefault;
            }
            let provider = provider.trim();
            if CREDENTIAL_FREE_PROVIDERS.contains(&provider) {
                return CredentialDecision::ProviderDefault;
            }
            if provider == BYOK_PROVIDER {
                return self.tenant_key(tenant_id).await;
            }
            refuse(
                PROVIDER_NOT_AVAILABLE_CODE,
                format!(
                    "provider {provider} is not available to tenants on a shared daemon; use {BYOK_PROVIDER} with your own key"
                ),
            )
        })
    }
}

fn not_configured() -> CredentialDecision {
    refuse(
        KEY_NOT_CONFIGURED_CODE,
        format!(
            "no model API key configured for this tenant; store one as the tenant secret {TENANT_LLM_KEY_REF}"
        ),
    )
}

fn refuse(code: &str, message: String) -> CredentialDecision {
    CredentialDecision::Refuse {
        code: code.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_store::{SQLiteStore, SecretStoreHandle};

    const OPERATOR: &str = "ten_local";

    fn secrets(dir: &std::path::Path) -> Arc<kura_secrets::Manager> {
        let backend = Arc::new(
            kura_secrets::LocalBackend::new(dir.join("tenant-secret-values")).expect("backend"),
        );
        let store = Arc::new(SecretStoreHandle::new(
            SQLiteStore::new(dir.to_str().expect("utf-8 dir")).expect("store"),
        ));
        Arc::new(kura_secrets::Manager::new(store, backend))
    }

    async fn store_key(secrets: &kura_secrets::Manager, tenant_id: &str, value: &str) {
        secrets
            .create(kura_secrets::CreateInput {
                tenant_id: tenant_id.to_string(),
                secret_ref: TENANT_LLM_KEY_REF.to_string(),
                display_name: String::new(),
                value: value.to_string(),
                document: None,
            })
            .await
            .expect("create tenant key");
    }

    fn refused_with(decision: &CredentialDecision) -> &str {
        match decision {
            CredentialDecision::Refuse { code, .. } => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn each_tenant_gets_only_its_own_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secrets = secrets(dir.path());
        store_key(&secrets, "ten_a", "sk-a").await;
        store_key(&secrets, "ten_b", "sk-b").await;
        let policy = SharedHostingCredentials::new(OPERATOR, secrets);

        for (tenant, key) in [("ten_a", "sk-a"), ("ten_b", "sk-b")] {
            match policy.decide(tenant, BYOK_PROVIDER).await {
                CredentialDecision::Key(got) => assert_eq!(got.expose(), key, "{tenant}"),
                other => panic!("{tenant}: expected its key, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_tenant_without_a_key_is_refused_not_given_the_operators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = SharedHostingCredentials::new(OPERATOR, secrets(dir.path()));

        let decision = policy.decide("ten_a", BYOK_PROVIDER).await;

        assert_eq!(refused_with(&decision), KEY_NOT_CONFIGURED_CODE);
    }

    #[tokio::test]
    async fn a_disabled_key_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secrets = secrets(dir.path());
        store_key(&secrets, "ten_a", "sk-a").await;
        secrets
            .disable(kura_secrets::DisableInput {
                tenant_id: "ten_a".to_string(),
                secret_ref: TENANT_LLM_KEY_REF.to_string(),
                disabled_reason: "leaked".to_string(),
            })
            .await
            .expect("disable");
        let policy = SharedHostingCredentials::new(OPERATOR, secrets);

        let decision = policy.decide("ten_a", BYOK_PROVIDER).await;

        assert_eq!(refused_with(&decision), KEY_DISABLED_CODE);
    }

    #[tokio::test]
    async fn the_platform_and_the_operator_keep_the_configured_credentials() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = SharedHostingCredentials::new(OPERATOR, secrets(dir.path()));

        for (tenant, provider) in [
            ("", BYOK_PROVIDER),
            ("  ", "claude_code"),
            (OPERATOR, BYOK_PROVIDER),
            (OPERATOR, "claude_code"),
        ] {
            assert_eq!(
                policy.decide(tenant, provider).await,
                CredentialDecision::ProviderDefault,
                "{tenant:?} {provider}"
            );
        }
    }

    #[tokio::test]
    async fn a_tenant_may_not_use_the_operators_other_providers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secrets = secrets(dir.path());
        // Even with a key stored: the key is for openai_compatible only.
        store_key(&secrets, "ten_a", "sk-a").await;
        let policy = SharedHostingCredentials::new(OPERATOR, secrets);

        for provider in ["claude_code", "codex", "my_account", ""] {
            let decision = policy.decide("ten_a", provider).await;
            assert_eq!(
                refused_with(&decision),
                PROVIDER_NOT_AVAILABLE_CODE,
                "{provider}"
            );
        }
        assert_eq!(
            policy.decide("ten_a", "echo").await,
            CredentialDecision::ProviderDefault
        );
    }
}
