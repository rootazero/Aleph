pub mod http_client;
pub mod pool;
mod protocol;
pub mod sse_stream;

pub use crate::a2a::port::{ResolvedEndpoint, RpcProtocol};
pub(crate) use http_client::DelegationResponse;
pub use http_client::{A2AClient, DeliveryStatus};
pub use pool::A2AClientPool;
pub use sse_stream::{fold_stream, parse_sse_response, FoldedOutcome};
