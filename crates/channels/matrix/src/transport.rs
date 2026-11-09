//! Port of daemon/internal/connectors/matrix/transport.go: the connector
//! Transport boundary and the in-memory FakeTransport used by tests and as the
//! default no-op transport of the runtime.

use std::sync::Arc;

use kura_im::{ReplyProgressor, ReplySender};
use kura_imtypes::{OutboundReply, ReplyCapabilities, SentReply};
use parking_lot::Mutex;

use crate::types::InboundEvent;

/// Go `Transport` interface. `start` drives the inbound event loop
/// synchronously, invoking `handle` for every inbound event for the duration
/// of the call (mirroring the Go transport's blocking sync loop).
pub trait Transport: Send + Sync {
    /// Go `Start(ctx, handle)`: begins consuming inbound events. The handle
    /// is only used for the duration of the call.
    fn start(&self, handle: &dyn Fn(InboundEvent)) -> Result<(), String>;
    /// Go `SendReply`: delivers a final outbound reply and returns the
    /// external message id assigned by the homeserver.
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String>;
    /// Go `ReplyCapabilities`.
    fn reply_capabilities(&self) -> ReplyCapabilities;
    /// Go `Close`.
    fn close(&self) -> Result<(), String>;
}

/// A Transport is a kura-im ReplySender: `reply_progressor` stays `None`
/// because Matrix only supports final-only foreground replies (Go's
/// replies.(ReplyProgressor) type assertion fails for the Matrix transport).
impl ReplySender for dyn Transport {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        Transport::send_reply(self, reply)
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        None
    }
}

/// Adapts a [Transport] into a kura-im [ReplySender] value (trait-object to
/// trait-object coercion is not automatic for unrelated traits).
pub(crate) struct TransportReplySender<'a>(pub &'a dyn Transport);

impl ReplySender for TransportReplySender<'_> {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.0.send_reply(reply)
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        None
    }
}

/// The owned form of [TransportReplySender], for `Runtime::reply_sender`.
pub(crate) struct OwnedReplySender(pub std::sync::Arc<dyn Transport>);

impl ReplySender for OwnedReplySender {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.0.send_reply(reply)
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        None
    }
}

/// Go `FakeTransport`: replays a fixed set of inbound events on start and
/// records every sent reply. Cloneable so tests can retain a handle to inspect
/// sent replies after the transport is moved into a runtime.
#[derive(Clone)]
pub struct FakeTransport {
    inbound: Arc<Vec<InboundEvent>>,
    sent: Arc<Mutex<Vec<OutboundReply>>>,
}

impl FakeTransport {
    /// Go `NewFakeTransport(messages ...InboundEvent)`.
    #[must_use]
    pub fn new(messages: Vec<InboundEvent>) -> Self {
        FakeTransport {
            inbound: Arc::new(messages),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Go `SentReplies`: a copy of every reply sent through this transport.
    #[must_use]
    pub fn sent_replies(&self) -> Vec<OutboundReply> {
        self.sent.lock().clone()
    }
}

impl Transport for FakeTransport {
    fn start(&self, handle: &dyn Fn(InboundEvent)) -> Result<(), String> {
        for event in self.inbound.iter().cloned() {
            handle(event);
        }
        Ok(())
    }

    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.sent.lock().push(reply.clone());
        Ok(SentReply {
            external_message_id: format!("matrix_reply_{}", reply.channel_id.trim()),
        })
    }

    fn reply_capabilities(&self) -> ReplyCapabilities {
        ReplyCapabilities {
            max_message_length: 40000,
            ..ReplyCapabilities::default()
        }
    }

    fn close(&self) -> Result<(), String> {
        Ok(())
    }
}
