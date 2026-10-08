//! Browser live-view workspace body.
//!
//! This slice is the **render shell and the pure state model** only:
//!
//! * [`model`] — what the Panel may show, built solely from resources and
//!   snapshots the gateway already authorized, and which rejects any update
//!   that does not match the bound resource/binding exactly.
//! * [`viewer`] — an accessible status view over that model.
//!
//! Not here yet (owned by the producer/issuer wiring): consuming a fragment
//! nonce, exchanging a ticket, opening the live socket, feeding
//! [`BrowserWorkspace`] from bus events. Until then nothing in this module can
//! name a browser target by itself.

pub mod model;
pub mod viewer;

pub use model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, ClosedResource, ModelError, UnboundReason,
    ViewerPhase,
};
pub use viewer::{BrowserViewer, BrowserViewerPage};
