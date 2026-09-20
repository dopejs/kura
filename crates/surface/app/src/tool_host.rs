//! Stage 9.0b/9.2/9.3/9.5: the assembly's [`kura_chat::ToolSource`].
//!
//! The agent loop lives in `kura-core` and calls anything that implements its
//! `Tool` trait; the chat service asks a `ToolSource` for the registry once
//! per turn. This is that source. It composes, per turn and per tenant:
//!
//! - every tool the connected MCP servers publish (`kura_mcp::tools_for_surface`,
//!   authorized under the `chat` surface's exposure rules when called);
//! - `memory.lookup`: fused recall over the tenant's Ready memory through the
//!   same `run_query` the `/v1/retrieval/queries` route uses, so the agent
//!   cannot recall anything the API would not return;
//! - one tool per Ready tool profile of the tenant (`web.search`,
//!   `image.generate`, `video.generate`), executed through `ToolRuntime`
//!   (quota reserve → egress → provider → commit/release). The `mcp_backed`
//!   family forwards to the user's own MCP server tool — the same `McpTool`
//!   adapter, so approval rules apply — with the quota reservation held
//!   around the call; `builtin_stub` answers without network.
//!
//! Every tool is wrapped so that a call runs the `chat/tool-call` hook point
//! first (a veto is reported to the model as a failed call, never silently
//! dropped), is recorded as a `chat.tool.called` event, and is counted.
//!
//! Dependencies are resolved lazily through the late `AppState` because the
//! chat plugin builds before tools/memory/mcp do.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kura_api::AppState;
use kura_chat::{ToolSource, ToolTurn};
use kura_core::{Tool, ToolError, ToolInvocation, ToolOutput, ToolRegistry};
use kura_llm::ToolSpec;
use serde_json::{Value, json};

pub const MEMORY_LOOKUP_TOOL: &str = "memory.lookup";
/// The MCP runtime surface the chat loop authorizes tools under.
pub const CHAT_RUNTIME_SURFACE: &str = "chat";
const MEMORY_LOOKUP_MAX_HITS: usize = 5;
/// Tool output is bounded before it goes back to the model.
pub const MAX_TOOL_RESULT_CHARS: usize = 16_000;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

pub struct AppTools {
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl AppTools {
    #[must_use]
    pub fn new(state: Arc<std::sync::OnceLock<AppState>>) -> Self {
        Self { state }
    }
}

impl ToolSource for AppTools {
    fn registry(&self, turn: &ToolTurn) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        let Some(state) = self.state.get() else {
            return Arc::new(registry);
        };
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        if let Some(mcp) = state.mcp.clone() {
            tools.extend(kura_mcp::tools_for_surface(&mcp, CHAT_RUNTIME_SURFACE));
        }
        if state.memory.is_some() {
            tools.push(Arc::new(MemoryLookupTool {
                state: state.clone(),
                tenant_id: turn.tenant_id.clone(),
            }));
        }
        tools.extend(profile_tools(state, &turn.tenant_id));
        for tool in tools {
            registry.register(Arc::new(Observed {
                inner: tool,
                state: state.clone(),
                turn: turn.clone(),
            }));
        }
        Arc::new(registry)
    }
}

/// One tool per Ready profile, named by capability. When two profiles serve
/// one capability the default wins, so the model sees one tool per
/// capability and the operator's default decides which vendor answers.
fn profile_tools(state: &AppState, tenant_id: &str) -> Vec<Arc<dyn Tool>> {
    let Some(tools) = state.tools.as_deref() else {
        return Vec::new();
    };
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    for capability in [
        kura_tools::Capability::WebSearch,
        kura_tools::Capability::ImageGenerate,
        kura_tools::Capability::VideoGenerate,
    ] {
        let Some(profile) = tools.resolve(tenant_id, capability, None) else {
            continue;
        };
        let backend = match profile.family {
            kura_tools::Family::McpBacked => match mcp_tool_for(state, &profile) {
                Some(tool) => Backend::Mcp(tool),
                // The server is not live or the tool is not discovered: do
                // not offer what cannot be called.
                None => continue,
            },
            kura_tools::Family::BuiltinStub => Backend::Stub,
        };
        out.push(Arc::new(ProfileTool {
            state: state.clone(),
            tenant_id: tenant_id.to_string(),
            capability,
            profile,
            backend,
        }));
    }
    out
}

