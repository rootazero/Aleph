pub mod connector;
pub mod reconnect;

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "native")]
pub mod native;

pub use connector::{AlephConnector, ConnectionError};
pub use reconnect::ReconnectStrategy;

#[cfg(feature = "wasm")]
pub use wasm::WasmConnector as DefaultConnector;

#[cfg(all(feature = "native", not(feature = "wasm")))]
pub use native::NativeConnector as DefaultConnector;
