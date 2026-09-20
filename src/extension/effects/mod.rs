//! Effects — the reversible half of a plugin's footprint on the running process.
//!
//! One rule (spec §3.1, U8 "C: 效果归 scope、视图归派生"): every registration
//! that has an inverse returns a [`Disposer`]; the caller (`lifecycle.rs`)
//! owns it inside the plugin's [`EffectScope`]; unmount = run the list in
//! reverse. Anything that has no inverse but can be re-derived from the
//! registry (skill dirs, sub-agents, tool index, hook executor) is a *view*
//! and is recomputed by `ExtensionManager::after_transition`, not disposed.
//!
//! Absorbed from Cordis `fiber.ts:418-561` / dsh AGENTS.md "Registrations are
//! effects" as an ownership rule only: no DI container, no Proxy context, no
//! cascade restart (scan-dsh-cordis.md Top-8 #1; three prior rounds' "不引
//! fiber" rulings stand, narrowed to this).

mod disposer;
mod scope;

pub use disposer::{async_disposer, sync_disposer, DisposeOutcome, Disposer};
pub use scope::{DisposeReport, EffectScope, PluginId, STEP_LABELS};
