pub mod client;
pub mod error;
pub mod types;

pub use client::DesktopBridgeClient;
pub use error::DesktopError;
pub use types::{
    CanvasPosition, DesktopRequest, DesktopResponse, DesktopRpcError, MouseButton, ScreenRegion,
};
