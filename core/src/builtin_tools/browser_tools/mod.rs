// Individual browser tools — focused, single-responsibility browser actions.
//
// Each tool wraps a ProfileManager and implements AlephTool for one operation.

pub mod click;
pub mod open;
pub mod screenshot;
pub mod snapshot;
pub mod type_text;

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use open::{BrowserOpenArgs, BrowserOpenOutput, BrowserOpenTool};
pub use screenshot::{BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool};
pub use snapshot::{BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool};
pub use type_text::{BrowserTypeArgs, BrowserTypeOutput, BrowserTypeTool};
