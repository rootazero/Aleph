//! Right-pane slot contracts. The app-shell host is introduced separately.
#![deny(unreachable_pub)]

pub mod pane;
mod registry;
mod slot;
mod slots;
pub(crate) mod tray;

pub use pane::RightPane;
pub use registry::{RegistryError, RightPaneRegistry};
pub use slot::{
    EmptyServiceHub, RightPaneSlot, ServiceHub, SlotCtx, SlotRenderCtx, StageOneTopicBus, TopicBus,
    TopicError, TopicSubscription, ValidatingTopicBus, STAGE_ONE_TOPICS,
};
pub(crate) use tray::TrayState;

use std::sync::Arc;

/// Build the stage-one catalog once, then share it without mutable access.
///
/// The app-shell host owns this construction when it is wired in Task 2.
pub fn stage_one_registry() -> Arc<RightPaneRegistry> {
    let mut registry = RightPaneRegistry::new();
    slots::register_stage_one_slots(&mut registry);
    Arc::new(registry)
}
