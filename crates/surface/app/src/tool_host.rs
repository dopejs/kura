//! Stage 9.0/9.0b/9.2/9.3/9.5: the assembly's [`kura_chat::ToolHost`].
//!
//! What the model can call is decided here, from three sources, and nothing
//! reaches the model that is not also gated at call time:
//!
//! - `memory.lookup` (9.0b): fused recall over the tenant's Ready memory,
//!   through the same `run_query` the `/v1/retrieval/queries` route uses, so
//!   the agent cannot recall anything the API would not return. Offered
//!   whenever the memory plane is built.
//! - one tool per Ready tool profile of the acting tenant (`web.search`,
//!   `image.generate`, `video.generate`), executed through `ToolRuntime`
//!   (quota reserve → egress → provider → commit/release). The
//!   `mcp_backed` family forwards to the user's own MCP server tool under the
//!   MCP plane's exposure/approval rules, and offers the model the server's
//!   real `inputSchema`; `builtin_stub` answers without network.
//!
//! Dependencies are resolved lazily through the late `AppState` because the
//! chat plugin builds before tools/memory/mcp do.

use std::sync::Arc;

use kura_api::AppState;
use kura_chat::{ToolContext, ToolHost, ToolOutcome};
use kura_llm::{ToolCall, ToolSpec};
use serde_json::{Value, json};

pub const MEMORY_LOOKUP_TOOL: &str = "memory.lookup";
/// The MCP runtime surface the chat loop authorizes tools under. A server's
/// exposure rules for this surface decide allow / approval / block.
pub const CHAT_RUNTIME_SURFACE: &str = "chat";
const MEMORY_LOOKUP_MAX_HITS: usize = 5;

