//! Port of the daemon/internal/events envelope types (bus.go) plus the domain
//! event emitters (agent_profiles, billing, connector_*, thread_*, live_validation,
//! integration_diagnostics, workspace_capability_bindings, etc.). The Bus is the
//! in-process fan-out layer; each domain module is a pure Event-builder mirroring the
//! corresponding Go file (category/name/scope/resource/payload byte-identical).

mod agent_profiles;
mod billing;
mod channel_management;
mod connector_delivery;
mod connector_diagnostics;
mod connector_discord;
mod connector_ingress;
mod connector_matrix;
mod connector_slack;
mod connector_telegram;
mod connectors;
mod evaluation_product;
mod integration_diagnostics;
mod live_validation;
mod thread_continuity;
mod thread_group_room;
mod thread_lifecycle;
mod util;
mod wire;
mod workspace_capability_bindings;

pub use agent_profiles::*;
pub use billing::*;
pub use channel_management::*;
pub use connector_delivery::*;
pub use connector_diagnostics::*;
pub use connector_discord::*;
pub use connector_ingress::*;
pub use connector_matrix::*;
pub use connector_slack::*;
pub use connector_telegram::*;
pub use connectors::*;
pub use evaluation_product::*;
pub use integration_diagnostics::*;
pub use live_validation::*;
pub use thread_continuity::*;
pub use thread_group_room::*;
pub use thread_lifecycle::*;
pub use workspace_capability_bindings::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event categories owned by no tenant (the events table is the only mixed table in the
/// inventory). Rows in these categories MUST keep tenant_id NULL.
pub const GLOBAL_CATEGORIES: [&str; 6] = [
    "mcp",
    "provider",
    "system",
    "daemon.migration",
    "connector_global",
    "capability_global",
];

/// Reports whether the given category is in the hard-coded global set.
#[must_use]
pub fn is_global_category(category: &str) -> bool {
    GLOBAL_CATEGORIES.contains(&category)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_action_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capability_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub event_id: String,
    pub sequence: i64,
    /// Not serialized: carried out-of-band by the store/tenancy layer.
    #[serde(skip)]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub category: String,
    pub name: String,
    pub occurred_at: DateTime<Utc>,
    pub scope: Scope,
    pub resource: Resource,
    pub payload: serde_json::Map<String, serde_json::Value>,
}

impl Default for Event {
    fn default() -> Self {
        Event {
            event_id: String::new(),
            sequence: 0,
            environment_scope: String::new(),
            tenant_id: String::new(),
            category: String::new(),
            name: String::new(),
            occurred_at: Utc::now(),
            scope: Scope::default(),
            resource: Resource::default(),
            payload: serde_json::Map::new(),
        }
    }
}

/// Subscription filter on the bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub environment_scope: String,
    pub category: String,
    pub run_id: String,
    pub session_id: String,
    pub schedule_id: String,
    pub schedule_attempt_id: String,
    pub resource_kind: String,
    pub cursor: i64,
    pub tenant_owned_tenant_id: String,
    pub include_global: bool,
}

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

use parking_lot::RwLock;

fn new_event_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("evt_{}", &hex[..16])
}

#[must_use]
fn matches(filter: &Filter, event: &Event) -> bool {
    if filter.cursor > 0 && event.sequence <= filter.cursor {
        return false;
    }
    if !filter.environment_scope.is_empty() && filter.environment_scope != event.environment_scope {
        return false;
    }
    if !filter.category.is_empty() && filter.category != event.category {
        return false;
    }
    if !filter.run_id.is_empty() && filter.run_id != event.scope.run_id {
        return false;
    }
    if !filter.session_id.is_empty() && filter.session_id != event.scope.session_id {
        return false;
    }
    if !filter.schedule_id.is_empty() && filter.schedule_id != event.scope.schedule_id {
        return false;
    }
    if !filter.schedule_attempt_id.is_empty()
        && filter.schedule_attempt_id != event.scope.schedule_attempt_id
    {
        return false;
    }
    if !filter.resource_kind.is_empty() && filter.resource_kind != event.resource.kind {
        return false;
    }
    if !filter.tenant_owned_tenant_id.is_empty() {
        if event.tenant_id != filter.tenant_owned_tenant_id {
            return false;
        }
    } else if filter.include_global && !event.tenant_id.is_empty() {
        return false;
    }
    true
}

