// Individual browser tools — focused, single-responsibility browser actions.
//
// Each tool wraps a ProfileManager and implements AlephTool for one operation.

pub mod click;
pub mod evaluate;
pub mod fill_form;
pub mod navigate;
pub mod open;
pub mod profile_tool;
pub mod screenshot;
pub mod select;
pub mod snapshot;
pub mod tabs;
pub mod type_text;

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use evaluate::{BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool};
pub use fill_form::{BrowserFillFormArgs, BrowserFillFormOutput, BrowserFillFormTool};
pub use navigate::{BrowserNavigateArgs, BrowserNavigateOutput, BrowserNavigateTool};
pub use open::{BrowserOpenArgs, BrowserOpenOutput, BrowserOpenTool};
pub use profile_tool::{BrowserProfileArgs, BrowserProfileOutput, BrowserProfileTool};
pub use screenshot::{BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool};
pub use select::{BrowserSelectArgs, BrowserSelectOutput, BrowserSelectTool};
pub use snapshot::{BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool};
pub use tabs::{BrowserTabsArgs, BrowserTabsOutput, BrowserTabsTool};
pub use type_text::{BrowserTypeArgs, BrowserTypeOutput, BrowserTypeTool};

/// Default browser profile name, used by serde `default` attributes across all browser tools.
pub(crate) fn default_profile() -> String {
    "default".into()
}
