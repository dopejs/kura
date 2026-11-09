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
    /// The server declared the tool read-only (`readOnlyHint`).
    read_only: bool,
    /// What the turn remembers about approvals (a work item's round): a
    /// call it holds an approval for waits on, or runs under, that approval
    /// instead of raising another. `None` for an ordinary turn.
    turn_approvals: Option<Arc<dyn kura_policy::TurnApprovals>>,
}

/// What an approval for a call of `tool_name` on `server_id` and
/// `runtime_surface` is raised for: the same action, resource kind and
/// resource id `Manager::authorize_tool` gives it.
fn approval_call<'a>(resource_id: &'a str, arguments: &'a str) -> kura_policy::ApprovalCall<'a> {
    kura_policy::ApprovalCall {
        action: TOOL_CALL_ACTION,
        resource_kind: crate::RESOURCE_KIND_TOOL,
        resource_id,
        arguments,
    }
}

/// The action an MCP tool's approval is for (`Manager::authorize_tool`).
const TOOL_CALL_ACTION: &str = "tool_call.execute";

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
            read_only: false,
            turn_approvals: None,
        }
    }

    /// What the turn remembers about approvals (work-items.md §7). With
    /// one, a call the turn already holds a pending or approved approval
    /// for waits on, or runs under, that approval instead of raising a new
    /// one; a call that gives up waiting tells it, so the turn's owner can
    /// wait for the approval.
    #[must_use]
    pub fn with_turn_approvals(
        mut self,
        turn_approvals: Option<Arc<dyn kura_policy::TurnApprovals>>,
    ) -> Self {
        self.turn_approvals = turn_approvals;
        self
    }

    /// Marks the tool as declared read-only by its server, which lets the
    /// agent loop run it beside other such calls when it also needs no
    /// approval.
    #[must_use]
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
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

    /// Declared read-only by its server **and** allowed on this surface
    /// without approval. The second half is what keeps a parallel batch from
    /// starting beside a call that stops to ask a person.
    fn parallel_safe(&self, _invocation: &ToolInvocation) -> bool {
        self.read_only
            && self.manager.runs_without_approval(
                &self.server_id,
                &self.tool_name,
                &self.runtime_surface,
            )
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

            let resource_id = format!(
                "{}:{}:{}",
                self.server_id.trim(),
                self.tool_name.trim(),
                self.runtime_surface.trim()
            );
            let call = approval_call(&resource_id, &invocation.arguments);

            // An approval the turn already holds for exactly this call (a
            // work item's round, coming back for the call a person was
            // asked about): run under it, or wait on it, rather than raise a
            // second one.
            if let Some(held) = self.held_approval(&call) {
                let granted = match held.status {
                    kura_policy::ApprovalStatus::Approved => self.authorize_with(&held.approval_id),
                    _ => self.await_approval(&held.approval_id).await,
                };
                let Some(granted) = granted else {
                    return Ok(self.not_approved(&call, &held.approval_id, None));
                };
                if let Some(turn_approvals) = &self.turn_approvals {
                    turn_approvals.used(&held.approval_id);
                }
                return self.run(arguments, granted).await;
            }

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
                ToolAuthorizationStatus::Pending => {
                    let approval_id = authorization
                        .approval
                        .as_ref()
                        .map(|approval| approval.approval_id.clone())
                        .unwrap_or_default();
                    match self.await_approval(&approval_id).await {
                        Some(granted) => {
                            if let Some(turn_approvals) = &self.turn_approvals {
                                turn_approvals.used(&approval_id);
                            }
                            granted
                        }
                        None => {
                            return Ok(self.not_approved(
                                &call,
                                &approval_id,
                                authorization.decision.as_ref(),
                            ));
                        }
                    }
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
            };
            self.run(arguments, authorization).await
        })
    }
}

impl McpTool {
    /// Runs the call under `authorization`, which allowed it.
    async fn run(
        &self,
        arguments: Value,
        authorization: ToolAuthorizationResponse,
    ) -> Result<ToolOutput, ToolError> {
        {
            // `call_tool` blocks on the server's reply. Off the async
            // worker, or it would stall every other future on it — including
            // the calls running beside this one (2.6).
            let result = {
                let manager = Arc::clone(&self.manager);
                let server_id = self.server_id.clone();
                let tool_name = self.tool_name.clone();
                // A blocking-pool thread does not inherit the task-local
                // tenant context the sandbox's tenancy checks read.
                let tenant = kura_identity::tenantctx::from_context();
                let run = move || {
                    let call =
                        || manager.call_tool(&server_id, &tool_name, arguments, &authorization);
                    match tenant {
                        Some(tenant) => kura_identity::tenantctx::with_context(tenant, call),
                        None => call(),
                    }
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => handle.spawn_blocking(run).await.map_err(|error| {
                        ToolError::Failed(format!("tool call aborted: {error}"))
                    })?,
                    Err(_) => run(),
                }
            }
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
        }
    }

    /// The approval the turn holds for `call`, as the engine has it now,
    /// when it is still one to wait on or run under.
    fn held_approval(&self, call: &kura_policy::ApprovalCall<'_>) -> Option<kura_policy::Approval> {
        let approval_id = self.turn_approvals.as_ref()?.held(call)?;
        let approval = self.manager.approval(&approval_id)?;
        // The engine's record must be for this very call: a memory that
        // handed back another call's approval must not let this one run.
        let same_call = approval.action == call.action
            && approval.resource_kind == call.resource_kind
            && approval.resource_id == call.resource_id;
        let usable = matches!(
            approval.status,
            kura_policy::ApprovalStatus::Pending | kura_policy::ApprovalStatus::Approved
        );
        (same_call && usable).then_some(approval)
    }

