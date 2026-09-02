//! The echo provider: a deterministic in-process provider registered by
//! default, used by tests and local smoke checks. Mirrors `EchoProvider` in
//! `daemon/internal/llm/dispatcher.go`.

use futures::future::BoxFuture;

use crate::provider::{Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter};
use crate::types::{Message, StreamChunk, Usage};

pub struct EchoProvider;

impl EchoProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EchoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move { Ok(echo_response(&request.messages)) })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let output = compose_echo_output(&request.messages);
            for (index, part) in output.split_whitespace().enumerate() {
                if request.cancel.is_cancelled() {
                    return Err(ProviderError::Cancelled);
                }
                let delta = if index > 0 { format!(" {part}") } else { part.to_string() };
                emit(StreamChunk { delta, ..StreamChunk::default() })?;
            }
            Ok(echo_response(&request.messages))
        })
    }
}

fn echo_response(messages: &[Message]) -> ProviderResponse {
    let output = compose_echo_output(messages);
    ProviderResponse {
        // Echo answers from its input; it calls nothing.
        tool_calls: Vec::new(),
        usage: Usage {
            input_tokens: approximate_tokens(&compose_echo_output(messages)),
            output_tokens: approximate_tokens(&output),
            total_tokens: 0,
        },
        finish_reason: "stop".into(),
        output,
    }
}

fn compose_echo_output(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|message| message.content.trim())
        .collect::<Vec<_>>()
        .join("\n")
}

fn approximate_tokens(text: &str) -> i64 {
    if text.trim().is_empty() {
        return 0;
    }
    text.split_whitespace().count() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CancelToken;

    fn user_message(content: &str) -> Message {
        Message { role: crate::types::MessageRole::User, content: content.into() }
    }

    fn request(messages: Vec<Message>) -> ProviderRequest {
        ProviderRequest { messages, ..ProviderRequest::default() }
    }

    #[tokio::test]
    async fn complete_echoes_trimmed_joined_content_with_word_usage() {
        let provider = EchoProvider::new();
        let response = provider
            .complete(request(vec![user_message("  hello world  "), user_message("again")]))
            .await
            .unwrap();
        assert_eq!(response.output, "hello world\nagain");
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.input_tokens, 3);
        assert_eq!(response.usage.output_tokens, 3);
        assert_eq!(response.usage.total_tokens, 0);
    }

    #[tokio::test]
    async fn stream_emits_word_deltas_with_rejoined_spaces() {
        let provider = EchoProvider::new();
        let mut deltas = Vec::new();
        let response = provider
            .stream(request(vec![user_message("one two three")]), &mut |chunk| {
                deltas.push(chunk.delta);
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(deltas, vec!["one", " two", " three"]);
        assert_eq!(response.output, "one two three");
    }

    #[tokio::test]
    async fn stream_aborts_when_cancelled() {
        let provider = EchoProvider::new();
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut req = request(vec![user_message("one two")]);
        req.cancel = cancel;
        let result = provider.stream(req, &mut |_| Ok(())).await;
        assert_eq!(result.unwrap_err(), ProviderError::Cancelled);
    }

    #[test]
    fn approximate_tokens_counts_words() {
        assert_eq!(approximate_tokens(""), 0);
        assert_eq!(approximate_tokens("   "), 0);
        assert_eq!(approximate_tokens("a b  c"), 3);
    }
}
