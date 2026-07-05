//! Process-global live handle for the `[moa]` config section.
//!
//! Mirrors `route_handle`: written at boot from the loaded Config, re-stored
//! by the `moa` tool after a successful preset patch (hot reload), read at
//! run construction. Avoids threading a Config handle through
//! `AgentHarnessRunner` (which holds only boot-time snapshots by design).

use crate::config::types::moa::MoaToml;
use crate::sync_primitives::RwLock;
use std::sync::OnceLock;

static MOA_CONFIG: OnceLock<RwLock<Option<MoaToml>>> = OnceLock::new();

fn cell() -> &'static RwLock<Option<MoaToml>> {
    MOA_CONFIG.get_or_init(|| RwLock::new(None))
}

/// Publish the current `[moa]` section (boot + after config patches).
pub fn store_moa_config(cfg: Option<MoaToml>) {
    *cell().write().unwrap_or_else(|e| e.into_inner()) = cfg;
}

/// Snapshot of the current `[moa]` section.
#[must_use]
pub fn get_moa_config() -> Option<MoaToml> {
    cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}
