//! Shared carrier for the resolved tenant context.
//!
//! Port of `daemon/internal/tenantctx`. Go threads the value through
//! `context.Context`; in Rust the carrier is a tokio task-local, which
//! propagates across `.await` points within a scope the same way a context
//! value propagates down a call chain. `kura_api::middleware::protected`
//! installs the value after the identity manager resolves the caller, by
//! driving the downstream handler inside [`scope`]; store-layer guards read
//! it back via [`require`].
//!
//! **Propagation limit:** a tokio task-local follows `.await` points inside
//! the scope but is *not* inherited by `tokio::spawn`. Work detached from the
//! request task sees no context and [`require`] fails closed there — detached
//! work must carry the tenant explicitly or re-enter a [`scope`] of its own.

use std::future::Future;

use crate::types::IdentityError;
use crate::types::TenantContext;

tokio::task_local! {
    static TENANT_CONTEXT: TenantContext;
}

/// Runs the closure with `tc` installed as the current tenant context.
pub fn with_context<R>(tc: TenantContext, f: impl FnOnce() -> R) -> R {
    TENANT_CONTEXT.sync_scope(tc, f)
}

/// Drives the future with `tc` installed as the current tenant context.
pub async fn scope<F: Future>(tc: TenantContext, fut: F) -> F::Output {
    TENANT_CONTEXT.scope(tc, fut).await
}

/// Extracts the resolved tenant context if one is installed in this scope.
pub fn from_context() -> Option<TenantContext> {
    TENANT_CONTEXT.try_with(Clone::clone).ok()
}

/// Returns the resolved tenant id, or [`IdentityError::TenantContextRequired`]
/// when no tenant context is installed or it carries an empty tenant id. This
/// is the canonical entry point for store-layer tenant guards.
pub fn require() -> Result<String, IdentityError> {
    match from_context() {
        Some(tc) if !tc.tenant_id.is_empty() => Ok(tc.tenant_id),
        _ => Err(IdentityError::TenantContextRequired),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(principal: &str, tenant: &str, token: &str) -> TenantContext {
        TenantContext {
            principal_id: principal.to_string(),
            tenant_id: tenant.to_string(),
            token_id: token.to_string(),
            ..TenantContext::default()
        }
    }

    #[test]
    fn require_without_context() {
        assert!(matches!(
            require(),
            Err(IdentityError::TenantContextRequired)
        ));
        assert!(from_context().is_none());
    }

    #[test]
    fn require_with_empty_tenant_id() {
        with_context(ctx("prn_abc", "", ""), || {
            assert!(matches!(
                require(),
                Err(IdentityError::TenantContextRequired)
            ));
        });
    }

    #[test]
    fn round_trip() {
        let tc = ctx("prn_a", "ten_a", "tok_a");
        with_context(tc.clone(), || {
            let got = from_context().expect("tenant context installed");
            assert_eq!(got.principal_id, tc.principal_id);
            assert_eq!(got.tenant_id, tc.tenant_id);
            assert_eq!(got.token_id, tc.token_id);
            assert_eq!(require().expect("tenant id"), "ten_a");
        });
        // The scope restores on exit: no leakage.
        assert!(from_context().is_none());
    }

    #[test]
    fn isolation_between_scopes() {
        let id_a = with_context(ctx("", "ten_a", ""), || require().expect("a"));
        let id_b = with_context(ctx("", "ten_b", ""), || require().expect("b"));
        assert_eq!(id_a, "ten_a");
        assert_eq!(id_b, "ten_b");
    }

    #[tokio::test]
    async fn propagates_across_await_points() {
        scope(ctx("prn_a", "ten_async", "tok_a"), async {
            tokio::task::yield_now().await;
            assert_eq!(require().expect("tenant id"), "ten_async");
        })
        .await;
    }

    #[tokio::test]
    async fn nested_scope_shadows_and_restores() {
        scope(ctx("prn_a", "ten_outer", ""), async {
            assert_eq!(require().expect("outer"), "ten_outer");
            scope(ctx("prn_b", "ten_inner", ""), async {
                assert_eq!(require().expect("inner"), "ten_inner");
            })
            .await;
            assert_eq!(require().expect("outer restored"), "ten_outer");
        })
        .await;
    }
}
