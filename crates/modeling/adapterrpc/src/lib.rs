//! Port of `daemon/internal/integrations/adapterrpc`. See rs/MIGRATION.md for conventions.
//!
//! The daemon side of the external integration adapter plane (Roadmap 59): schema-aligned
//! RPC envelopes, a newline-delimited JSON codec, a stdio transport, and a [`Client`] that
//! domain Backend shims (calendar, mail) use to dispatch operations to an out-of-process
//! adapter.
//!
//! The adapter performs provider request/response mapping only. The daemon retains the
//! operation ledger, idempotency, side-effect evidence, artifacts, and persistence; nothing
//! in this crate records that state.

mod client;
mod codec;
mod conformance;
mod credentials;
mod error;
mod transport;
mod types;

pub use client::{Client, CredentialResolver, DEFAULT_DEADLINE, ResolverError};
pub use codec::{read_request, read_response, write_message};
pub use conformance::{CONFORMANCE_OPS, ConformanceReport, ConformanceResult, run_conformance};
pub use credentials::{IntegrationCredentialFetcher, scoped_resolver};
pub use error::{AdapterError, CodecError, Error, FailureKind, is_ambiguous};
pub use types::{CONTRACT_VERSION, Request, Response, Status};
