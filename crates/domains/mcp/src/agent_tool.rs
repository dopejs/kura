//! MCP tools, as tools the agent loop can call.
//!
//! The loop in `kura-core` calls anything that implements its `Tool` trait.
//! MCP already knows how to reach a server, what it publishes, and whether a
//! given surface is allowed to invoke it. This is the adapter between the two,
//! so a tool a user connected over MCP is a tool the model can use.
//!
//! Authorization is not bypassed here, and cannot be. Every call goes through
//! `authorize_tool` first, and a tool whose exposure rule says
//! `approval_required` comes back `Pending` -- reported to the model as
//! refused rather than run. That is the boundary a state-changing tool needs:
//! the model may ask, and a person decides.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kura_core::{Tool, ToolError, ToolInvocation, ToolOutput};
use kura_llm::ToolSpec;
use serde_json::Value;

use crate::manager::Manager;
use crate::types::{AuthorizeToolInput, ToolAuthorizationResponse, ToolAuthorizationStatus};

/// How long a tool waits for a person to answer before giving up.
///
/// The person is usually right there -- the model has just said it needs their
/// approval -- so this is about how long a turn may sit open, not how long
/// someone might take to notice. Giving up hands the turn back with the reason,
/// and the approval stays pending for the next attempt.
const APPROVAL_WAIT: Duration = Duration::from_secs(180);

/// How often the pending approval is re-read while waiting.
const APPROVAL_POLL: Duration = Duration::from_millis(250);

/// The name a tool is offered to the model under.
///
/// Qualified by server, because two servers may publish the same tool name and
/// the model has to be able to say which one it means.
#[must_use]
pub fn qualified_name(server_id: &str, tool_name: &str) -> String {
    format!("{server_id}__{tool_name}")
}

/// An MCP tool the agent loop can call.
pub struct McpTool {
    manager: Arc<Manager>,
    server_id: String,
    tool_name: String,
    description: String,
    parameters: Value,
    /// Which surface is asking. Exposure rules are per surface, so a tool may
    /// be allowed in chat and blocked in a scheduled run.
    runtime_surface: String,
    /// How long to wait for a person to answer an approval.
    approval_wait: Duration,
}

impl McpTool {
    #[must_use]
    pub fn new(
        manager: Arc<Manager>,
        server_id: impl Into<String>,
        tool_name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        runtime_surface: impl Into<String>,
    ) -> Self {
        Self {
            manager,
            server_id: server_id.into(),
            tool_name: tool_name.into(),
            description: description.into(),
            parameters,
            runtime_surface: runtime_surface.into(),
            approval_wait: APPROVAL_WAIT,
        }
    }

    /// How long this tool waits for an approval. A surface nobody is watching
    /// wants a shorter one than a chat someone is sitting in front of.
    #[must_use]
    pub fn with_approval_wait(mut self, wait: Duration) -> Self {
        self.approval_wait = wait;
        self
    }
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: qualified_name(&self.server_id, &self.tool_name),
            description: self.description.clone(),
            // An object with no properties, when the server published nothing.
            // Sending `null` would leave a provider to guess the shape.
            parameters: if self.parameters.is_null() {
                serde_json::json!({"type": "object", "properties": {}})
            } else {
                self.parameters.clone()
            },
        }
    }

    fn call<'a>(
        &'a self,
        invocation: &'a ToolInvocation,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // Arguments arrive as the model wrote them. Malformed JSON is the
            // model's mistake to correct, so it is reported as a failed call
            // rather than raised: the turn continues and it can try again.
            let arguments: Value = if invocation.arguments.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                match serde_json::from_str(&invocation.arguments) {
                    Ok(value) => value,
                    Err(error) => {
                        return Ok(ToolOutput::failed(format!(
                            "arguments are not valid JSON: {error}"
                        )));
                    }
                }
            };

            let authorization = self
                .manager
                .authorize_tool(
                    &self.server_id,
                    &self.tool_name,
                    &AuthorizeToolInput {
                        runtime_surface: self.runtime_surface.clone(),
                        // What the person will be shown. Without it the
                        // approval names a tool and nothing else.
                        arguments: invocation.arguments.clone(),
                        approval_id: String::new(),
                        requested_by: "agent".to_string(),
                    },
                )
                .map_err(|error| ToolError::Failed(error.to_string()))?;

            let authorization = match authorization.status {
                ToolAuthorizationStatus::Allowed => authorization,
                // Someone has to say yes. The approval was raised by the call
                // above and an event announced it; this waits for the answer
                // rather than failing the turn, because the person is being
                // asked right now and a turn that ended here would make them
                // start it over.
                ToolAuthorizationStatus::Pending => match self.await_approval(&authorization).await {
                    Some(granted) => granted,
                    None => {
                        return Ok(ToolOutput::failed(format!(
                            "{} was not run: it needs a person to approve it, and none did",
                            self.tool_name
                        )));
                    }
                },
                ToolAuthorizationStatus::Rejected | ToolAuthorizationStatus::Blocked => {
                    let reason = if authorization.message.is_empty() {
                        "it is not available on this surface".to_string()
                    } else {
                        authorization.message.clone()
                    };
                    return Ok(ToolOutput::failed(format!(
                        "{} was not run: {reason}",
                        self.tool_name
                    )));
                }
            };

            let result = self
                .manager
                .call_tool(&self.server_id, &self.tool_name, arguments, &authorization)
                .map_err(|error| ToolError::Failed(error.to_string()))?;

            if !result.error.is_empty() {
                // The server answered with a failure. That is an answer, and
                // the model can act on it.
                return Ok(ToolOutput::failed(result.error));
            }
            let output = match result.output {
                Some(Value::String(text)) => text,
                Some(value) => value.to_string(),
                None => String::new(),
            };
            Ok(ToolOutput::ok(output))
        })
    }
}

