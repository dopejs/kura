use kura_policy::{
    ApprovalStatus, DecisionOutcome, Engine, RequestApprovalInput, ResolveApprovalInput,
};

fn request(action: &str) -> RequestApprovalInput {
    RequestApprovalInput {
        action: action.to_string(),
        resource_kind: "calendar".to_string(),
        resource_id: "cal-1".to_string(),
        reason: "test reason".to_string(),
        requested_by: "prn_1".to_string(),
        ..RequestApprovalInput::default()
    }
}

#[test]
fn request_approval_creates_pending() {
    let engine = Engine::new();
    let (approval, decision) = engine
        .request_approval(request("calendar.create"))
        .expect("request");
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(decision.outcome, DecisionOutcome::RequiresApproval);
    assert_eq!(decision.approval_id, approval.approval_id);
    assert!(approval.approval_id.starts_with("approval_"));
}

#[test]
fn request_approval_requires_action_and_reason() {
    let engine = Engine::new();
    let mut input = request("calendar.create");
    input.action = String::new();
    assert!(engine.request_approval(input.clone()).is_err());
    let mut input2 = request("calendar.create");
    input2.reason = String::new();
    assert!(engine.request_approval(input2).is_err());
}

#[test]
fn resolve_approval_approves() {
    let engine = Engine::new();
    let (approval, _) = engine
        .request_approval(request("mail.send"))
        .expect("request");
    let (resolved, decision) = engine
        .resolve_approval(
            &approval.approval_id,
            ResolveApprovalInput {
                resolution: "approved".to_string(),
                comment: "ok".to_string(),
            },
        )
        .expect("resolve");
    assert_eq!(resolved.status, ApprovalStatus::Approved);
    assert!(resolved.resolved_at.is_some());
    assert_eq!(decision.outcome, DecisionOutcome::Approved);
}

#[test]
fn resolve_approval_rejects_non_pending_and_invalid() {
    let engine = Engine::new();
    let (approval, _) = engine
        .request_approval(request("mail.send"))
        .expect("request");
    let _ = engine
        .resolve_approval(
            &approval.approval_id,
            ResolveApprovalInput {
                resolution: "approved".to_string(),
                comment: String::new(),
            },
        )
        .expect("resolve once");
    // second resolve on non-pending fails
    assert!(
        engine
            .resolve_approval(
                &approval.approval_id,
                ResolveApprovalInput {
                    resolution: "approved".to_string(),
                    comment: String::new()
                }
            )
            .is_err()
    );
    // invalid resolution
    let (a2, _) = engine
        .request_approval(request("mail.send"))
        .expect("request2");
    assert!(
        engine
            .resolve_approval(
                &a2.approval_id,
                ResolveApprovalInput {
                    resolution: "weird".to_string(),
                    comment: String::new()
                }
            )
            .is_err()
    );
}

#[test]
fn list_approvals_filters_by_status() {
    let engine = Engine::new();
    let (a, _) = engine
        .request_approval(request("calendar.create"))
        .expect("request");
    let _ = engine
        .resolve_approval(
            &a.approval_id,
            ResolveApprovalInput {
                resolution: "approved".to_string(),
                comment: String::new(),
            },
        )
        .expect("resolve");
    assert_eq!(
        engine.list_approvals(Some(ApprovalStatus::Pending)).len(),
        0
    );
    assert_eq!(
        engine.list_approvals(Some(ApprovalStatus::Approved)).len(),
        1
    );
    assert_eq!(engine.list_decisions().len(), 2);
}