/// The user's MCP tool behind an `mcp_backed` profile, if its server is
/// registered for the tenant and the tool is discovered and available.
fn mcp_tool_for(state: &AppState, profile: &kura_tools::ToolProfile) -> Option<kura_mcp::McpTool> {
    let mcp = state.mcp.clone()?;
    mcp.get_server_for_tenant(&profile.mcp_server_id, &profile.tenant_id)?;
    let published = mcp
        .list_tools_for_tenant(&profile.mcp_server_id, &profile.tenant_id)
        .ok()?;
    let tool = published
        .into_iter()
        .find(|t| t.tool.tool_name == profile.mcp_tool_name)?;
    if tool.effective_availability == "unavailable" {
        return None;
    }
    Some(kura_mcp::McpTool::new(
        mcp,
        profile.mcp_server_id.clone(),
        profile.mcp_tool_name.clone(),
        tool.tool.description.clone(),
        tool.tool.input_schema.clone(),
        CHAT_RUNTIME_SURFACE,
    ))
}

enum Backend {
    Stub,
    Mcp(kura_mcp::McpTool),
}

/// A capability served by a tool profile.
struct ProfileTool {
    state: AppState,
    tenant_id: String,
    capability: kura_tools::Capability,
    profile: kura_tools::ToolProfile,
    backend: Backend,
}

impl Tool for ProfileTool {
    fn spec(&self) -> ToolSpec {
        match &self.backend {
            Backend::Mcp(tool) => ToolSpec {
                name: self.capability.as_str().to_string(),
                ..tool.spec()
            },
            Backend::Stub => ToolSpec {
                name: self.capability.as_str().to_string(),
                description: format!(
                    "{} via the compiled-in stub (no network).",
                    self.capability.as_str()
                ),
                parameters: stub_parameters(self.capability),
            },
        }
    }

    fn call<'a>(&'a self, invocation: &'a ToolInvocation) -> ToolFuture<'a> {
        Box::pin(async move {
            let Some(runtime) = self.state.tool_runtime.as_deref() else {
                return Ok(ToolOutput::failed("tool plane is not configured"));
            };
            let args = match parse_arguments(&invocation.arguments) {
                Ok(args) => args,
                Err(reason) => return Ok(ToolOutput::failed(reason)),
            };
            let credential = resolve_credential(&self.state, &self.tenant_id, &self.profile).await;
            match &self.backend {
                Backend::Stub => {
                    let request: kura_tools::SearchRequest =
                        serde_json::from_value(args).unwrap_or_default();
                    match runtime.search(
                        &self.tenant_id,
                        &self.profile,
                        credential.as_deref(),
                        &request,
                        &kura_tools::BuiltinStubProvider,
                    ) {
                        Ok(results) => Ok(ToolOutput::ok(render_tool_value(
                            &serde_json::to_value(results).unwrap_or(Value::Null),
                        ))),
                        Err(err) => Ok(ToolOutput::failed(err.to_string())),
                    }
                }
                Backend::Mcp(tool) => {
                    // Reserve before the call, settle after it: the MCP call
                    // is async, so the runtime's reservation is held across it.
                    let reservation = match runtime.guard(&self.tenant_id, &self.profile) {
                        Ok(reservation) => reservation,
                        Err(err) => return Ok(ToolOutput::failed(err.to_string())),
                    };
                    let outcome = tool.call(invocation).await;
                    match &outcome {
                        Ok(output) if output.success => reservation.commit(),
                        _ => reservation.release("provider_failed"),
                    }
                    outcome
                }
            }
        })
    }
}

/// Recall over the tenant's Ready memory (9.0b).
struct MemoryLookupTool {
    state: AppState,
    tenant_id: String,
}