impl McpTool {
    /// Wait for a person to answer the approval this call raised.
    ///
    /// Returns the second authorization -- the one carrying the approval id --
    /// when it was granted, and `None` when it was refused or nobody answered
    /// in time.
    ///
    /// The second authorization is not a formality: a rule can change while a
    /// person is deciding, and the runtime is asked again with the approval in
    /// hand. It is not the only thing between a stale grant and a call,
    /// though -- `call_tool` refuses anything that is not `Allowed` whatever it
    /// is handed, so dropping the check here changes the message the model gets
    /// and not whether the tool runs. Defence in depth, said plainly so nobody
    /// reads it as the guard.
    async fn await_approval(
        &self,
        pending: &ToolAuthorizationResponse,
    ) -> Option<ToolAuthorizationResponse> {
        let approval_id = pending.approval.as_ref()?.approval_id.clone();
        if approval_id.is_empty() {
            return None;
        }
        let deadline = std::time::Instant::now() + self.approval_wait;
        loop {
            match self.manager.approval(&approval_id) {
                // Answered yes. Ask again with the id, and let the runtime say
                // whether that is still enough.
                Some(approval) if approval.status == kura_policy::ApprovalStatus::Approved => {
                    let granted = self
                        .manager
                        .authorize_tool(
                            &self.server_id,
                            &self.tool_name,
                            &AuthorizeToolInput {
                                runtime_surface: self.runtime_surface.clone(),
                                arguments: String::new(),
                                approval_id: approval_id.clone(),
                                requested_by: "agent".to_string(),
                            },
                        )
                        .ok()?;
                    return (granted.status == ToolAuthorizationStatus::Allowed).then_some(granted);
                }
                // Answered no, or withdrawn. Either way there is nothing to
                // wait for.
                Some(approval)
                    if approval.status != kura_policy::ApprovalStatus::Pending =>
                {
                    return None;
                }
                // Still pending, or gone. A vanished approval is not something
                // that will resolve.
                Some(_) => {}
                None => return None,
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(APPROVAL_POLL).await;
        }
    }
}

/// Every tool the connected servers publish, as tools the loop can call.
///
/// Built from what discovery already recorded rather than by reaching out: a
/// server that is down should not stop a turn from starting, and its tools are
/// simply not offered until it is back.
#[must_use]
pub fn tools_for_surface(manager: &Arc<Manager>, runtime_surface: &str) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for server in manager.list_servers() {
        let server_id = server.server.server_id.clone();
        let Ok(published) = manager.list_tools(&server_id) else {
            continue;
        };
        for resource in published {
            tools.push(Arc::new(McpTool::new(
                Arc::clone(manager),
                server_id.clone(),
                resource.tool.tool_name.clone(),
                resource.tool.description.clone(),
                resource.tool.input_schema.clone(),
                runtime_surface,
            )));
        }
    }
    tools
}