struct Subscriber {
    filter: Filter,
    sender: SyncSender<Event>,
}

struct BusState {
    history: Vec<Event>,
    subscribers: HashMap<i64, Subscriber>,
    next_id: i64,
    next_seq: i64,
    closed: bool,
}

impl Default for BusState {
    fn default() -> Self {
        BusState {
            history: Vec::new(),
            subscribers: HashMap::new(),
            next_id: 0,
            next_seq: 0,
            closed: false,
        }
    }
}

/// In-process event bus: an append-only history plus filtered live subscribers. The bus is
/// cloneable (shared via Arc) and safe for use across threads.
#[derive(Clone, Default)]
pub struct Bus {
    state: Arc<RwLock<BusState>>,
}

impl Bus {
    #[must_use]
    pub fn new() -> Self {
        Bus {
            state: Arc::new(RwLock::new(BusState::default())),
        }
    }

    /// Publishes an event to the history and all matching subscribers. Returns the event with
    /// its generated id and sequence filled in.
    pub fn publish(&self, mut event: Event) -> Event {
        let (subs, seq_assigned) = {
            let mut state = self.state.write();
            if event.event_id.is_empty() {
                event.event_id = new_event_id();
            }
            if event.occurred_at == chrono::DateTime::<Utc>::MIN_UTC {
                event.occurred_at = Utc::now();
            }
            if event.payload.is_empty() {
                event.payload = serde_json::Map::new();
            }
            if event.sequence == 0 {
                state.next_seq += 1;
                event.sequence = state.next_seq;
            } else if event.sequence > state.next_seq {
                state.next_seq = event.sequence;
            }
            state.history.push(event.clone());
            let subs: Vec<SyncSender<Event>> = state
                .subscribers
                .values()
                .filter(|sub| matches(&sub.filter, &event))
                .map(|sub| sub.sender.clone())
                .collect();
            (subs, event.sequence)
        };
        let _ = seq_assigned;
        for sender in subs {
            let _ = sender.try_send(event.clone());
        }
        event
    }

    /// Returns the events matching the filter (history only).
    #[must_use]
    pub fn list(&self, filter: &Filter) -> Vec<Event> {
        let state = self.state.read();
        state
            .history
            .iter()
            .filter(|e| matches(filter, e))
            .cloned()
            .collect()
    }

    /// Subscribes to live events matching the filter. Returns a receiver and an unsubscribe
    /// handle; dropping the handle (or calling `unsubscribe`) removes the subscription.
    pub fn subscribe(&self, filter: Filter) -> (Receiver<Event>, Unsubscribe) {
        let mut state = self.state.write();
        if state.closed {
            let (sender, receiver) = mpsc::sync_channel(16);
            drop(sender);
            return (
                receiver,
                Unsubscribe {
                    state: Arc::clone(&self.state),
                    id: -1,
                },
            );
        }
        let id = state.next_id;
        state.next_id += 1;
        let (sender, receiver) = mpsc::sync_channel(16);
        state.subscribers.insert(id, Subscriber { filter, sender });
        (
            receiver,
            Unsubscribe {
                state: Arc::clone(&self.state),
                id,
            },
        )
    }

    /// Closes the bus, dropping all subscribers and closing their channels.
    pub fn close(&self) {
        let mut state = self.state.write();
        if state.closed {
            return;
        }
        state.closed = true;
        state.subscribers.clear();
    }
}

/// Subscription handle returned by `Bus::subscribe`. Removing it unsubscribes.
pub struct Unsubscribe {
    state: Arc<RwLock<BusState>>,
    id: i64,
}

impl Unsubscribe {
    pub fn unsubscribe(self) {
        if self.id < 0 {
            return;
        }
        self.state.write().subscribers.remove(&self.id);
    }
}

impl Drop for Unsubscribe {
    fn drop(&mut self) {
        if self.id < 0 {
            return;
        }
        self.state.write().subscribers.remove(&self.id);
    }
}
