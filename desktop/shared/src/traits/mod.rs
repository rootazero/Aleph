//! Capability traits for desktop native integration.
//!
//! Each trait defines a contract for a specific domain of desktop capabilities.
//! Platform implementations (macOS, Windows, Linux) implement these traits
//! behind the [`crate::DesktopPlatform`] aggregator.

pub mod automation;
pub mod ax;
pub mod media;
pub mod permission;
pub mod pim;
pub mod power;
pub mod screen;
pub mod system;

pub use automation::AutomationCapability;
pub use ax::AccessibilityCapability;
pub use media::MediaCapability;
pub use permission::PermissionCapability;
pub use pim::PimCapability;
pub use power::{InhibitorGuard, PowerCapability};
pub use screen::ScreenCapability;
pub use system::SystemCapability;
