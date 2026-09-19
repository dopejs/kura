//! Stable, tenant-scoped idempotency operation keys (port of
//! `operation_key.go`).

#[must_use]
pub fn run_operation_key(tenant_id: &str, client_key: &str, run_id: &str) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "run",
        &first_non_empty(&[client_key, run_id]),
    ])
}

#[must_use]
pub fn workflow_operation_key(
    tenant_id: &str,
    run_id: &str,
    workflow_id: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "workflow",
        run_id,
        &first_non_empty(&[workflow_id, client_key]),
    ])
}

#[must_use]
pub fn tool_call_operation_key(
    tenant_id: &str,
    run_id: &str,
    step_id: &str,
    tool_call_id: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "tool_call",
        run_id,
        step_id,
        &first_non_empty(&[tool_call_id, client_key]),
    ])
}

#[must_use]
pub fn live_validation_operation_key(
    tenant_id: &str,
    validation_id: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "live_validation",
        &first_non_empty(&[validation_id, client_key]),
    ])
}

#[must_use]
pub fn integration_operation_key(
    tenant_id: &str,
    domain: &str,
    operation_id: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "integration",
        domain,
        &first_non_empty(&[operation_id, client_key]),
    ])
}

#[must_use]
pub fn artifact_operation_key(
    tenant_id: &str,
    artifact_id: &str,
    storage_key: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "artifact",
        &first_non_empty(&[artifact_id, storage_key, client_key]),
    ])
}

#[must_use]
pub fn evaluation_operation_key(
    tenant_id: &str,
    candidate_id: &str,
    attempt_id: &str,
    client_key: &str,
) -> String {
    join_operation_key(&[
        "tenant",
        tenant_id,
        "evaluation",
        candidate_id,
        &first_non_empty(&[attempt_id, client_key]),
    ])
}

fn join_operation_key(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| {
            let value = part.trim();
            let value = if value.is_empty() { "unknown" } else { value };
            value.replace(':', "_")
        })
        .collect::<Vec<_>>()
        .join(":")
}

fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    format!("missing_{}", values.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::CLIENT;
    use crate::fixtures::RUN_ID;
    use crate::fixtures::STEP_ID;
    use crate::fixtures::TEN_FINITE;
    use crate::fixtures::TEN_OTHER;

    #[test]
    fn operation_keys_are_stable_and_tenant_scoped() {
        let cases: [(&str, String, &str); 7] = [
            (
                "run",
                run_operation_key(TEN_FINITE, CLIENT, RUN_ID),
                "tenant:ten_finite:run:client_fixture",
            ),
            (
                "workflow",
                workflow_operation_key(TEN_FINITE, RUN_ID, "workflow_1", CLIENT),
                "tenant:ten_finite:workflow:run_fixture:workflow_1",
            ),
            (
                "tool",
                tool_call_operation_key(TEN_FINITE, RUN_ID, STEP_ID, "tool_1", CLIENT),
                "tenant:ten_finite:tool_call:run_fixture:step_fixture:tool_1",
            ),
            (
                "live",
                live_validation_operation_key(TEN_FINITE, "validation_1", CLIENT),
                "tenant:ten_finite:live_validation:validation_1",
            ),
            (
                "integration",
                integration_operation_key(TEN_FINITE, "mail", "op_1", CLIENT),
                "tenant:ten_finite:integration:mail:op_1",
            ),
            (
                "artifact",
                artifact_operation_key(TEN_FINITE, "", "artifact/key", CLIENT),
                "tenant:ten_finite:artifact:artifact/key",
            ),
            (
                "evaluation",
                evaluation_operation_key(TEN_FINITE, "candidate_1", "attempt_1", CLIENT),
                "tenant:ten_finite:evaluation:candidate_1:attempt_1",
            ),
        ];
        for (name, got, want) in cases {
            assert_eq!(got, want, "{name}");
        }
        assert_ne!(
            run_operation_key(TEN_FINITE, CLIENT, RUN_ID),
            run_operation_key(TEN_OTHER, CLIENT, RUN_ID),
            "operation keys must include tenant identity"
        );
    }
}
