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

use kura_core::{Tool, ToolError, ToolInvocation, ToolOutput};
use kura_llm::ToolSpec;
use serde_json::Value;

use crate::manager::Manager;
use crate::types::{AuthorizeToolInput, ToolAuthorizationStatus};

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
        }
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
                        approval_id: String::new(),
                        requested_by: "agent".to_string(),
                    },
                )
                .map_err(|error| ToolError::Failed(error.to_string()))?;

            match authorization.status {
                ToolAuthorizationStatus::Allowed => {}
                // Said plainly, so the model tells the user what is needed
                // rather than reporting that the tool is broken.
                ToolAuthorizationStatus::Pending => {
                    return Ok(ToolOutput::failed(format!(
                        "{} needs a person to approve it before it can run",
                        self.tool_name
                    )));
                }
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
            }

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
