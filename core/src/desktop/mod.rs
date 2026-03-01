pub mod client;
pub mod error;
pub mod types;

pub use client::DesktopBridgeClient;
pub use error::DesktopError;
pub use types::{
    CanvasPosition, DesktopRequest, DesktopResponse, DesktopRpcError, MouseButton, RefId,
    ResolvedElement, ScreenRegion, SnapshotStats,
};

// Re-export NativeDesktop when desktop-native feature is enabled
#[cfg(feature = "desktop-native")]
pub use aleph_desktop::{DesktopCapability, NativeDesktop};
