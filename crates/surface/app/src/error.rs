//! Top-level daemon assembly errors (port of the wrapped errors returned by
//! Go's app.New / app.Run).

use std::io;

/// Errors produced while building, serving, or closing the daemon application.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("load config: {0}")]
    Config(#[from] kura_config::ConfigError),
    #[error("store: {0}")]
    Store(String),
    #[error("plugin profile: {0}")]
    PluginProfile(String),
    #[error("secrets backend: {0}")]
    Secrets(String),
    #[error("skills registry: {0}")]
    Skills(String),
    #[error("bind http listener on {addr}: {source}")]
    Bind { addr: String, source: io::Error },
    #[error("serve http: {0}")]
    Serve(io::Error),
    #[error("connector runtime: {0}")]
    ConnectorRuntime(String),
    #[error("connector runtime start: {0}")]
    ConnectorStart(String),
    #[error("restore persisted state: {0}")]
    Restore(String),
    #[error("scheduler start: {0}")]
    SchedulerStart(String),
    #[error("reminders start: {0}")]
    RemindersStart(String),
    #[error("system event publish: {0}")]
    SystemEvent(String),
    #[error("upgrade rehearsal: {0}")]
    Rehearsal(String),
}
