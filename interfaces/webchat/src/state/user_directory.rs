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

    /// Fetch `users.list` + `users.me`. Idempotent and safe to call from every
    /// consumer's component body — a second call while the first is still in
    /// flight, or after the directory is already populated, is a no-op.
    ///
    /// **Waits for the socket.** This registers an effect rather than firing
    /// the fetch inline, and the difference is the whole reason the attribution
    /// label shipped broken: `MessageList` mounts during app boot, *before*
    /// `DashboardState.rpc_tx` exists, so an inline fetch got
    /// `Err("Not connected")` from `rpc_call` before a frame was ever put on
    /// the wire. Nothing retried it — this state is provided once at the app
    /// root, so its emptiness outlived every remount — and the two `if let Ok`
    /// arms below swallowed the error, so there was no console trace either.
    /// The observable was a room bubble labelled `u-5fd9…` instead of
    /// `QA Member`, plus the viewer's OWN messages labelled too (`my_user_id`
    /// stayed `None`, making `author_label`'s self-suppression vacuous). Note
    /// this was not a race that sometimes lost: boot order made it lose every
    /// time, which is why it reproduced 100% and still read as "unimplemented".
    ///
    /// Re-runs on reconnect for free, and the guard makes every run after a
    /// successful load a no-op. Requires a reactive owner — all call sites are
    /// component bodies.
    pub fn ensure_loaded(&self, dash: DashboardState) {
        let this = *self;
        Effect::new(move |_| {
            if !should_fetch(
                dash.is_connected.get(),
                this.loading.get_untracked(),
                this.names.with_untracked(HashMap::is_empty),
            ) {
                return;
            }
            this.loading.set(true);
            leptos::task::spawn_local(async move {
                match UsersApi::list(&dash).await {
                    Ok(users) => {
                        let map = users
                            .into_iter()
                            .map(|u| (u.user_id, u.display_name))
                            .collect();
                        this.names.set(map);
                    }
                    // Not fatal — the label falls back to the raw id — but it
                    // must not be invisible: silence here is what let the
                    // original defect read as "the feature was never built".
                    Err(e) => leptos::logging::warn!("user directory: users.list failed: {e}"),
                }
                match UsersApi::me(&dash).await {
                    Ok(Some(me)) => this.my_user_id.set(Some(me.user_id)),
                    Ok(None) => {}
                    Err(e) => leptos::logging::warn!("user directory: users.me failed: {e}"),
                }
                this.loading.set(false);
            });
        });
    }
}

/// Whether [`UserDirectoryState::ensure_loaded`] should start a fetch.
///
/// Extracted as a pure function so the "wait for the socket" half is pinned by
/// a host test: inside the effect it is three reactive reads whose only
/// observable is a WS frame that either does or does not appear, which is
/// precisely the thing that went unnoticed the first time.
#[must_use]
pub const fn should_fetch(is_connected: bool, loading: bool, names_empty: bool) -> bool {
    is_connected && !loading && names_empty
}

#[cfg(test)]
mod tests {
    use super::should_fetch;

    #[test]
    fn a_disconnected_panel_does_not_burn_its_one_attempt() {
        assert!(
            !should_fetch(false, false, true),
            "firing before the socket exists is the original defect: rpc_call \
             returns Err(\"Not connected\") without sending, and nothing retries"
        );
    }

    #[test]
    fn fetches_once_the_socket_is_up() {
        assert!(should_fetch(true, false, true));
    }

    #[test]
    fn does_not_stampede_or_refetch() {
        assert!(!should_fetch(true, true, true), "already in flight");
        assert!(!should_fetch(true, false, false), "already populated");
    }
}
