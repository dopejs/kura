//! mail route family (port of daemon/internal/api/mail.go).
//!
//! Every handler mirrors the Go flow: require the mail + integrations
//! managers (500 when unconfigured), begin the integration-operation quota
//! reservation when a tenant context is present, drive the kura_mail Manager,
//! record the resulting account/operation/artifacts into the SQLite store and
//! publish the mail.* events, then commit (or release) the billing
//! reservation. Status codes, DTO shapes, validation and the
//! `writeMailError` mapping (404 for the not-found sentinels, 400 for the
//! invalid/blocked/unavailable sentinels) match the Go handlers.
//!
//! The `protected()` middleware and the `withByIDTenantGuard` wrapper are
//! applied by the app-wiring layer (they need the AppState at construction,
//! which a stateless family `router()` cannot reach — same deferral as the
//! other route families). Handlers therefore read the tenant context
//! opportunistically via `Option<Extension<TenantContext>>` and run the
//! by-id tenant-ownership check inline through
//! [`crate::middleware::guard_resource_for_tenant`], so behavior is identical
//! once the middleware lands.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};

use kura_billing::UsageReservation;
use kura_mail::{AccountProjection, Artifact, Operation, OperationFilter};

use crate::error::ApiError;
use crate::middleware::{TenantContext, guard_resource_for_tenant};
use crate::response::Json;
use crate::state::AppState;
use crate::types::{
    CreateMailDraftRequest, DownloadMailAttachmentRequest, ForwardMailMessageRequest,
    MailAccountListResponse, MailAttachmentRefRequest, MailAttachmentResponse,
    MailDraftListResponse, MailDraftResponse, MailMessageResponse, MailOperationListResponse,
    MailOperationResponse, MailSourceLinkageRequest, MailThreadListResponse, MailThreadResponse,
    ReplyMailMessageRequest, SendMailDraftRequest, SendMailMessageRequest, UpdateMailDraftRequest,
};

/// Handler error carrying either the canonical [`ApiError`] mapping or the
/// billing denial body the Go `writeBillingDenial` writes (503 / 429 with the
/// stable `DenialPayload` shape instead of `writeError`'s `{error: ...}`).
#[derive(Debug)]
enum MailApiError {
    Api(ApiError),
    BillingDenial {
        status: StatusCode,
        body: serde_json::Value,
    },
}

impl From<ApiError> for MailApiError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl IntoResponse for MailApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(err) => err.into_response(),
            Self::BillingDenial { status, body } => (status, AxumJson(body)).into_response(),
        }
    }
}

/// Reply/forward handlers return either a draft or a sent message depending on
/// the manager result (Go: `if draft != nil { MailDraftResponse } else {
/// MailMessageResponse }`).
#[derive(Debug)]
enum ReplyForwardOutcome {
    Draft(MailDraftResponse),
    Message(MailMessageResponse),
}

