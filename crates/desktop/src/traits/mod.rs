//! Capability traits for desktop native integration.
//!
//! Each trait defines a contract for a specific domain of desktop capabilities.
//! Platform implementations (macOS, Windows, Linux) implement these traits
//! behind the [`crate::DesktopPlatform`] aggregator.

pub mod automation;
pub mod pim;
pub mod screen;
pub mod system;

pub use automation::AutomationCapability;
pub use pim::PimCapability;
pub use screen::ScreenCapability;
pub use system::SystemCapability;
