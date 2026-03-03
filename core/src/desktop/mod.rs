pub mod client;
pub mod error;
pub mod types;

pub use client::DesktopBridgeClient;
pub use error::DesktopError;
pub use types::{
    CanvasPosition, DesktopRequest, DesktopResponse, DesktopRpcError, MouseButton, RefId,
    ResolvedElement, ScreenRegion, SnapshotStats,
};

// Re-export desktop capability types
pub use aleph_desktop::{
    Capability, DesktopCapability, NativeDesktop, OcrResult, Screenshot, WindowInfo,
};