impl IntoResponse for ReplyForwardOutcome {
    fn into_response(self) -> Response {
        match self {
            Self::Draft(body) => Json(body).into_response(),
            Self::Message(body) => Json(body).into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// Query DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailThreadsQuery {
    #[serde(default)]
    integration_id: Option<String>,
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailOperationsQuery {
    #[serde(default)]
    integration_id: Option<String>,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    workflow_id: Option<String>,
    #[serde(default)]
    schedule_id: Option<String>,
    #[serde(default)]
    delivery_id: Option<String>,
    #[serde(default)]
    operation_class: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    result_mode: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    draft_id: Option<String>,
}

/// Route family router. Stateless: handlers take `State<AppState>` from the
/// router state applied by the top-level assembly.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        // /v1/mail/accounts
        .route("/v1/mail/accounts", get(list_accounts))
        .route("/v1/mail/accounts/{integration_id}", get(get_account))
        // /v1/mail/threads
        .route("/v1/mail/threads", get(list_threads))
        .route("/v1/mail/threads/{thread_id}", get(get_thread))
        // /v1/mail/messages
        .route("/v1/mail/messages/send", post(send_message))
        .route("/v1/mail/messages/{message_id}", get(get_message))
        .route("/v1/mail/messages/{message_id}/reply", post(reply_message))
        .route(
            "/v1/mail/messages/{message_id}/forward",
            post(forward_message),
        )
        // /v1/mail/drafts
        .route("/v1/mail/drafts", get(list_drafts).post(create_draft))
        .route("/v1/mail/drafts/{draft_id}", get(get_draft))
        .route("/v1/mail/drafts/{draft_id}/update", post(update_draft))
        .route("/v1/mail/drafts/{draft_id}/send", post(send_draft))
        // /v1/mail/attachments
        .route(
            "/v1/mail/attachments/{attachment_ref_id}/download",
            post(download_attachment),
        )
        // /v1/mail/operations
        .route("/v1/mail/operations", get(list_operations))
        .route("/v1/mail/operations/{operation_id}", get(get_operation))
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

/// GET /v1/mail/accounts — list projected mail accounts (Go
/// `handleMailAccounts`).
#[allow(clippy::unused_async)]
async fn list_accounts(
    State(state): State<AppState>,
    Query(query): Query<MailThreadsQuery>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailAccountListResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let selection = selection(query.integration_id.as_deref());
    let items = manager
        .list_accounts(&integrations.list(), &selection)
        .map_err(|err| write_mail_error(&err))?;
    record_mail_accounts(&state, tenant.as_ref().map(|t| &t.0), &items)?;
    Ok(Json(MailAccountListResponse { items }))
}

/// GET /v1/mail/accounts/{integrationId} — single projected account (Go
/// `handleMailAccountRoutes` + `withByIDTenantGuard` on mail_accounts).
#[allow(clippy::unused_async)]
async fn get_account(
    State(state): State<AppState>,
    Path(integration_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<AccountProjection>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let integration_id = integration_id.trim().to_string();
    if integration_id.is_empty() {
        // Go: http.NotFound.
        return Err(ApiError::NotFound("not found".to_string()).into());
    }
    guard_resource_for_tenant(
        &state,
        tenant.as_ref().map(|t| &t.0),
        "api:GET /v1/mail/accounts/{integrationId}",
        "mail_accounts",
        "mail_account_id",
        &integration_id,
        "mail_account",
    )
    .await?;
    let items = manager
        .list_accounts(
            &integrations.list(),
            &kura_mail::Selection {
                integration_id: integration_id.clone(),
                ..Default::default()
            },
        )
        .map_err(|err| write_mail_error(&err))?;
    if items.is_empty() {
        // Go: http.NotFound when no account matches.
        return Err(ApiError::NotFound("not found".to_string()).into());
    }
    record_mail_accounts(&state, tenant.as_ref().map(|t| &t.0), &items[..1])?;
    Ok(Json(items.into_iter().next().expect("non-empty items")))
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

/// GET /v1/mail/threads — list thread snapshots (Go `handleMailThreads`).
#[allow(clippy::unused_async)]
async fn list_threads(
    State(state): State<AppState>,
    Query(query): Query<MailThreadsQuery>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailThreadListResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "GET /v1/mail/threads",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::ListThreadsInput {
        selection: selection(query.integration_id.as_deref()),
        limit: query
            .limit
            .as_deref()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0),
        cursor: query
            .cursor
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        source: source_linkage_with_operation(&None, &operation_id),
    };
    match manager.list_threads(&integrations.list(), &input) {
        Ok((account, items, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailThreadListResponse {
                account,
                items,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// GET /v1/mail/threads/{threadId} — single thread snapshot (Go
/// `handleMailThreadRoutes`).
#[allow(clippy::unused_async)]
async fn get_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<MailThreadsQuery>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailThreadResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "GET /v1/mail/threads/{threadId}",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::GetThreadInput {
        selection: selection(query.integration_id.as_deref()),
        thread_id: thread_id.trim().to_string(),
        source: source_linkage_with_operation(&None, &operation_id),
    };
    match manager.get_thread(&integrations.list(), &input) {
        Ok((account, thread, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailThreadResponse {
                account,
                thread,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// GET /v1/mail/messages/{messageId} — single message snapshot (Go
/// `handleMailMessageGet`).
#[allow(clippy::unused_async)]
async fn get_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Query(query): Query<MailThreadsQuery>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailMessageResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "GET /v1/mail/messages/{messageId}",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::GetMessageInput {
        selection: selection(query.integration_id.as_deref()),
        message_id: message_id.trim().to_string(),
        source: source_linkage_with_operation(&None, &operation_id),
    };
    match manager.get_message(&integrations.list(), &input) {
        Ok((account, message, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailMessageResponse {
                account,
                message,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/messages/send — direct send (Go `handleMailSendMessage`);
/// 201 Created.
#[allow(clippy::unused_async)]
async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    AxumJson(request): AxumJson<SendMailMessageRequest>,
) -> Result<(StatusCode, Json<MailMessageResponse>), MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/messages/send",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::SendMessageInput {
        selection: selection(Some(&request.integration_id)),
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(&request.attachment_refs),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.send_message(&integrations.list(), &input) {
        Ok((account, message, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok((
                StatusCode::CREATED,
                Json(MailMessageResponse {
                    account,
                    message,
                    operation,
                    artifacts,
                }),
            ))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/messages/{messageId}/reply — draft or sent reply (Go
/// `handleMailReplyMessage`).
#[allow(clippy::unused_async)]
async fn reply_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    AxumJson(request): AxumJson<ReplyMailMessageRequest>,
) -> Result<ReplyForwardOutcome, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/messages/{messageId}/reply",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::ReplyMessageInput {
        selection: selection(Some(&request.integration_id)),
        message_id: message_id.trim().to_string(),
        result_mode: request.result_mode,
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(&request.attachment_refs),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.reply_message(&integrations.list(), &input) {
        Ok((account, draft, message, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            match (draft, message) {
                (Some(draft), _) => Ok(ReplyForwardOutcome::Draft(MailDraftResponse {
                    account,
                    draft,
                    operation,
                    artifacts,
                })),
                (None, Some(message)) => Ok(ReplyForwardOutcome::Message(MailMessageResponse {
                    account,
                    message,
                    operation,
                    artifacts,
                })),
                // Go would nil-dereference here; never panic — surface a 500.
                (None, None) => {
                    Err(ApiError::internal("mail reply produced neither draft nor message").into())
                }
            }
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/messages/{messageId}/forward — draft or sent forward (Go
/// `handleMailForwardMessage`).
#[allow(clippy::unused_async)]
async fn forward_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    AxumJson(request): AxumJson<ForwardMailMessageRequest>,
) -> Result<ReplyForwardOutcome, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/messages/{messageId}/forward",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::ForwardMessageInput {
        selection: selection(Some(&request.integration_id)),
        message_id: message_id.trim().to_string(),
        result_mode: request.result_mode,
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(&request.attachment_refs),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.forward_message(&integrations.list(), &input) {
        Ok((account, draft, message, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            match (draft, message) {
                (Some(draft), _) => Ok(ReplyForwardOutcome::Draft(MailDraftResponse {
                    account,
                    draft,
                    operation,
                    artifacts,
                })),
                (None, Some(message)) => Ok(ReplyForwardOutcome::Message(MailMessageResponse {
                    account,
                    message,
                    operation,
                    artifacts,
                })),
                (None, None) => Err(ApiError::internal(
                    "mail forward produced neither draft nor message",
                )
                .into()),
            }
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

// ---------------------------------------------------------------------------
// Drafts
// ---------------------------------------------------------------------------

/// GET /v1/mail/drafts — list draft snapshots (Go `handleMailDrafts` GET).
#[allow(clippy::unused_async)]
async fn list_drafts(
    State(state): State<AppState>,
    Query(query): Query<MailThreadsQuery>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailDraftListResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "GET /v1/mail/drafts",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::ListDraftsInput {
        selection: selection(query.integration_id.as_deref()),
        source: source_linkage_with_operation(&None, &operation_id),
    };
    match manager.list_drafts(&integrations.list(), &input) {
        Ok((account, items, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailDraftListResponse {
                account,
                items,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/drafts — create a draft (Go `handleMailDrafts` POST); 201
/// Created.
#[allow(clippy::unused_async)]
async fn create_draft(
    State(state): State<AppState>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    AxumJson(request): AxumJson<CreateMailDraftRequest>,
) -> Result<(StatusCode, Json<MailDraftResponse>), MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/drafts",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::CreateDraftInput {
        selection: selection(Some(&request.integration_id)),
        compose_mode: request.compose_mode,
        thread_id: request.thread_id.trim().to_string(),
        source_message_id: request.source_message_id.trim().to_string(),
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(&request.attachment_refs),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.create_draft(&integrations.list(), &input) {
        Ok((account, draft, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok((
                StatusCode::CREATED,
                Json(MailDraftResponse {
                    account,
                    draft,
                    operation,
                    artifacts,
                }),
            ))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// GET /v1/mail/drafts/{draftId} — single draft snapshot (Go
/// `handleMailDraftGet`).
#[allow(clippy::unused_async)]
async fn get_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    Query(query): Query<MailThreadsQuery>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailDraftResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "GET /v1/mail/drafts/{draftId}",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::GetDraftInput {
        selection: selection(query.integration_id.as_deref()),
        draft_id: draft_id.trim().to_string(),
        source: source_linkage_with_operation(&None, &operation_id),
    };
    match manager.get_draft(&integrations.list(), &input) {
        Ok((account, draft, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailDraftResponse {
                account,
                draft,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/drafts/{draftId}/update — update a draft (Go
/// `handleMailDraftUpdate`).
#[allow(clippy::unused_async)]
async fn update_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    AxumJson(request): AxumJson<UpdateMailDraftRequest>,
) -> Result<Json<MailDraftResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/drafts/{draftId}/update",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::UpdateDraftInput {
        selection: selection(Some(&request.integration_id)),
        draft_id: draft_id.trim().to_string(),
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.trim().to_string(),
        body: request.body,
        attachment_refs: mail_attachment_inputs(&request.attachment_refs),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.update_draft(&integrations.list(), &input) {
        Ok((account, draft, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailDraftResponse {
                account,
                draft,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

/// POST /v1/mail/drafts/{draftId}/send — send a draft (Go
/// `handleMailDraftSend`); tolerates an empty request body like Go's
/// `ErrBodyNotAllowed`/EOF tolerance.
#[allow(clippy::unused_async)]
async fn send_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    request: Option<AxumJson<SendMailDraftRequest>>,
) -> Result<Json<MailMessageResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let request = request
        .map(|AxumJson(r)| r)
        .unwrap_or(SendMailDraftRequest {
            integration_id: String::new(),
            source: None,
        });
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/drafts/{draftId}/send",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::SendDraftInput {
        selection: selection(Some(&request.integration_id)),
        draft_id: draft_id.trim().to_string(),
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.send_draft(&integrations.list(), &input) {
        Ok((account, _draft, message, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailMessageResponse {
                account,
                message,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

/// POST /v1/mail/attachments/{attachmentRefId}/download — resolve and
/// download an attachment reference (Go `handleMailAttachmentRoutes`, Roadmap
/// 64); tolerates an empty request body like Go.
#[allow(clippy::unused_async)]
async fn download_attachment(
    State(state): State<AppState>,
    Path(attachment_ref_id): Path<String>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
    request: Option<AxumJson<DownloadMailAttachmentRequest>>,
) -> Result<Json<MailAttachmentResponse>, MailApiError> {
    let (manager, integrations) = require_mail_deps(&state)?;
    let request = request
        .map(|AxumJson(r)| r)
        .unwrap_or(DownloadMailAttachmentRequest {
            integration_id: String::new(),
            message_id: String::new(),
            display_name: String::new(),
            media_type: String::new(),
            size_bytes: 0,
            source: None,
        });
    let operation_id = kura_mail::new_operation_id();
    let reservation = begin_integration_operation_quota(
        &state,
        tenant.as_ref().map(|t| &t.0),
        &operation_id,
        "POST /v1/mail/attachments/{attachmentRefId}/download",
        idempotency_key(&headers),
    )
    .await?;
    let input = kura_mail::DownloadAttachmentInput {
        selection: selection(Some(&request.integration_id)),
        message_id: request.message_id.trim().to_string(),
        attachment_ref_id: attachment_ref_id.trim().to_string(),
        display_name: request.display_name.trim().to_string(),
        media_type: request.media_type.trim().to_string(),
        size_bytes: request.size_bytes,
        source: source_linkage_with_operation(&request.source, &operation_id),
    };
    match manager.download_attachment(&integrations.list(), &input) {
        Ok((account, attachment, operation, artifacts)) => {
            record_mail_activity_and_commit_quota(
                &state,
                tenant.as_ref().map(|t| &t.0),
                reservation.as_ref(),
                Some(&account),
                &operation,
                &artifacts,
            )
            .await?;
            Ok(Json(MailAttachmentResponse {
                account,
                attachment,
                operation,
                artifacts,
            }))
        }
        Err(err) => {
            record_failed_mail_operation(
                &state,
                manager,
                tenant.as_ref().map(|t| &t.0),
                &operation_id,
                reservation.as_ref(),
            )
            .await?;
            Err(write_mail_error(&err))
        }
    }
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// GET /v1/mail/operations — list the operation ledger (Go
/// `handleMailOperations`).
#[allow(clippy::unused_async)]
async fn list_operations(
    State(state): State<AppState>,
    Query(query): Query<MailOperationsQuery>,
) -> Result<Json<MailOperationListResponse>, MailApiError> {
    let Some(manager) = &state.mail else {
        return Err(ApiError::Internal("mail manager is not configured".to_string()).into());
    };
    let mut filter = OperationFilter::default();
    filter.integration_id = trim_q(query.integration_id.as_deref());
    filter.run_id = trim_q(query.run_id.as_deref());
    filter.workflow_id = trim_q(query.workflow_id.as_deref());
    filter.schedule_id = trim_q(query.schedule_id.as_deref());
    filter.delivery_id = trim_q(query.delivery_id.as_deref());
    if let Some(class) =
        parse_mail_enum::<kura_mail::OperationClass>(&trim_q(query.operation_class.as_deref()))
    {
        filter.operation_class = class;
    }
    if let Some(status) =
        parse_mail_enum::<kura_mail::OperationStatus>(&trim_q(query.status.as_deref()))
    {
        filter.status = status;
    }
    if let Some(result_mode) =
        parse_mail_enum::<kura_mail::ResultMode>(&trim_q(query.result_mode.as_deref()))
    {
        filter.result_mode = result_mode;
    }
    filter.thread_id = trim_q(query.thread_id.as_deref());
    filter.message_id = trim_q(query.message_id.as_deref());
    filter.draft_id = trim_q(query.draft_id.as_deref());
    Ok(Json(MailOperationListResponse {
        items: manager.list_operations(&filter),
    }))
}

/// GET /v1/mail/operations/{operationId} — operation detail + artifacts (Go
/// `handleMailOperationRoutes` + `withByIDTenantGuard` on mail_operations).
#[allow(clippy::unused_async)]
async fn get_operation(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<MailOperationResponse>, MailApiError> {
    let Some(manager) = &state.mail else {
        return Err(ApiError::Internal("mail manager is not configured".to_string()).into());
    };
    let operation_id = operation_id.trim().to_string();
    if operation_id.is_empty() {
        // Go: http.NotFound.
        return Err(ApiError::NotFound("not found".to_string()).into());
    }
    guard_resource_for_tenant(
        &state,
        tenant.as_ref().map(|t| &t.0),
        "api:GET /v1/mail/operations/{operationId}",
        "mail_operations",
        "operation_id",
        &operation_id,
        "mail_operation",
    )
    .await?;
    let Some(operation) = manager.get_operation(&operation_id) else {
        // Go: http.NotFound.
        return Err(ApiError::NotFound("not found".to_string()).into());
    };
    let artifacts = manager.list_artifacts(&operation_id);
    Ok(Json(MailOperationResponse {
        operation,
        artifacts,
    }))
}

// ---------------------------------------------------------------------------
// Go helper ports
// ---------------------------------------------------------------------------

/// Go `requireMailDeps`: both managers must be configured (500 otherwise).
fn require_mail_deps(
    state: &AppState,
) -> Result<(&kura_mail::Manager, &kura_integrations::Manager), MailApiError> {
    let Some(manager) = &state.mail else {
        return Err(ApiError::Internal("mail dependencies are not configured".to_string()).into());
    };
    let Some(integrations) = &state.integrations else {
        return Err(ApiError::Internal("mail dependencies are not configured".to_string()).into());
    };
    Ok((manager, integrations))
}

/// Go `mailSourceLinkage(source)`; the operation id is always injected like
/// `mailSourceLinkageWithOperation`.
fn source_linkage_with_operation(
    source: &Option<MailSourceLinkageRequest>,
    operation_id: &str,
) -> kura_mail::SourceLinkage {
    let mut linkage = match source {
        Some(s) => kura_mail::SourceLinkage {
            run_id: s.run_id.trim().to_string(),
            step_id: s.step_id.trim().to_string(),
            tool_call_id: s.tool_call_id.trim().to_string(),
            workflow_id: s.workflow_id.trim().to_string(),
            workflow_step_id: s.workflow_step_id.trim().to_string(),
            schedule_id: s.schedule_id.trim().to_string(),
            schedule_attempt_id: s.schedule_attempt_id.trim().to_string(),
            delivery_id: s.delivery_id.trim().to_string(),
            allow_send_side_effects: s.allow_send_side_effects,
            ..Default::default()
        },
        None => kura_mail::SourceLinkage::default(),
    };
    linkage.operation_id = operation_id.trim().to_string();
    linkage
}

/// Go `mailAttachmentInputs`.
fn mail_attachment_inputs(
    items: &[MailAttachmentRefRequest],
) -> Vec<kura_mail::AttachmentRefInput> {
    if items.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .map(|item| kura_mail::AttachmentRefInput {
            attachment_ref_id: item.attachment_ref_id.trim().to_string(),
            display_name: item.display_name.trim().to_string(),
            media_type: item.media_type.trim().to_string(),
            size_bytes: Some(item.size_bytes),
            ..Default::default()
        })
        .collect()
}

/// Go `mail.Selection{IntegrationID: strings.TrimSpace(...)}`.
fn selection(integration_id: Option<&str>) -> kura_mail::Selection {
    kura_mail::Selection {
        integration_id: integration_id.unwrap_or_default().trim().to_string(),
    }
}

/// Go `r.Header.Get("Idempotency-Key")` (HeaderMap lookups are
/// case-insensitive).
fn idempotency_key(headers: &HeaderMap) -> &str {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default()
}

fn trim_q(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_string()
}

/// Parses an open string enum from a query value; unknown values yield `None`
/// (Go would set the raw string and match nothing, so leaving the filter
/// unset is the closest non-false-positive behavior).
fn parse_mail_enum<T>(value: &str) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_value(serde_json::Value::String(trimmed.to_string())).ok()
}

/// Go `writeMailError`: the four not-found sentinels map to 404; every other
/// error (including the Go default branch) maps to 400.
fn write_mail_error(err: &kura_mail::MailError) -> MailApiError {
    match err {
        kura_mail::MailError::MailIntegrationNotFound
        | kura_mail::MailError::MailThreadNotFound
        | kura_mail::MailError::MailMessageNotFound
        | kura_mail::MailError::MailDraftNotFound => {
            MailApiError::Api(ApiError::NotFound(err.to_string()))
        }
        kura_mail::MailError::MailRecipientRequired
        | kura_mail::MailError::MailAttachmentUnresolved
        | kura_mail::MailError::MailBackgroundSendBlocked
        | kura_mail::MailError::MailSelectionInvalid
        | kura_mail::MailError::MailUnavailable => {
            MailApiError::Api(ApiError::BadRequest(err.to_string()))
        }
        other => MailApiError::Api(ApiError::BadRequest(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Billing quota (Go billing_enforcement.go: beginIntegrationOperationQuota,
// commitBillingReservation, releaseBillingReservation)
// ---------------------------------------------------------------------------

/// Go `beginIntegrationOperationQuota`: no tenant context means no gate
/// (empty reservation); otherwise reserve one integration operation against
/// the tenant's quota and return the reservation. Denials become 429
/// (`quota_denied`) or 503 with the stable denial payload.
async fn begin_integration_operation_quota(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation_id: &str,
    entry_point: &str,
    idempotency_key: &str,
) -> Result<Option<UsageReservation>, MailApiError> {
    let Some(tc) = tenant else {
        return Ok(None);
    };
    if tc.0.tenant_id.is_empty() {
        return Ok(None);
    }
    let tenant_id = tc.0.tenant_id.clone();
    let operation_key =
        kura_billing::integration_operation_key(&tenant_id, "mail", operation_id, idempotency_key);
    let hosted = matches!(state.config.environment, kura_config::Environment::Prod);
    let result = match &state.billing {
        Some(manager) => {
            manager
                .reserve(kura_billing::ReserveInput {
                    tenant_id,
                    category: kura_billing::Category::from(
                        kura_billing::Category::INTEGRATION_OPERATIONS,
                    ),
                    amount: 1,
                    operation_key: operation_key.clone(),
                    reservation_point: format!(
                        "{entry_point} before integration backend operation"
                    ),
                    guarded_entry_point: entry_point.to_string(),
                    hosted,
                    ..Default::default()
                })
                .await
        }
        None => {
            if hosted {
                Ok(kura_billing::ReserveResult {
                    allowed: false,
                    denial: Some(kura_billing::new_quota_state_unavailable_denial(
                        &tenant_id,
                        &operation_key,
                    )),
                    failure: Some(kura_billing::BillingError::QuotaStateUnavailable),
                    ..Default::default()
                })
            } else {
                Ok(kura_billing::ReserveResult {
                    allowed: true,
                    ..Default::default()
                })
            }
        }
    };
    let result = result.map_err(|err| MailApiError::Api(ApiError::internal(err)))?;
    if !result.allowed {
        let status = if matches!(
            result.failure,
            Some(kura_billing::BillingError::QuotaDenied)
        ) {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        let body = match result.denial {
            Some(denial) => {
                serde_json::to_value(denial).unwrap_or_else(|_| serde_json::Value::Null)
            }
            None => {
                let message = result
                    .failure
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "quota denied".to_string());
                serde_json::json!({ "error": message })
            }
        };
        return Err(MailApiError::BillingDenial { status, body });
    }
    Ok(result.reservation)
}

/// Go `commitBillingReservation`: no-op without a manager or an empty
/// reservation id.
async fn commit_billing_reservation(
    state: &AppState,
    reservation: Option<&UsageReservation>,
) -> Result<(), MailApiError> {
    let Some(reservation) = reservation else {
        return Ok(());
    };
    if reservation.reservation_id.is_empty() {
        return Ok(());
    }
    let Some(manager) = &state.billing else {
        return Ok(());
    };
    manager
        .commit(kura_billing::ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: "billing.integration_operation_committed".to_string(),
            reason: "mail operation recorded after backend attempt".to_string(),
            ..Default::default()
        })
        .await
        .map_err(|err| MailApiError::Api(ApiError::internal(err)))?;
    Ok(())
}

/// Go `releaseBillingReservation`: best-effort release (errors swallowed).
async fn release_billing_reservation(
    state: &AppState,
    reservation: Option<&UsageReservation>,
    reason: &str,
) {
    let Some(reservation) = reservation else {
        return;
    };
    if reservation.reservation_id.is_empty() {
        return;
    }
    let Some(manager) = &state.billing else {
        return;
    };
    let _ = manager
        .release(kura_billing::ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: "billing.reservation_released".to_string(),
            reason: reason.to_string(),
            ..Default::default()
        })
        .await;
}

// ---------------------------------------------------------------------------
// Activity recording (Go recordMailAccounts / recordMailActivity /
// recordMailActivityAndCommitQuota)
// ---------------------------------------------------------------------------

/// Go `recordMailAccounts`: persist + publish every projected account.
fn record_mail_accounts(
    state: &AppState,
    tenant: Option<&TenantContext>,
    items: &[AccountProjection],
) -> Result<(), MailApiError> {
    for item in items {
        persist_mail_account(state, item)?;
        publish_mail_account_projected(state, tenant, item)?;
    }
    Ok(())
}

/// Go `recordMailActivityAndCommitQuota` (the operation is always non-empty
/// after a manager attempt; an empty operation skips everything).
async fn record_mail_activity_and_commit_quota(
    state: &AppState,
    tenant: Option<&TenantContext>,
    reservation: Option<&UsageReservation>,
    account: Option<&AccountProjection>,
    operation: &Operation,
    artifacts: &[Artifact],
) -> Result<(), MailApiError> {
    if operation.operation_id.is_empty() {
        return Ok(());
    }
    record_mail_activity(state, tenant, account, operation, artifacts)?;
    commit_billing_reservation(state, reservation).await
}

/// Go `recordMailActivity`: persist the account projection, the operation,
/// and every artifact, publishing the mail.* events; terminal operation
/// statuses publish the completed/failed event.
fn record_mail_activity(
    state: &AppState,
    tenant: Option<&TenantContext>,
    account: Option<&AccountProjection>,
    operation: &Operation,
    artifacts: &[Artifact],
) -> Result<(), MailApiError> {
    if let Some(account) = account {
        if !account.integration_id.is_empty() {
            persist_mail_account(state, account)?;
            publish_mail_account_projected(state, tenant, account)?;
        }
    }
    if operation.operation_id.is_empty() {
        return Ok(());
    }
    persist_mail_operation(state, operation)?;
    publish_mail_operation_requested(state, tenant, operation)?;
    for artifact in artifacts {
        persist_mail_artifact(state, artifact)?;
        publish_mail_artifact_recorded(state, tenant, artifact, operation)?;
    }
    match operation.status {
        kura_mail::OperationStatus::Completed => {
            publish_mail_operation_completed(state, tenant, operation)?;
        }
        kura_mail::OperationStatus::Failed
        | kura_mail::OperationStatus::Blocked
        | kura_mail::OperationStatus::Cancelled => {
            publish_mail_operation_failed(state, tenant, operation)?;
        }
        _ => {}
    }
    Ok(())
}

/// Error path: the Go manager returns the failed/blocked operation alongside
/// the error; the Rust manager keeps it in its ledger, so re-read it and
/// record it. When no operation was created (selection failed before a backend
/// attempt), release the billing reservation like Go.
async fn record_failed_mail_operation(
    state: &AppState,
    manager: &kura_mail::Manager,
    tenant: Option<&TenantContext>,
    operation_id: &str,
    reservation: Option<&UsageReservation>,
) -> Result<(), MailApiError> {
    let Some(operation) = manager.get_operation(operation_id) else {
        release_billing_reservation(
            state,
            reservation,
            "mail operation failed before backend attempt",
        )
        .await;
        return Ok(());
    };
    let artifacts = manager.list_artifacts(operation_id);
    let account = manager.get_account(&operation.integration_id);
    record_mail_activity_and_commit_quota(
        state,
        tenant,
        reservation,
        account.as_ref(),
        &operation,
        &artifacts,
    )
    .await
}

fn persist_mail_account(state: &AppState, item: &AccountProjection) -> Result<(), MailApiError> {
    state
        .store
        .lock()
        .upsert_mail_account(item)
        .map_err(ApiError::from_store)?;
    Ok(())
}

fn persist_mail_operation(state: &AppState, item: &Operation) -> Result<(), MailApiError> {
    state
        .store
        .lock()
        .upsert_mail_operation(item)
        .map_err(ApiError::from_store)?;
    Ok(())
}

fn persist_mail_artifact(state: &AppState, item: &Artifact) -> Result<(), MailApiError> {
    state
        .store
        .lock()
        .upsert_mail_artifact(item)
        .map_err(ApiError::from_store)?;
    Ok(())
}

fn publish_mail_account_projected(
    state: &AppState,
    tenant: Option<&TenantContext>,
    account: &AccountProjection,
) -> Result<(), MailApiError> {
    let payload = serde_json::json!({
        "integrationId": account.integration_id,
        "accountKey": account.account_key,
        "mailboxAddress": account.mailbox_address,
        "readinessStatus": account.readiness_status,
        "canonicalDefault": account.canonical_default,
    });
    publish_mail_event(
        state,
        tenant,
        "mail.account_projected",
        "mail_account",
        &account.mail_account_id,
        payload,
    )?;
    Ok(())
}

fn publish_mail_operation_requested(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &Operation,
) -> Result<(), MailApiError> {
    publish_mail_operation_event(state, tenant, "mail.operation_requested", operation)
}

fn publish_mail_operation_completed(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &Operation,
) -> Result<(), MailApiError> {
    publish_mail_operation_event(state, tenant, "mail.operation_completed", operation)
}

fn publish_mail_operation_failed(
    state: &AppState,
    tenant: Option<&TenantContext>,
    operation: &Operation,
) -> Result<(), MailApiError> {
    publish_mail_operation_event(state, tenant, "mail.operation_failed", operation)
}

fn publish_mail_operation_event(
    state: &AppState,
    tenant: Option<&TenantContext>,
    name: &str,
    operation: &Operation,
) -> Result<(), MailApiError> {
    let payload = serde_json::json!({
        "operationId": operation.operation_id,
        "operationClass": operation.operation_class,
        "integrationId": operation.integration_id,
        "runId": operation.run_id,
        "workflowId": operation.workflow_id,
        "scheduleId": operation.schedule_id,
        "resultMode": operation.result_mode,
        "sendPath": operation.send_path,
        "threadId": operation.thread_id,
        "messageId": operation.message_id,
        "draftId": operation.draft_id,
        "failureClass": operation.failure_class,
    });
    publish_mail_event(
        state,
        tenant,
        name,
        "mail_operation",
        &operation.operation_id,
        payload,
    )?;
    Ok(())
}

fn publish_mail_artifact_recorded(
    state: &AppState,
    tenant: Option<&TenantContext>,
    artifact: &Artifact,
    operation: &Operation,
) -> Result<(), MailApiError> {
    let payload = serde_json::json!({
        "artifactId": artifact.artifact_id,
        "operationId": operation.operation_id,
        "threadId": artifact.thread_id,
        "messageId": artifact.message_id,
        "draftId": artifact.draft_id,
        "attachmentRefId": artifact.attachment_ref_id,
    });
    publish_mail_event(
        state,
        tenant,
        "mail.artifact_recorded",
        "mail_artifact",
        &artifact.artifact_id,
        payload,
    )?;
    Ok(())
}

/// Go \`publishEvent\` for the mail category: fill environment scope from the
/// config, bind the tenant id when a tenant context is present (mail is not a
/// global category), persist the event row, then fan out on the bus.
fn publish_mail_event(
    state: &AppState,
    tenant: Option<&TenantContext>,
    name: &str,
    resource_kind: &str,
    resource_id: &str,
    payload: serde_json::Value,
) -> Result<kura_events::Event, MailApiError> {
    let mut event = kura_events::Event {
        event_id: new_event_id(),
        environment_scope: crate::middleware::environment_scope_from_config(&state.config),
        category: "mail".to_string(),
        name: name.to_string(),
        occurred_at: chrono::Utc::now(),
        scope: kura_events::Scope::default(),
        resource: kura_events::Resource {
            kind: resource_kind.to_string(),
            id: resource_id.to_string(),
        },
        payload: payload.as_object().cloned().unwrap_or_default(),
        ..Default::default()
    };
    if let Some(tc) = tenant {
        if !tc.0.tenant_id.is_empty() && !kura_events::is_global_category(&event.category) {
            event.tenant_id = tc.0.tenant_id.clone();
        }
    }
    let persisted = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(state.event_bus.publish(persisted))
}

fn new_event_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("evt_{}", &hex[..16])
}

// ---------------------------------------------------------------------------
// Handler behavior tests (port of daemon/internal/api/mail_test.go)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::{Method, Request};
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-mail-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                telegram: kura_config::TelegramConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                slack: kura_config::SlackConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                matrix: kura_config::MatrixConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
            },
        }
    }

    /// Go \`seedHealthyMailIntegration\`: create a healthy fake-local mail
    /// integration (the in-memory manager holds it; no store row needed since
    /// these tests carry no tenant context).
    fn seed_healthy_mail_integration(
        manager: &kura_integrations::Manager,
        integration_id: &str,
        canonical_default: bool,
    ) {
        let created = manager
            .create(kura_integrations::CreateInput {
                integration_id: integration_id.to_string(),
                domain_kind: "mail".to_string(),
                display_name: integration_id.to_string(),
                environment_scope: "test".to_string(),
                canonical_default,
                account_binding: kura_integrations::AccountBinding {
                    account_key: "alice@example.com".to_string(),
                    account_label: "Alice Mailbox".to_string(),
                    ..Default::default()
                },
                backend_binding: kura_integrations::BackendBinding {
                    backend_kind: kura_integrations::BackendKind::FakeLocal,
                    supports_probe_read: true,
                    supports_probe_mutation: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("create mail integration");
        manager
            .update_readiness(
                &created.integration_id,
                kura_integrations::UpdateReadinessInput {
                    readiness_status: kura_integrations::ReadinessStatus::Healthy,
                    auth_state: kura_integrations::AuthState::Authorized
                        .as_str()
                        .to_string(),
                    health_state: kura_integrations::HealthState::Healthy.as_str().to_string(),
                    secret_resolution: "resolved".to_string(),
                    ..Default::default()
                },
            )
            .expect("update readiness");
    }

    /// Builds a state with only the mail-relevant managers populated (shared
    /// behind Arcs so cloned states observe the same in-memory ledger).
    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-mail-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let integrations = Arc::new(kura_integrations::Manager::new("test"));
        let mail = Arc::new(kura_mail::Manager::new("test"));
        let mut state = AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store);
        state.integrations = Some(integrations);
        state.mail = Some(mail);
        state
    }

    async fn request(
        state: AppState,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, serde_json::Value, String) {
        let app = crate::routes::router(state);
        let mut builder = Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder
            .body(axum::body::Body::from(body.unwrap_or_default().to_string()))
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8_lossy(&bytes).to_string();
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        (status, json, text)
    }

    async fn get(state: AppState, uri: &str) -> (StatusCode, serde_json::Value, String) {
        request(state, Method::GET, uri, None).await
    }

    async fn post(
        state: AppState,
        uri: &str,
        body: &str,
    ) -> (StatusCode, serde_json::Value, String) {
        request(state, Method::POST, uri, Some(body)).await
    }

    #[tokio::test]
    async fn mail_routes_support_selection_fallback_and_inspection() {
        let state = test_state();
        let integrations = state.integrations.clone().expect("integrations");
        seed_healthy_mail_integration(&integrations, "mail-a", true);
        seed_healthy_mail_integration(&integrations, "mail-b", false);

        // GET /v1/mail/accounts lists both projected accounts.
        let (status, json, body) = get(state.clone(), "/v1/mail/accounts").await;
        assert_eq!(status, StatusCode::OK, "accounts body={body}");
        assert_eq!(
            json["items"].as_array().map(Vec::len),
            Some(2),
            "accounts={json}"
        );

        // Explicit selection wins over the canonical default.
        let (status, json, body) =
            get(state.clone(), "/v1/mail/threads?integrationId=mail-b").await;
        assert_eq!(status, StatusCode::OK, "explicit threads body={body}");
        assert_eq!(json["account"]["integrationId"], "mail-b");
        assert_eq!(json["operation"]["selectionMode"], "explicit");
        assert_eq!(json["items"][0]["threadId"], "thread_seed");

        // No selection falls back to the canonical default account.
        let (status, json, body) = get(state.clone(), "/v1/mail/threads").await;
        assert_eq!(status, StatusCode::OK, "default threads body={body}");
        assert_eq!(json["account"]["integrationId"], "mail-a");
        assert_eq!(json["operation"]["selectionMode"], "canonical_default");

        // Message detail returns the seeded message snapshot.
        let (status, json, body) = get(
            state.clone(),
            "/v1/mail/messages/msg_seed?integrationId=mail-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "message body={body}");
        assert_eq!(json["operation"]["operationClass"], "get_message");
        assert_eq!(json["message"]["messageId"], "msg_seed");

        // Draft list returns the seeded draft snapshot.
        let (status, json, body) = get(state.clone(), "/v1/mail/drafts?integrationId=mail-a").await;
        assert_eq!(status, StatusCode::OK, "drafts body={body}");
        assert_eq!(json["items"][0]["draftId"], "draft_seed");

        // Explicit mail read operations are persisted to the SQLite store.
        let persisted = state
            .store
            .lock()
            .list_mail_operations(
                "test",
                &kura_store::mail::MailOperationFilter {
                    integration_id: "mail-b".to_string(),
                    ..Default::default()
                },
            )
            .expect("list mail operations");
        assert!(
            !persisted.is_empty(),
            "expected persisted explicit mail read operations"
        );
    }

    #[tokio::test]
    async fn mail_mutation_routes_preserve_send_truth_and_block_unsafe_send() {
        let state = test_state();
        let integrations = state.integrations.clone().expect("integrations");
        seed_healthy_mail_integration(&integrations, "mail-a", true);

        // Create draft -> 201 with draft-only result truth.
        let (status, json, body) = post(
            state.clone(),
            "/v1/mail/drafts",
            r#"{"integrationId":"mail-a","composeMode":"new_message","to":["carol@example.com"],"subject":"Phase 30 draft","body":"Draft body"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create draft body={body}");
        assert_eq!(json["operation"]["resultMode"], "draft_only");
        assert_eq!(json["draft"]["composeMode"], "new_message");
        let draft_id = json["draft"]["draftId"]
            .as_str()
            .expect("draftId")
            .to_string();

        // Update draft -> 200, stable identity, updated status.
        let (status, json, body) = post(
            state.clone(),
            &format!("/v1/mail/drafts/{draft_id}/update"),
            r#"{"subject":"Phase 30 updated","body":"Updated body"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update draft body={body}");
        assert_eq!(json["draft"]["draftId"], draft_id);
        assert_eq!(json["draft"]["draftStatus"], "updated");

        // Direct send -> 201 with sent/direct path truth.
        let (status, json, body) = post(
            state.clone(),
            "/v1/mail/messages/send",
            r#"{"integrationId":"mail-a","to":["dave@example.com"],"subject":"Phase 30 direct send","body":"Sent body"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "direct send body={body}");
        assert_eq!(json["operation"]["resultMode"], "sent");
        assert_eq!(json["operation"]["sendPath"], "direct");

        // Send the created draft -> 200 with sent/draft path truth.
        let (status, json, body) = post(
            state.clone(),
            &format!("/v1/mail/drafts/{draft_id}/send"),
            r#"{"integrationId":"mail-a"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "send draft body={body}");
        assert_eq!(json["operation"]["resultMode"], "sent");
        assert_eq!(json["operation"]["sendPath"], "draft");

        // Reply in draft mode -> 200 with a draft response linked to msg_seed.
        let (status, json, body) = post(
            state.clone(),
            "/v1/mail/messages/msg_seed/reply",
            r#"{"integrationId":"mail-a","resultMode":"draft","body":"Reply later"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "reply body={body}");
        assert!(
            json.get("draft").is_some(),
            "expected draft response, got {json}"
        );
        assert_eq!(json["operation"]["resultMode"], "draft_only");
        assert_eq!(json["draft"]["sourceMessageId"], "msg_seed");

        // Forward in send mode -> 200 with a message response linked to msg_seed.
        let (status, json, body) = post(
            state.clone(),
            "/v1/mail/messages/msg_seed/forward",
            r#"{"integrationId":"mail-a","resultMode":"send","to":["erin@example.com"],"body":"FYI"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "forward body={body}");
        assert!(
            json.get("message").is_some(),
            "expected message response, got {json}"
        );
        assert_eq!(json["operation"]["resultMode"], "sent");
        assert_eq!(json["message"]["forwardedFromMessageId"], "msg_seed");

        // Unresolvable attachment blocks the send with a 400.
        let (status, _json, body) = post(
            state.clone(),
            "/v1/mail/messages/send",
            r#"{"integrationId":"mail-a","to":["frank@example.com"],"subject":"Blocked attachment","attachmentRefs":[{"attachmentRefId":"missing_contract_attachment"}]}"#,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "blocked attachment body={body}"
        );
        assert!(
            body.contains("attachment reference could not be resolved"),
            "expected attachment-unresolved message, got {body}"
        );

        // Background (workflow-sourced) send without allowSendSideEffects is gated.
        let (status, _json, body) = post(
            state.clone(),
            "/v1/mail/messages/send",
            r#"{"integrationId":"mail-a","to":["gina@example.com"],"subject":"Workflow blocked","source":{"workflowId":"wf_1"}}"#,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "background blocked body={body}"
        );
        assert!(
            body.contains(
                "background final send requires explicit allowSendSideEffects permission"
            ),
            "expected background-send gating message, got {body}"
        );

        // Blocked operations are persisted to the SQLite store.
        let blocked = state
            .store
            .lock()
            .list_mail_operations(
                "test",
                &kura_store::mail::MailOperationFilter {
                    status: "blocked".to_string(),
                    ..Default::default()
                },
            )
            .expect("list blocked mail operations");
        assert!(
            blocked.len() >= 2,
            "expected blocked mail operations to persist, got {blocked:?}"
        );
    }

    #[tokio::test]
    async fn mail_operations_routes_round_trip() {
        let state = test_state();
        let integrations = state.integrations.clone().expect("integrations");
        seed_healthy_mail_integration(&integrations, "mail-a", true);

        // A completed inspection operation exists after a thread list.
        let (status, json, body) =
            get(state.clone(), "/v1/mail/threads?integrationId=mail-a").await;
        assert_eq!(status, StatusCode::OK, "threads body={body}");
        let operation_id = json["operation"]["operationId"]
            .as_str()
            .expect("operationId")
            .to_string();

        // The operation ledger lists it.
        let (status, json, body) =
            get(state.clone(), "/v1/mail/operations?integrationId=mail-a").await;
        assert_eq!(status, StatusCode::OK, "operations body={body}");
        let ids: Vec<&str> = json["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|item| item["operationId"].as_str())
            .collect();
        assert!(
            ids.contains(&operation_id.as_str()),
            "expected {operation_id} in {ids:?}"
        );

        // Detail round-trips the operation with its artifact.
        let (status, json, body) = get(
            state.clone(),
            &format!("/v1/mail/operations/{operation_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "operation detail body={body}");
        assert_eq!(json["operation"]["operationId"], operation_id);
        assert_eq!(json["operation"]["operationClass"], "list_threads");

        // Unknown operation id -> 404.
        let (status, _json, _body) =
            get(state.clone(), "/v1/mail/operations/mail_op_does_not_exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Missing managers -> 500.
        let bare = AppState::new(
            test_config(),
            Arc::new(kura_events::Bus::new()),
            state.store.clone(),
        );
        let (status, json, body) = get(bare, "/v1/mail/accounts").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body={body}");
        assert_eq!(json["error"], "mail dependencies are not configured");
    }
}
