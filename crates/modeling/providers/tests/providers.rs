//! Behavioral tests ported from the Go providers package (manager_test.go).

use std::sync::Arc;

use kura_config::LlmConfig;
use kura_llm::{Dispatcher, EchoProvider};
use kura_providers::{new_check_id, new_manager};

/// A manager with echo asked for by name.
///
/// Nothing is listed until something is configured, so a test that wants echo
/// has to say so. These used to rely on the inventory being seeded regardless
/// of configuration, which is what made a fresh daemon list several providers
/// its owner had never set up.
fn manager_with_echo() -> kura_providers::Manager {
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(EchoProvider::new()));
    let cfg = LlmConfig { default_provider: "echo".to_string(), ..LlmConfig::default() };
    new_manager(cfg, Some(dispatcher), vec![])
}

#[test]
fn only_what_was_configured_is_listed() {
    let manager = manager_with_echo();
    let profiles = manager.list_profiles();
    // Echo was asked for; the HTTP endpoint was not configured, so it is
    // absent rather than listed with faults nobody can clear.
    assert_eq!(
        profiles.iter().map(|p| p.provider_id.as_str()).collect::<Vec<_>>(),
        vec!["echo"]
    );
}

#[test]
fn an_unconfigured_manager_lists_nothing() {
    let dispatcher = Arc::new(Dispatcher::new());
    let manager = new_manager(LlmConfig::default(), Some(dispatcher), vec![]);

    // Empty, not broken. A daemon nobody has set up should read as one.
    assert!(manager.list_profiles().is_empty());
}

#[test]
fn a_configured_endpoint_is_listed() {
    let cfg = LlmConfig {
        openai_compatible: kura_config::OpenAiCompatibleProviderConfig {
            base_url: "https://api.example.test/v1".to_string(),
            model: "a-model".to_string(),
            ..Default::default()
        },
        ..LlmConfig::default()
    };
    let manager = new_manager(cfg, Some(Arc::new(Dispatcher::new())), vec![]);

    assert_eq!(
        manager.list_profiles().iter().map(|p| p.provider_id.as_str()).collect::<Vec<_>>(),
        vec!["openai_compatible"]
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
    let err = manager.set_default_model("echo", "unknown-model").unwrap_err();
    assert!(err.to_string().contains("not supported"), "err: {err}");
    assert!(manager.set_default_model("echo", "echo-v1").is_ok());
}

#[test]
fn an_account_configured_before_startup_is_listed() {
    // The accounts a user configured are handed to the daemon at startup. They
    // were registered with the dispatcher and then left out of the inventory,
    // so a request reached the vendor while the settings page said no provider
    // existed -- and the default provider named one that was not in the list.
    let cfg = LlmConfig {
        default_provider: "anthropic".to_string(),
        accounts: vec![kura_config::AccountProviderConfig {
            id: "anthropic".to_string(),
            title: "Anthropic".to_string(),
            protocol: kura_config::AccountProtocol::AnthropicMessages,
            base_url: "https://api.anthropic.test".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            access_token: "token".to_string(),
            ..Default::default()
        }],
        ..LlmConfig::default()
    };
    let manager = new_manager(cfg, Some(Arc::new(Dispatcher::new())), vec![]);

    let profiles = manager.list_profiles();
    assert_eq!(
        profiles.iter().map(|p| p.provider_id.as_str()).collect::<Vec<_>>(),
        vec!["anthropic"]
    );
    assert!(profiles[0].default, "the configured default must be the one listed");
}

#[test]
fn an_account_added_while_running_joins_the_configured_ones() {
    // Both lists are the same list. Adding one at runtime used to replace the
    // startup set in the eyes of `load_profiles`, because only one of the two
    // was ever read.
    let cfg = LlmConfig {
        accounts: vec![kura_config::AccountProviderConfig {
            id: "anthropic".to_string(),
            base_url: "https://api.anthropic.test".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            access_token: "token".to_string(),
            ..Default::default()
        }],
        ..LlmConfig::default()
    };
    let manager = new_manager(cfg, Some(Arc::new(Dispatcher::new())), vec![]);

    manager.upsert_account(kura_config::AccountProviderConfig {
        id: "zhipu".to_string(),
        base_url: "https://open.bigmodel.test/api/paas/v4".to_string(),
        model: "glm-4".to_string(),
        access_token: "token".to_string(),
        ..Default::default()
    });

    assert_eq!(
        manager.list_profiles().iter().map(|p| p.provider_id.as_str()).collect::<Vec<_>>(),
        vec!["anthropic", "zhipu"]
    );
}