impl Tool for MemoryLookupTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: MEMORY_LOOKUP_TOOL.to_string(),
            description: "Recall the user's stored memory (facts, preferences, decisions, past \
                          scenarios). Use it when the answer depends on something the user told \
                          you before. Returns ranked hits with asset ids you can cite as \
                          Memory[<layer> <assetId>]."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to recall, in natural language." },
                    "assetId": { "type": "string", "description": "Optional: fetch one asset by id instead of searching." }
                },
                "required": []
            }),
        }
    }

    fn call<'a>(&'a self, invocation: &'a ToolInvocation) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match parse_arguments(&invocation.arguments) {
                Ok(args) => args,
                Err(reason) => return Ok(ToolOutput::failed(reason)),
            };
            let Some(memory) = self.state.memory.as_deref() else {
                return Ok(ToolOutput::failed("memory plane is not configured"));
            };
            if let Some(asset_id) = args
                .get("assetId")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                let Some(asset) = memory.get(asset_id) else {
                    return Ok(ToolOutput::failed("no such memory asset"));
                };
                // Same visibility and tenancy rules as injection: a lookup by
                // id must not reach past what retrieval would rank.
                let visible = asset.tenant_id == self.tenant_id.trim()
                    && asset.status == kura_memory::AssetStatus::Ready
                    && matches!(
                        asset.visibility,
                        kura_memory::Visibility::Private | kura_memory::Visibility::Team
                    );
                if !visible {
                    return Ok(ToolOutput::failed("no such memory asset"));
                }
                return Ok(ToolOutput::ok(format!(
                    "Memory[{} {}] {}\n{}",
                    asset.layer.as_str(),
                    asset.asset_id,
                    asset.title,
                    asset.content
                )));
            }
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return Ok(ToolOutput::failed("query or assetId is required"));
            }
            match kura_api::routes::retrieval::run_query(
                &self.state,
                &self.tenant_id,
                query,
                MEMORY_LOOKUP_MAX_HITS,
            ) {
                Ok(hits) if hits.is_empty() => Ok(ToolOutput::ok("no matching memory")),
                Ok(hits) => Ok(ToolOutput::ok(
                    hits.iter()
                        .map(|hit| {
                            format!(
                                "[{}] Memory[{} {}] {}\n{}",
                                hit.rank, hit.layer, hit.asset_id, hit.title, hit.content
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                )),
                Err(err) => Ok(ToolOutput::failed(err)),
            }
        })
    }
}

/// The observation wrapper: hook point, audit event, metrics, output bound.
struct Observed {
    inner: Arc<dyn Tool>,
    state: AppState,
    turn: ToolTurn,
}

impl Tool for Observed {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn call<'a>(&'a self, invocation: &'a ToolInvocation) -> ToolFuture<'a> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let mut effective = ToolInvocation {
                call_id: invocation.call_id.clone(),
                name: invocation.name.clone(),
                arguments: invocation.arguments.clone(),
            };
            let mut vetoed: Option<String> = None;
            if let Some(hooks) = &self.state.hooks {
                let mut payload = json!({
                    "tenantId": self.turn.tenant_id,
                    "threadId": self.turn.thread_id,
                    "agentProfileId": self.turn.agent_profile_id,
                    "callId": invocation.call_id,
                    "name": invocation.name,
                    "arguments": invocation.arguments,
                });
                let outcome = hooks.run(kura_plugin::points::CHAT_TOOL_CALL, &mut payload);
                if let Some((plugin_id, reason)) = outcome.halted {
                    vetoed = Some(format!("tool call vetoed by plugin {plugin_id}: {reason}"));
                } else if outcome.ran > 0 {
                    if let Some(rewritten) = payload.get("arguments").and_then(Value::as_str) {
                        effective.arguments = rewritten.to_string();
                    }
                }
            }
            let result = match vetoed {
                Some(reason) => Ok(ToolOutput::failed(reason)),
                None => self.inner.call(&effective).await,
            };
            let result = result.map(|output| ToolOutput {
                content: truncate_tool_output(&output.content),
                success: output.success,
            });
            let (output, is_error) = match &result {
                Ok(output) => (output.content.clone(), !output.success),
                Err(err) => (err.to_string(), true),
            };
            let metrics = kura_telemetry::metrics::registry();
            metrics.inc(
                kura_telemetry::metrics::CHAT_TOOL_CALLS_TOTAL,
                &[
                    ("name", invocation.name.as_str()),
                    ("outcome", if is_error { "error" } else { "ok" }),
                ],
            );
            metrics.observe(
                kura_telemetry::metrics::CHAT_TOOL_CALL_DURATION_SECONDS,
                &[("name", invocation.name.as_str())],
                started.elapsed().as_secs_f64(),
            );
            self.publish_called(
                &effective,
                &output,
                is_error,
                started.elapsed().as_millis() as i64,
            );
            result
        })
    }
}

