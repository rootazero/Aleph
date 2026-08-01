pub mod connector;
pub mod failure;
pub mod reconnect;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use connector::{AlephConnector, ConnectionError};
pub use failure::{
    classify, stage_for_connect_error, ConnectionFailure, FailureStage, OriginLiveness,
};
pub use reconnect::ReconnectStrategy;

#[cfg(feature = "wasm")]
pub use wasm::WasmConnector as DefaultConnector;
