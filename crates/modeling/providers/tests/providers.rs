//! Behavioral tests ported from the Go providers package (manager_test.go).

use std::sync::Arc;

use kura_config::LlmConfig;
use kura_llm::{Dispatcher, EchoProvider};
use kura_providers::{new_check_id, new_manager};

fn manager_with_echo() -> kura_providers::Manager {
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(EchoProvider::new()));
    new_manager(LlmConfig::default(), Some(dispatcher), vec![])
}

#[test]
fn list_profiles_returns_echo_and_openai() {
    let manager = manager_with_echo();
    let profiles = manager.list_profiles();
    assert_eq!(profiles.len(), 2);
    assert!(profiles.iter().any(|p| p.provider_id == "echo"));
    assert!(
        profiles
            .iter()
            .any(|p| p.provider_id == "openai_compatible")
    );
    // Sorted by provider id.
    assert!(
        profiles
            .windows(2)
            .all(|w| w[0].provider_id <= w[1].provider_id)
    );
}

#[test]
fn resolve_defaults_to_echo() {
    let manager = manager_with_echo();
    let resolved = manager.resolve("", "", 0, 0).expect("resolve");
    assert_eq!(resolved.provider_id, "echo");
    assert_eq!(resolved.model, "echo-v1");
}

#[test]
fn resolve_unknown_provider_errors() {
    let manager = manager_with_echo();
    let err = manager.resolve("nonexistent", "", 0, 0).unwrap_err();
    assert!(err.to_string().contains("provider not found"), "err: {err}");
}

#[test]
fn new_check_id_has_prefix() {
    let id = new_check_id();
    assert!(id.starts_with("provider_check_"), "id: {id}");
}

#[test]
fn set_default_model_rejects_unknown_model() {
    let manager = manager_with_echo();
    // echo is ModelSelectionMode::Fixed with known model "echo-v1".
    let err = manager
        .set_default_model("echo", "unknown-model")
        .unwrap_err();
    assert!(err.to_string().contains("not supported"), "err: {err}");
    assert!(manager.set_default_model("echo", "echo-v1").is_ok());
}
