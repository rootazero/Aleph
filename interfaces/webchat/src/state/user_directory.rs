//! Process-wide `user_id -> display_name` projection, plus the viewer's own
//! id. Fed lazily from `users.list` / `users.me` — the first reader that
//! finds the directory empty triggers a fetch, so no route pays the cost
//! unless it actually renders attribution (project-room chat bubbles,
//! roster pickers).
//!
//! Deliberately NOT a cache with invalidation: `display_name` is free-form
//! text an owner can rename at will (`users.update`), and the P2 surfaces
//! that read this (message attribution, roster rows) tolerate a stale name
//! for the rest of the session — the alternative (re-fetching on every
//! bubble render) is the one thing this type exists to avoid.

use leptos::prelude::*;
use std::collections::HashMap;

use crate::api::users::UsersApi;
use crate::context::DashboardState;

/// Provided once at the app root (mirrors `StoreState` / `TeamsTabState`) so
/// every consumer — message bubbles, project roster UI — shares one fetch.
#[derive(Clone, Copy)]
pub struct UserDirectoryState {
    names: RwSignal<HashMap<String, String>>,
    /// The viewer's own `user_id`, from `users.me`. `None` covers both "not
    /// fetched yet" and "no caller identity" (unrestricted/loopback callers
    /// with no P1 user attached) — both correctly suppress the "is this my
    /// own message" comparison rather than false-negativing it.
    pub my_user_id: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
}

impl Default for UserDirectoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl UserDirectoryState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            names: RwSignal::new(HashMap::new()),
            my_user_id: RwSignal::new(None),
            loading: RwSignal::new(false),
        }
    }

    /// Resolve a display name, falling back to the raw id when unknown (a
    /// deactivated user, or a directory that hasn't loaded yet) — the raw id
    /// is still meaningful to an operator, unlike a blank label.
    #[must_use]
    pub fn display_name(&self, user_id: &str) -> String {
        self.names
            .with(|m| m.get(user_id).cloned())
            .unwrap_or_else(|| user_id.to_string())
    }

    /// Reactive read of every known `(user_id, display_name)` pair, sorted by
    /// display name — the shape a roster picker wants directly.
    #[must_use]
    pub fn all(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.names.get().into_iter().collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    /// Fetch `users.list` + `users.me` once. Idempotent and safe to call from
    /// every consumer's mount effect — a second call while the first is still
    /// in flight, or after the directory is already populated, is a no-op.
    pub fn ensure_loaded(&self, dash: DashboardState) {
        if self.loading.get_untracked() || !self.names.with_untracked(HashMap::is_empty) {
            return;
        }
        self.loading.set(true);
        let this = *self;
        leptos::task::spawn_local(async move {
            if let Ok(users) = UsersApi::list(&dash).await {
                let map = users
                    .into_iter()
                    .map(|u| (u.user_id, u.display_name))
                    .collect();
                this.names.set(map);
            }
            if let Ok(Some(me)) = UsersApi::me(&dash).await {
                this.my_user_id.set(Some(me.user_id));
            }
            this.loading.set(false);
        });
    }
}
