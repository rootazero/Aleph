//! Right-pane slot contracts. The app-shell host is introduced separately.
#![deny(unreachable_pub)]

mod registry;
mod slot;
mod slots;

pub use registry::{RegistryError, RightPaneRegistry};
pub use slot::{RightPaneSlot, SlotCtx, SlotRenderCtx};

use std::sync::Arc;

/// Build the stage-one catalog once, then share it without mutable access.
///
/// The app-shell host owns this construction when it is wired in Task 2.
#[expect(
    clippy::arc_with_non_send_sync,
    reason = "The required Arc catalog is shared only within the Leptos UI thread."
)]
pub fn stage_one_registry() -> Arc<RightPaneRegistry> {
    let mut registry = RightPaneRegistry::new();
    slots::register_stage_one_slots(&mut registry);
    Arc::new(registry)
}
