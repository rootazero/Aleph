pub mod events;
pub mod state;
pub mod view;

pub use state::HaloState;
pub use view::HaloView;

/// Alias for the promoted chat view (was HaloView).
pub use view::HaloView as ChatView;