pub struct AppToolHost {
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl AppToolHost {
    #[must_use]
    pub fn new(state: Arc<std::sync::OnceLock<AppState>>) -> Self {
        Self { state }
    }

    fn memory_lookup_spec() -> ToolSpec {
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

    /// One spec per Ready profile, named by capability. When two profiles
    /// serve one capability the default wins, so the model sees one tool per
    /// capability and the operator's default decides which vendor answers.
    fn profile_specs(state: &AppState, tenant_id: &str) -> Vec<ToolSpec> {
        let Some(tools) = state.tools.as_deref() else {
            return Vec::new();
        };
        let mut specs = Vec::new();
        for capability in [
            kura_tools::Capability::WebSearch,
            kura_tools::Capability::ImageGenerate,
            kura_tools::Capability::VideoGenerate,
        ] {
            let Some(profile) = tools.resolve(tenant_id, capability, None) else {
                continue;
            };
            let (description, parameters) = match profile.family {
                kura_tools::Family::McpBacked => match mcp_tool_shape(state, &profile) {
                    Some(shape) => shape,
                    // The server is not healthy or the tool is not discovered:
                    // do not offer what cannot be called.
                    None => continue,
                },
                kura_tools::Family::BuiltinStub => (
                    format!(
                        "{} via the compiled-in stub (no network).",
                        capability.as_str()
                    ),
                    stub_parameters(capability),
                ),
            };
            specs.push(ToolSpec {
                name: capability.as_str().to_string(),
                description,
                parameters,
            });
        }
        specs
    }

    fn call_memory_lookup(state: &AppState, ctx: &ToolContext, args: &Value) -> ToolOutcome {
        let Some(memory) = state.memory.as_deref() else {
            return ToolOutcome::error("memory plane is not configured");
        };
        if let Some(asset_id) = args
            .get("assetId")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            let Some(asset) = memory.get(asset_id) else {
                return ToolOutcome::error("no such memory asset");
            };
            // Same visibility and tenancy rules as injection: a lookup by id
            // must not reach past what retrieval would rank.
            let visible = asset.tenant_id == ctx.tenant_id.trim()
                && asset.status == kura_memory::AssetStatus::Ready
                && matches!(
                    asset.visibility,
                    kura_memory::Visibility::Private | kura_memory::Visibility::Team
                );
            if !visible {
                return ToolOutcome::error("no such memory asset");
            }
            return ToolOutcome::ok(format!(
                "Memory[{} {}] {}\n{}",
                asset.layer.as_str(),
                asset.asset_id,
                asset.title,
                asset.content
            ));
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return ToolOutcome::error("query or assetId is required");
        }
        match kura_api::routes::retrieval::run_query(
            state,
            &ctx.tenant_id,
            query,
            MEMORY_LOOKUP_MAX_HITS,
        ) {
            Ok(hits) if hits.is_empty() => ToolOutcome::ok("no matching memory"),
            Ok(hits) => ToolOutcome::ok(
                hits.iter()
                    .map(|hit| {
                        format!(
                            "[{}] Memory[{} {}] {}\n{}",
                            hit.rank, hit.layer, hit.asset_id, hit.title, hit.content
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
            Err(err) => ToolOutcome::error(err),
        }
    }

    fn call_capability(
        state: &AppState,
        ctx: &ToolContext,
        capability: kura_tools::Capability,
        args: Value,
    ) -> ToolOutcome {
        let (Some(tools), Some(runtime)) = (state.tools.as_deref(), state.tool_runtime.as_deref())
        else {
            return ToolOutcome::error("tool plane is not configured");
        };
        let Some(profile) = tools.resolve(&ctx.tenant_id, capability, None) else {
            return ToolOutcome::error(format!(
                "no usable {} provider is configured",
                capability.as_str()
            ));
        };
        let credential = resolve_credential(state, &ctx.tenant_id, &profile);
        let result = match profile.family {
            kura_tools::Family::BuiltinStub => {
                let request: kura_tools::SearchRequest =
                    serde_json::from_value(args).unwrap_or_default();
                runtime
                    .search(
                        &ctx.tenant_id,
                        &profile,
                        credential.as_deref(),
                        &request,
                        &kura_tools::BuiltinStubProvider,
                    )
                    .map(|results| serde_json::to_value(results).unwrap_or(Value::Null))
            }
            kura_tools::Family::McpBacked => {
                let provider = McpBackedProvider {
                    state,
                    tenant_id: ctx.tenant_id.clone(),
                };
                runtime.invoke(
                    &ctx.tenant_id,
                    &profile,
                    credential.as_deref(),
                    args,
                    &provider,
                )
            }
        };
        match result {
            Ok(value) => ToolOutcome::ok(render_tool_value(&value)),
            Err(err) => ToolOutcome::error(err.to_string()),
        }
    }
}

impl ToolHost for AppToolHost {
    fn available_tools(&self, ctx: &ToolContext) -> Vec<ToolSpec> {
        let Some(state) = self.state.get() else {
            return Vec::new();
        };
        let mut specs = Vec::new();
        if state.memory.is_some() {
            specs.push(Self::memory_lookup_spec());
        }
        specs.extend(Self::profile_specs(state, &ctx.tenant_id));
        specs
    }

    fn call(&self, ctx: &ToolContext, call: &ToolCall) -> ToolOutcome {
        let Some(state) = self.state.get() else {
            return ToolOutcome::error("assembly is not ready");
        };
        let args: Value = if call.arguments.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(err) => {
                    return ToolOutcome::error(format!("arguments are not valid JSON: {err}"));
                }
            }
        };
        if call.name == MEMORY_LOOKUP_TOOL {
            return Self::call_memory_lookup(state, ctx, &args);
        }
        match kura_tools::Capability::parse(&call.name) {
            Some(capability) => Self::call_capability(state, ctx, capability, args),
            None => ToolOutcome::error(format!("unknown tool: {}", call.name)),
        }
    }
}

/// The MCP-backed family: authorizes the call under the chat surface's
/// exposure rules, then calls the user's server. An approval-required rule
/// is reported to the model as an error naming the approval, so the operator
/// can grant it and the model can retry, rather than being silently blocked.
struct McpBackedProvider<'a> {
    state: &'a AppState,
    tenant_id: String,
}

impl kura_tools::CapabilityProvider for McpBackedProvider<'_> {
    fn family(&self) -> kura_tools::Family {
        kura_tools::Family::McpBacked
    }

    fn invoke(
        &self,
        profile: &kura_tools::ToolProfile,
        _credential: Option<&str>,
        arguments: Value,
    ) -> Result<Value, String> {
        let Some(mcp) = self.state.mcp.as_deref() else {
            return Err("mcp plane is not configured".to_string());
        };
        // Tenancy: the profile's server must belong to the acting tenant.
        if mcp
            .get_server_for_tenant(&profile.mcp_server_id, &self.tenant_id)
            .is_none()
        {
            return Err("mcp server not found".to_string());
        }
        let authorization = mcp
            .authorize_tool(
                &profile.mcp_server_id,
                &profile.mcp_tool_name,
                &kura_mcp::AuthorizeToolInput {
                    runtime_surface: CHAT_RUNTIME_SURFACE.to_string(),
                    requested_by: "chat".to_string(),
                    ..kura_mcp::AuthorizeToolInput::default()
                },
            )
            .map_err(|err| err.to_string())?;
        if authorization.status != kura_mcp::ToolAuthorizationStatus::Allowed {
            let approval = authorization
                .approval
                .as_ref()
                .map(|a| format!(" (approval {})", a.approval_id))
                .unwrap_or_default();
            return Err(format!(
                "tool use is {}{}: {}",
                authorization.status.as_str(),
                approval,
                if authorization.message.is_empty() {
                    "not allowed on the chat surface"
                } else {
                    &authorization.message
                }
            ));
        }
        let result = mcp
            .call_tool(
                &profile.mcp_server_id,
                &profile.mcp_tool_name,
                arguments,
                &authorization,
            )
            .map_err(|err| err.to_string())?;
        if !result.error.is_empty() {
            return Err(format!("{}: {}", result.failure_class, result.error));
        }
        Ok(result.output.unwrap_or(Value::Null))
    }
}

/// Description and parameters for an MCP-backed profile, from the server's
/// discovered tool. `None` when the tool is not discovered or not available
/// on the chat surface.
fn mcp_tool_shape(state: &AppState, profile: &kura_tools::ToolProfile) -> Option<(String, Value)> {
    let mcp = state.mcp.as_deref()?;
    let tools = mcp
        .list_tools_for_tenant(&profile.mcp_server_id, &profile.tenant_id)
        .ok()?;
    let tool = tools
        .into_iter()
        .find(|t| t.tool.tool_name == profile.mcp_tool_name)?;
    if tool.effective_availability == "unavailable" {
        return None;
    }
    let description = if tool.tool.description.is_empty() {
        format!(
            "{} via MCP server {}",
            profile.capability.as_str(),
            profile.mcp_server_id
        )
    } else {
        tool.tool.description.clone()
    };
    let parameters = tool
        .tool
        .input_schema
        .clone()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    Some((description, parameters))
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

/// Resolves the profile's `secretRef` through the tenant secret plane on a
/// throwaway runtime (the tool host is called from the chat service's
/// blocking thread, never from an async worker). Failure is `None`, as in
/// the tools route: the reason may echo the reference.
fn resolve_credential(
    state: &AppState,
    tenant_id: &str,
    profile: &kura_tools::ToolProfile,
) -> Option<String> {
    let secret_ref = profile.secret_ref.trim();
    if secret_ref.is_empty() {
        return None;
    }
    let secrets = state.secrets.clone()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime
        .block_on(secrets.resolve(kura_secrets::ResolveInput {
            tenant_id: tenant_id.to_string(),
            secret_ref: secret_ref.to_string(),
        }))
        .ok()
        .map(|resolved| resolved.value)
}
