pub mod connection;
pub mod protocol;
pub mod state;
pub mod api;
pub mod observability;

pub use connection::connector::{AlephConnector, ConnectionError};
pub use protocol::rpc::RpcClient;

/// Re-export commonly used types
pub mod prelude {
    pub use crate::connection::connector::{AlephConnector, ConnectionError};
    pub use crate::protocol::rpc::RpcClient;
    #[cfg(feature = "leptos")]
    pub use leptos::prelude::*;
}