impl Observed {
    /// `chat.tool.called`: the audit record of one tool call, with the
    /// (bounded) arguments and output an operator needs to reconstruct what
    /// the model did with a tool.
    fn publish_called(
        &self,
        call: &ToolInvocation,
        output: &str,
        is_error: bool,
        duration_ms: i64,
    ) {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "tenantId".into(),
            Value::String(self.turn.tenant_id.clone()),
        );
        payload.insert(
            "threadId".into(),
            Value::String(self.turn.thread_id.clone()),
        );
        payload.insert("callId".into(), Value::String(call.call_id.clone()));
        payload.insert("name".into(), Value::String(call.name.clone()));
        payload.insert(
            "arguments".into(),
            Value::String(truncate_tool_output(&call.arguments)),
        );
        payload.insert("output".into(), Value::String(output.to_string()));
        payload.insert("isError".into(), Value::Bool(is_error));
        payload.insert("durationMs".into(), Value::from(duration_ms));
        let event = kura_events::Event {
            event_id: kura_chat::new_event_id(),
            occurred_at: chrono::Utc::now(),
            category: "chat".to_string(),
            name: "chat.tool.called".to_string(),
            scope: self.turn.scope.clone(),
            resource: kura_events::Resource {
                kind: "tool_call".to_string(),
                id: call.call_id.clone(),
            },
            payload,
            ..kura_events::Event::default()
        };
        let event = self
            .state
            .store
            .lock()
            .append_event(&event)
            .unwrap_or(event);
        self.state.event_bus.publish(event);
    }
}

fn parse_arguments(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|err| format!("arguments are not valid JSON: {err}"))
}

fn stub_parameters(capability: kura_tools::Capability) -> Value {
    match capability {
        kura_tools::Capability::WebSearch => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "maxResults": { "type": "integer", "minimum": 1 }
            },
            "required": ["query"]
        }),
        _ => json!({
            "type": "object",
            "properties": { "prompt": { "type": "string" } },
            "required": ["prompt"]
        }),
    }
}

/// MCP `content` blocks become text; anything else is rendered as JSON.
fn render_tool_value(value: &Value) -> String {
    if let Some(blocks) = value.get("content").and_then(Value::as_array) {
        let texts: Vec<String> = blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
            .collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Truncates on a char boundary and marks the cut so the model knows the
/// output is partial.
#[must_use]
pub fn truncate_tool_output(text: &str) -> String {
    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    out.push_str("\n…[truncated]");
    out
}

/// Resolves the profile's `secretRef` through the tenant secret plane.
/// Failure is `None`, as in the tools route: the reason may echo the
/// reference.
async fn resolve_credential(
    state: &AppState,
    tenant_id: &str,
    profile: &kura_tools::ToolProfile,
) -> Option<String> {
    let secret_ref = profile.secret_ref.trim();
    if secret_ref.is_empty() {
        return None;
    }
    let secrets = state.secrets.clone()?;
    secrets
        .resolve(kura_secrets::ResolveInput {
            tenant_id: tenant_id.to_string(),
            secret_ref: secret_ref.to_string(),
        })
        .await
        .ok()
        .map(|resolved| resolved.value)
}
