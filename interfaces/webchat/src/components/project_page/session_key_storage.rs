//! `project_id -> session_key` mapping, persisted client-side (P2 Task 8).
//!
//! A project room is "a dedicated conversation per project" (spec §6.4), but
//! the server has no `session_key` to hand back until the room's first
//! `chat.send`. `localStorage` is the simplest correct place to remember
//! which backend session a room's chat continues into across a reload — the
//! server itself has no `projects.get`-reachable "this room's session" field
//! to read instead. Mirrors `state::layout`'s `read_persisted_layout_mode` /
//! `persist_layout_mode` cfg-gating (no `web_sys` off wasm32).

#[cfg(target_arch = "wasm32")]
const KEY_PREFIX: &str = "aleph.project_room_session.";

#[cfg(target_arch = "wasm32")]
pub fn load(project_id: &str) -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    storage
        .get_item(&format!("{KEY_PREFIX}{project_id}"))
        .ok()
        .flatten()
}

/// Non-wasm (test host): no localStorage, no `web_sys` (which panics off-wasm).
#[cfg(not(target_arch = "wasm32"))]
pub fn load(_project_id: &str) -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn store(project_id: &str, session_key: &str) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let _ = storage.set_item(&format!("{KEY_PREFIX}{project_id}"), session_key);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn store(_project_id: &str, _session_key: &str) {}