    /// What the model reads when the call was not approved: refused, or
    /// nobody answered in time. In the second case the approval is still
    /// pending and the turn's memory is told, so its owner can wait for it.
    fn not_approved(
        &self,
        call: &kura_policy::ApprovalCall<'_>,
        approval_id: &str,
        decision: Option<&kura_policy::Decision>,
    ) -> ToolOutput {
        let pending = self
            .manager
            .approval(approval_id)
            .filter(|approval| approval.status == kura_policy::ApprovalStatus::Pending);
        let Some(pending) = pending else {
            return ToolOutput::failed(format!(
                "{} was not run: it was not approved, or is no longer allowed",
                self.tool_name
            ));
        };
        let mut message = format!(
            "{} was not run: it needs a person to approve it, and none did",
            self.tool_name
        );
        if let Some(note) = self
            .turn_approvals
            .as_ref()
            .and_then(|turn_approvals| turn_approvals.gave_up(call, &pending, decision))
        {
            message.push_str(". ");
            message.push_str(note.trim());
        }
        ToolOutput::failed(message)
    }

    /// The authorization for a call under the approved `approval_id`, if
    /// the runtime still allows it.
    fn authorize_with(&self, approval_id: &str) -> Option<ToolAuthorizationResponse> {
        let granted = self
            .manager
            .authorize_tool(
                &self.server_id,
                &self.tool_name,
                &AuthorizeToolInput {
                    runtime_surface: self.runtime_surface.clone(),
                    arguments: String::new(),
                    approval_id: approval_id.to_string(),
                    requested_by: "agent".to_string(),
                },
            )
            .ok()?;
        (granted.status == ToolAuthorizationStatus::Allowed).then_some(granted)
    }

    /// Wait for a person to answer the approval `approval_id`.
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
    async fn await_approval(&self, approval_id: &str) -> Option<ToolAuthorizationResponse> {
        if approval_id.is_empty() {
            return None;
        }
        let deadline = std::time::Instant::now() + self.approval_wait;
        loop {
            match self.manager.approval(approval_id) {
                // Answered yes. Ask again with the id, and let the runtime say
                // whether that is still enough.
                Some(approval) if approval.status == kura_policy::ApprovalStatus::Approved => {
                    return self.authorize_with(approval_id);
                }
                // Answered no, or withdrawn. Either way there is nothing to
                // wait for.
                Some(approval) if approval.status != kura_policy::ApprovalStatus::Pending => {
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
///
/// This offers every tenant's servers. A turn that acts for a tenant wants
/// [`tools_for_tenant`].
#[must_use]
pub fn tools_for_surface(manager: &Arc<Manager>, runtime_surface: &str) -> Vec<Arc<dyn Tool>> {
    collect_tools(manager, manager.list_servers(), runtime_surface, None)
}

/// The tools a turn acting for `tenant_id` may be offered: its own tenant's
/// servers, plus servers no tenant owns when `include_unowned` is set (servers
/// created before servers recorded their tenant, or with identity off).
///
/// Another tenant's servers are never offered. Their exposure rules are that
/// tenant's to set, and a call would run on their server. An empty tenant id
/// means the turn has no tenant (identity not configured) and keeps today's
/// behavior: every server.
#[must_use]
pub fn tools_for_tenant(
    manager: &Arc<Manager>,
    tenant_id: &str,
    runtime_surface: &str,
    include_unowned: bool,
) -> Vec<Arc<dyn Tool>> {
    tools_for_tenant_with_approvals(manager, tenant_id, runtime_surface, include_unowned, None)
}

/// [`tools_for_tenant`] for a turn that remembers its approvals (a work
/// item's round, work-items.md §7): every tool is built
/// [`McpTool::with_turn_approvals`].
#[must_use]
pub fn tools_for_tenant_with_approvals(
    manager: &Arc<Manager>,
    tenant_id: &str,
    runtime_surface: &str,
    include_unowned: bool,
    turn_approvals: Option<Arc<dyn kura_policy::TurnApprovals>>,
) -> Vec<Arc<dyn Tool>> {
    let tenant_id = tenant_id.trim();
    if tenant_id.is_empty() {
        return collect_tools(
            manager,
            manager.list_servers(),
            runtime_surface,
            turn_approvals,
        );
    }
    let servers = manager
        .list_servers()
        .into_iter()
        .filter(|server| {
            let owner = server.server.tenant_id.trim();
            owner == tenant_id || (include_unowned && owner.is_empty())
        })
        .collect();
    collect_tools(manager, servers, runtime_surface, turn_approvals)
}

fn collect_tools(
    manager: &Arc<Manager>,
    servers: Vec<crate::ServerResource>,
    runtime_surface: &str,
    turn_approvals: Option<Arc<dyn kura_policy::TurnApprovals>>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for server in servers {
        let server_id = server.server.server_id.clone();
        let Ok(published) = manager.list_tools(&server_id) else {
            continue;
        };
        for resource in published {
            tools.push(Arc::new(
                McpTool::new(
                    Arc::clone(manager),
                    server_id.clone(),
                    resource.tool.tool_name.clone(),
                    resource.tool.description.clone(),
                    resource.tool.input_schema.clone(),
                    runtime_surface,
                )
                .with_read_only(resource.tool.read_only_hint)
                .with_turn_approvals(turn_approvals.clone()),
            ));
        }
    }
    tools
}
