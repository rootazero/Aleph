include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"));

// Re-export all items from the generated i18n module for ergonomic usage:
// `use crate::i18n::*;` in any file makes `t!`, `use_i18n`, `Locale`,
// `I18nContextProvider`, etc. available directly.
pub use i18n::*;

/// The generated i18n context handle, spelled out as one name.
///
/// `use_i18n()` only works inside a reactive owner, so code that runs in a
/// `'static` closure detached from the component tree (the `run.*` event
/// dispatcher in `views/chat/events.rs`) cannot look the context up on demand
/// — it has to be handed one at wiring time. The handle is `Copy`, so carrying
/// it costs nothing, and resolving through it (rather than snapshotting
/// strings) keeps a mid-session locale switch honest.
pub type I18nCtx = leptos_i18n::I18nContext<Locale>;
