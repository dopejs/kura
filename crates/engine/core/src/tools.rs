use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kura_model_provider::ToolSpec;
use kura_protocol::ImagePart;

#[derive(Debug)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug)]
pub struct ToolOutput {
    pub content: String,
    pub success: bool,
    /// Images beside the text, such as a screenshot. Most tools return none.
    pub images: Vec<ImagePart>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            images: Vec::new(),
        }
    }

    pub fn failed(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: false,
            images: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_images(mut self, images: Vec<ImagePart>) -> Self {
        self.images = images;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("tool execution failed: {0}")]
    Failed(String),
}

/// Object-safe tool interface. Native `async fn` is not object-safe, so
/// implementations return a boxed future instead.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn call<'a>(
        &'a self,
        invocation: &'a ToolInvocation,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

    /// Whether this call may run concurrently with other such calls of the
    /// same round. Only a call that changes nothing *and* will not stop to
    /// ask a person qualifies: a batch must never start beside a call that
    /// is waiting for approval. Asked before the batch starts, so it must
    /// not have side effects. The default is serial, which is always safe.
    fn parallel_safe(&self, _invocation: &ToolInvocation) -> bool {
        false
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.spec().name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.tools.values().map(|tool| tool.spec()).collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// [`Tool::parallel_safe`] by name; an unknown tool is serial.
    pub fn parallel_safe(&self, invocation: &ToolInvocation) -> bool {
        self.get(&invocation.name)
            .is_some_and(|tool| tool.parallel_safe(invocation))
    }

    pub async fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolOutput, ToolError> {
        match self.get(&invocation.name) {
            Some(tool) => tool.call(invocation).await,
            None => Err(ToolError::NotFound(invocation.name.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoTool;

    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "echo back the arguments".into(),
                parameters: json!({"type": "object"}),
            }
        }

        fn call<'a>(
            &'a self,
            invocation: &'a ToolInvocation,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
            Box::pin(async move { Ok(ToolOutput::ok(invocation.arguments.clone())) })
        }
    }

    #[tokio::test]
    async fn registry_dispatches_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let invocation = ToolInvocation {
            call_id: "call_1".into(),
            name: "echo".into(),
            arguments: "{\"text\":\"hi\"}".into(),
        };
        let output = registry.invoke(&invocation).await.unwrap();
        assert!(output.success);
        assert_eq!(output.content, "{\"text\":\"hi\"}");
    }

    #[tokio::test]
    async fn unknown_tool_is_not_found() {
        let registry = ToolRegistry::new();
        let invocation = ToolInvocation {
            call_id: "call_1".into(),
            name: "missing".into(),
            arguments: "{}".into(),
        };
        let err = registry.invoke(&invocation).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound(name) if name == "missing"));
    }
}
