//! Browser live-view workspace body.
//!
//! This slice is the **render shell, the pure state model and the bus bridge**:
//!
//! * [`model`] — what the Panel may show, built solely from resources and
//!   snapshots the gateway already authorized, and which rejects any update
//!   that does not match the bound resource/binding exactly.
//! * [`launch`] — Safe-resource eligibility, issuer URL validation and one-shot
//!   Frames grant lifecycle for the Panel's explicit open action.
//! * [`viewer`] — an accessible status view over that model.
//! * [`bridge`] — decodes the exact `browser.live.resource` /
//!   `browser.live.closed` bus topics (nested `resource` / `closed` envelope
//!   only) and applies them to the root [`BrowserWorkspace`].
//!
//! The event bridge is wired (root provider + persistent topic subscriptions
//! in `app.rs`), and the standalone `/viewer/browser` route now has a
//! read-only WASM transport for consume/exchange/AuthHello/WebSocket frames.
//! The in-workspace shell now requests a Frames-only grant only after an
//! exact resource is Safe, validates the Gateway-returned standalone URL and
//! opens it unchanged in a new tab. The shell still does not control the
//! browser or derive a target from a URL, profile or current tab.

pub mod bridge;
pub mod launch;
pub mod model;
pub mod session;
pub mod standalone;
pub mod transport;
pub mod viewer;

pub use bridge::{
    apply_browser_event, decode_browser_event, seed_from_gateway, BrowserBusEvent,
    BrowserEventOutcome, SeedOutcome,
};
pub use launch::{
    launch_target, model_launch_target, validate_launch_url, LaunchBlock, LaunchPhase, LaunchTarget,
};
pub use model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, ClosedResource, ModelError, UnboundReason,
    ViewerPhase,
};
pub use session::{
    is_standalone_viewer_path, AcceptedFrame, AuthHelloText, CloseReason, FragmentError,
    FrameReject, OriginError, SessionError, SessionPhase, ViewerEndpoints, ViewerFragment,
    ViewerSession,
};
pub use standalone::{
    is_standalone_viewer_route, StandaloneBrowserViewer, StandaloneBrowserViewerRoute,
};
pub use transport::TransportUi;
pub use viewer::{BrowserViewer, BrowserViewerPage};
