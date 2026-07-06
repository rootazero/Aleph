//! Process-global live handle for the `[moa]` config section.
//!
//! Mirrors `route_handle`: written at boot from the loaded Config, re-stored
//! by the `moa` tool after a successful preset patch (hot reload), read at
//! run construction. Avoids threading a Config handle through
//! `AgentHarnessRunner` (which holds only boot-time snapshots by design).

use crate::config::MoaToml;
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

/// Serializes tests that mutate the process-global `MOA_CONFIG` slot above.
/// Unlike the per-session handles (keyed maps), this is a single unkeyed
/// slot, so concurrent test threads touching it would race under the default
/// multi-threaded test runner. ONE crate-wide lock lives here, next to the
/// static it guards — private per-module copies would not serialize against
/// each other.
#[cfg(test)]
pub(crate) fn moa_config_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
