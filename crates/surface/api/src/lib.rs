//! kura-api — the HTTP API surface of the daemon (port of Go
//! daemon/internal/api). Wave 8 foundation: application state, error mapping,
//! response helpers, auth/tenant middleware, the introspection routes, and the
//! shared DTO vocabulary (port of api/types.go). Route families land in later
//! waves as modules under src/.

pub mod error;
pub mod hosting;
pub mod middleware;
pub mod response;
pub mod routes;
pub mod state;
pub mod types;

pub use error::ApiError;
pub use routes::router;
pub use state::AppState;
pub use types::*;
