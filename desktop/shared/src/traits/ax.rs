//! Accessibility tree capability trait.
//!
//! Platform implementations that expose an accessibility tree (macOS via the
//! Accessibility API, Windows via UI Automation) implement this trait and
//! return `Some(&self.ax)` from [`crate::DesktopPlatform::ax`].

use async_trait::async_trait;

use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, PerformActionParams, QueryByRoleParams, QueryFocusedParams,
    QueryListResult, QueryResult, QueryTreeParams, SetValueParams,
};

use crate::error::Result;

/// Query the OS accessibility (AX) element tree.
///
/// All methods are async because the backing implementation is I/O-bound
/// (macOS marshals over the Swift-helper RPC; Windows runs UI Automation COM
/// on a blocking thread). Platforms without an accessibility tree (currently
/// Linux) return `None` from `ax()`, so these methods are never called there;
/// `set_value` / `perform_action` also keep a `NotImplemented` default so a
/// platform can offer read-only AX without a write path.
#[async_trait]
pub trait AccessibilityCapability: Send + Sync {
    /// Return the UI element that currently holds keyboard focus, or `None` if
    /// nothing is focused (or the focused element is inaccessible).
    ///
    /// # The `pid` contract
    ///
    /// `params.pid == None` asks about the **system**: whichever app is
    /// frontmost owns the answer.
    ///
    /// `params.pid == Some(p)` asks about **that process**, and an implementation
    /// must return either an element belonging to `p` or `None`. Never another
    /// app's element — the caller is about to decide whether it is safe to type,
    /// and the whole reason it named a process is that the process it is typing
    /// into is not necessarily the one in front of the user.
    ///
    /// A platform whose accessibility layer only answers system-wide satisfies
    /// this by *filtering* its answer, which is strictly the same information the
    /// caller would otherwise have had to reconstruct. A platform that can ask an
    /// application directly (macOS) genuinely answers the question, and is the
    /// reason the parameter exists at all.
    async fn query_focused(&self, params: QueryFocusedParams) -> Result<Option<AxElement>>;

    /// Return the AX subtree rooted at the target process.
    ///
    /// `params.pid` selects the process; `None` means "frontmost app".
    /// `params.max_depth` and `params.max_nodes` bound the walk.
    ///
    /// Returning [`QueryResult`] rather than a bare element is deliberate: the
    /// walk is budgeted, so "this is all of it" and "this is as much as I was
    /// allowed to read" are different answers and the caller has to be able to
    /// tell them apart. Implementations must set
    /// [`QueryResult::truncated`] whenever they stopped early.
    async fn query_tree(&self, params: QueryTreeParams) -> Result<QueryResult>;

    /// Collect all elements whose AX role matches `params.role`.
    ///
    /// `params.pid` selects the process; `None` means "frontmost app". The
    /// budget bounds the *search*, so a truncated result means there may be
    /// further matches beyond the ones returned.
    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<QueryListResult>;

    /// Write `params.value` into the located element's `AXValue` attribute
    /// and read it back for verification. Platforms without a semantic
    /// accessibility write path inherit this `NotImplemented` default.
    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        let _ = params;
        Err(crate::DesktopError::NotImplemented("ax.set_value".into()))
    }

    /// Perform a native AX action (e.g. `AXPress`) on the located element.
    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        let _ = params;
        Err(crate::DesktopError::NotImplemented(
            "ax.perform_action".into(),
        ))
    }
}
