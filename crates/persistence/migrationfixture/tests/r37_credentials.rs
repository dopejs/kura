//! r37 credential-boundary fixture: local credential files + provider /
//! integration / connector / MCP state seeded through the store CRUD.

mod common;

use common::head_store;
use kura_migrationfixture::{R37_FAKE_SECRET_TENANT_A, seed_r37_local_credential_state};

#[test]
fn r37_credential_files_are_written_with_expected_values() {
    let (store, dir) = head_store("r37_files");
    let fixture = seed_r37_local_credential_state(&store, &dir).unwrap();

    assert_eq!(
        fixture.mcp_secret_refs,
        vec!["R37_MCP_TOKEN", "R37_SHARED_TOKEN", "R37_CONFLICT_TOKEN"]
    );
    assert_eq!(
        fixture.skill_secret_refs,
        vec!["R37_SKILL_TOKEN", "R37_SHARED_TOKEN", "R37_CONFLICT_TOKEN"]
    );
    assert_eq!(fixture.conflict_ref, "R37_CONFLICT_TOKEN");

    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/mcp-secrets.json")).unwrap())
            .unwrap();
    assert_eq!(mcp["R37_MCP_TOKEN"], R37_FAKE_SECRET_TENANT_A);
    assert_eq!(mcp["R37_SHARED_TOKEN"], "shared-r37-value");
    assert_eq!(mcp["R37_CONFLICT_TOKEN"], "mcp-side");

    let skill: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{dir}/skill-secrets.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(skill["R37_SKILL_TOKEN"], "skill-r37-value");
    assert_eq!(skill["R37_SHARED_TOKEN"], "shared-r37-value");
    assert_eq!(skill["R37_CONFLICT_TOKEN"], "skill-side");
}

#[test]
fn r37_store_rows_load_back_through_the_domain_crud() {
    let (store, dir) = head_store("r37_crud");
    let fixture = seed_r37_local_credential_state(&store, &dir).unwrap();

    // Provider auth state.
    let states = store.list_provider_auth_states().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].provider_id, fixture.provider_id);
    assert_eq!(states[0].status, kura_providers::AuthStatus::Authenticated);
    assert_eq!(states[0].family, kura_providers::Family::OpenAICompatible);
    assert_eq!(states[0].auth_mode, kura_providers::AuthMode::ApiKey);
    assert_eq!(
        states[0].metadata.get("source").map(String::as_str),
        Some("r37_migration_fixture")
    );

    // Integration resource (seeded via CRUD; pre-tenant fixture already added
    // two integrations, so match by id).
    let integrations = store.list_integrations("test").unwrap();
    let r37_integration = integrations
        .iter()
        .find(|item| item.integration_id == fixture.integration_id)
        .expect("r37 integration present");
    assert_eq!(r37_integration.domain_kind, "calendar");
    assert_eq!(
        r37_integration.readiness_status,
        kura_integrations::ReadinessStatus::Healthy
    );
    assert_eq!(
        r37_integration
            .account_binding
            .as_ref()
            .map(|binding| binding.account_key.as_str()),
        Some("r37-legacy@example.com")
    );
    assert_eq!(
        r37_integration.backend_binding.backend_kind,
        kura_integrations::BackendKind::ManagedProvider
    );
    assert_eq!(
        r37_integration.backend_binding.backend_ref_id,
        fixture.provider_id
    );

    // Connector with the conflicting secret ref (never the raw material).
    let connectors = store.list_connectors().unwrap();
    assert_eq!(connectors.len(), 1);
    assert_eq!(connectors[0].connector_id, fixture.connector_id);
    assert_eq!(connectors[0].kind, "discord");
    assert_eq!(connectors[0].status, kura_connectors::Status::Healthy);
    assert_eq!(
        connectors[0].secret_refs,
        vec![fixture.conflict_ref.clone()]
    );
    assert!(
        !connectors[0]
            .secret_refs
            .iter()
            .any(|r| r.contains("DO_NOT_LEAK"))
    );

    // MCP server + state + tool + exposure rule.
    let servers = store.list_mcp_servers().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].server_id, fixture.mcp_server_id);
    assert!(servers[0].enabled);
    assert!(servers[0].document.contains("R37 Legacy MCP"));

    let server_states = store.list_mcp_server_states().unwrap();
    assert_eq!(server_states.len(), 1);
    assert_eq!(server_states[0].server_id, fixture.mcp_server_id);
    assert_eq!(server_states[0].status, "healthy");

    let tools = store.list_mcp_tools(&fixture.mcp_server_id).unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, fixture.mcp_tool_name);
    assert_eq!(tools[0].discovery_status, "discovered");
    assert!(tools[0].document.contains("lookup"));

    let rules = store
        .list_mcp_tool_exposure_rules(&fixture.mcp_server_id)
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].runtime_surface, "chat");
    assert_eq!(rules[0].exposure_mode, "allow");
    assert!(rules[0].active);
}
