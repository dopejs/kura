//! `work_item.open`: a chat turn opens a work item (work-items.md §5.1).
//!
//! Offered to ordinary turns only, never to a round (a round runs on its
//! item's own `thr_wi_` thread): a work item cannot spawn work items in
//! v1, and a round's tool must not start a turn while the round holds a
//! slot (§11, deadlock). The first round therefore runs on a thread of its
//! own, admitted there, and the tool returns at once with the item id; the
//! turn's own answer is the acknowledgement.
//!
//! A turn an IM message started opens the item on an `im_direct` or
//! `im_mention` basis with that message's sender, and with
//! `subscribe_origin` (the default) the item owns the conversation: the
//! person's later messages there wake it. Any other turn opens it on the
//! `api` basis.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kura_chat::ToolTurn;
use kura_core::{Tool, ToolError, ToolInvocation, ToolOutput};
use kura_llm::ToolSpec;
use serde_json::json;

use crate::manager::{Manager, OpenRequest};

pub const OPEN_TOOL: &str = "work_item.open";

/// Whether `thread_id` is a work item's own thread, where its rounds run.
#[must_use]
pub fn is_round_thread(thread_id: &str) -> bool {
    thread_id.trim().starts_with("thr_wi_")
}

/// `work_item.open` for one turn.
pub struct OpenTool {
    manager: Arc<Manager>,
    tenant_id: String,
    run_id: String,
}

impl OpenTool {
    /// The tool for `turn`, or `None` when the turn may not open items: it
    /// is a round, or it has no tenant.
    #[must_use]
    pub fn for_turn(manager: Arc<Manager>, turn: &ToolTurn) -> Option<Self> {
        if is_round_thread(&turn.thread_id) || turn.tenant_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            manager,
            tenant_id: turn.tenant_id.trim().to_string(),
            run_id: turn.scope.run_id.trim().to_string(),
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArguments {
    title: String,
    scope: String,
    #[serde(default = "default_true")]
    subscribe_origin: bool,
}

fn default_true() -> bool {
    true
}

impl Tool for OpenTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: OPEN_TOOL.to_string(),
            description: "Open a work item: work that outlives this turn. It runs in rounds on \
                 its own thread, waits between rounds, and goes on when the person who asked \
                 writes again in this conversation. Use it only when the person explicitly asks \
                 for work that has to wait for something (CI, a review, their reply) or take \
                 longer than this answer. `scope` is what they asked for, in their words; the \
                 item may do nothing outside it. Returns the item id; tell the person the item \
                 is open."
                .to_string(),
            parameters: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["title", "scope"],
                "properties": {
                    "title": {"type": "string", "description": "A short name for the work."},
                    "scope": {"type": "string", "description": "What the person asked for, in their own words."},
                    "subscribe_origin": {
                        "type": "boolean",
                        "description": "In a chat conversation, let the person's later messages there wake the item. Default true."
                    }
                }
            }),
        }
    }

    fn call<'a>(
        &'a self,
        invocation: &'a ToolInvocation,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.open(&invocation.arguments)) })
    }
}

impl OpenTool {
    fn open(&self, arguments: &str) -> ToolOutput {
        let args: OpenArguments = match serde_json::from_str(if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        }) {
            Ok(args) => args,
            Err(err) => return ToolOutput::failed(format!("invalid arguments: {err}")),
        };
        let origin = self.manager.turn_origin(&self.tenant_id, &self.run_id);
        let request = OpenRequest {
            tenant_id: self.tenant_id.clone(),
            principal_id: String::new(),
            request_ref: origin
                .as_ref()
                .map(|origin| origin.delivery_id.clone())
                .unwrap_or_default(),
            title: args.title,
            scope: args.scope,
            provider: String::new(),
            model: String::new(),
            subscribe_origin: args.subscribe_origin,
            origin,
        };
        let subscribed = request.subscribe_origin
            && request.origin.as_ref().is_some_and(|origin| {
                !origin.source_key.is_empty() && !origin.author_id.is_empty()
            });
        match self.manager.open(request) {
            Ok((item, turn)) => {
                self.manager.spawn_first_round(turn);
                ToolOutput::ok(
                    json!({
                        "itemId": item.item_id,
                        "title": item.title,
                        "basis": item.mandate.basis.as_str(),
                        "stage": item.stage.map(|stage| stage.as_str()),
                        "subscribed": subscribed,
                    })
                    .to_string(),
                )
            }
            Err(err) => ToolOutput::failed(format!("{} ({})", err, err.code())),
        }
    }
}
