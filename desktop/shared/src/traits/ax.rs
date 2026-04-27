//! Accessibility tree capability trait.
//!
//! Platform implementations that support the macOS Accessibility API
//! implement this trait and return `Some(&self.ax)` from
//! [`crate::DesktopPlatform::ax`].

use async_trait::async_trait;

use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryByRoleParams, QueryTreeParams};

use crate::error::Result;

/// Query the OS accessibility (AX) element tree.
///
/// All methods are async because the underlying RPC call to the Swift
/// helper is I/O-bound.  On non-macOS platforms the `DesktopPlatform`
/// default returns `None` from `ax()`, so these methods are never called.
#[async_trait]
pub trait AccessibilityCapability: Send + Sync {
    /// Return the UI element that currently holds keyboard focus, or `None`
    /// if no element is focused (or the system-wide focused element is
    /// inaccessible).
    async fn query_focused(&self) -> Result<Option<AxElement>>;

    /// Return the full AX subtree rooted at the target process.
    ///
    /// `params.pid` selects the process; `None` means "frontmost app".
    /// `params.max_depth` bounds the returned tree (default 6).
    async fn query_tree(&self, params: QueryTreeParams) -> Result<Option<AxElement>>;

    /// Collect all elements whose AX role matches `params.role`.
    ///
    /// `params.pid` selects the process; `None` means "frontmost app".
    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<Vec<AxElement>>;
}
