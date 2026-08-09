//! Native iPhone Memory screens. Mirrors the phone Chat/Settings drill-down
//! pattern: a menu landing (`/memory`) — agent selector + Graph/List rows —
//! drilling into the full-screen Graph galaxy (`/memory/graph`), the Vault
//! list (`/memory/list`), and a read-only note detail (`/memory/note`).
//! Reuses the memory data layer (R4); only the presentation is phone-specific.

pub mod cell;
pub mod detail;
pub mod graph;
pub mod list;
pub mod menu;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::AgentsApi;
use crate::api::{CompressedFact, MemoryApi};
use crate::context::DashboardState;
use crate::state::memory::MemoryState;
use crate::views::memory::data::{MemoryFacet, NOTE_WINDOW};

use self::detail::PhoneMemoryDetail;
use self::graph::PhoneMemoryGraph;
use self::list::PhoneMemoryList;
use self::menu::PhoneMemoryMenu;

/// Router-owned state for the phone Memory screens. Every field is an
/// `RwSignal` (Copy), so the struct is `Copy` and travels via context.
#[derive(Clone, Copy)]
pub struct PhoneMemoryState {
    /// One `list_facts` window; faceted + filtered + paginated client-side.
    pub window: RwSignal<Vec<CompressedFact>>,
    pub loaded: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub facet: RwSignal<MemoryFacet>,
    pub query: RwSignal<String>,
    /// Load-more index; the list shows items `0..(page+1)*PAGE_SIZE`.
    pub page: RwSignal<u32>,
    /// The note opened in the detail screen.
    pub selected: RwSignal<Option<CompressedFact>>,
    /// Bumping this re-triggers the note-window loader (used by the Retry cell).
    pub reload_nonce: RwSignal<u32>,
}

/// Phone Memory router. Owns `PhoneMemoryState`, bootstraps the agent, and
/// connect-gated-loads the note window. Renders the list at `/memory` and the
/// detail at `/memory/note`. No streaming subscription (request/response).
#[component]
#[must_use]
pub fn PhoneMemory() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();

    let st = PhoneMemoryState {
        window: RwSignal::new(Vec::new()),
        loaded: RwSignal::new(false),
        error: RwSignal::new(None),
        facet: RwSignal::new(MemoryFacet::AllNotes),
        query: RwSignal::new(String::new()),
        page: RwSignal::new(0),
        selected: RwSignal::new(None),
        reload_nonce: RwSignal::new(0),
    };
    provide_context(st);

    // Agent bootstrap — honor the server default_id (mirrors the wide Memory
    // view). Idempotent: re-runs until `agents` is non-empty.
    Effect::new(move || {
        if !dashboard.is_connected.get() || !mem.agents.get().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&dashboard).await {
                mem.agents.set(resp.agents);
                // Post-`.await`: see `crate::disposed_reads`.
                let Some(current) = mem.agent_id.try_get_untracked() else {
                    return;
                };
                if current != resp.default_id {
                    mem.agent_id.set(resp.default_id);
                }
            }
        });
    });

    // Note-window loader — connect-gated (cold-boot lesson) + per-agent.
    // Also re-runs when `reload_nonce` is bumped (manual Retry from the error cell).
    Effect::new(move || {
        st.reload_nonce.get();
        if dashboard.is_connected.get() {
            let agent = mem.agent_id.get();
            spawn_local(async move {
                st.loaded.set(false);
                st.error.set(None);
                match MemoryApi::list_facts(&dashboard, &agent, NOTE_WINDOW, 0).await {
                    Ok((facts, _total)) => st.window.set(facts),
                    Err(e) => {
                        st.error
                            .set(Some(crate::components::admin_refusal::settings_load_error(
                                i18n,
                                &e,
                                |e| e.to_string(),
                            )))
                    }
                }
                st.loaded.set(true);
                st.page.set(0);
            });
        } else {
            st.window.set(Vec::new());
            st.loaded.set(false);
        }
    });

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        MemoryScreen::Note => view! { <PhoneMemoryDetail/> }.into_any(),
        MemoryScreen::Graph => view! { <PhoneMemoryGraph/> }.into_any(),
        MemoryScreen::List => view! { <PhoneMemoryList/> }.into_any(),
        MemoryScreen::Menu => view! { <PhoneMemoryMenu/> }.into_any(),
    }
}

// ---------------------------------------------------------------------------
// Pure path → screen mapping. Extracted so the routing table is unit-tested
// without the Leptos runtime (mirrors `state::memory::parse_view_param`).
// ---------------------------------------------------------------------------

/// Which phone Memory screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScreen {
    Menu,
    List,
    Graph,
    Note,
}

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> MemoryScreen {
    match path {
        "/memory/note" => MemoryScreen::Note,
        "/memory/graph" => MemoryScreen::Graph,
        "/memory/list" => MemoryScreen::List,
        _ => MemoryScreen::Menu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_for_path_maps_each_route() {
        assert_eq!(screen_for_path("/memory"), MemoryScreen::Menu);
        assert_eq!(screen_for_path("/memory/list"), MemoryScreen::List);
        assert_eq!(screen_for_path("/memory/graph"), MemoryScreen::Graph);
        assert_eq!(screen_for_path("/memory/note"), MemoryScreen::Note);
    }

    #[test]
    fn screen_for_path_unknown_falls_back_to_menu() {
        assert_eq!(screen_for_path("/memory/bogus"), MemoryScreen::Menu);
        assert_eq!(screen_for_path("/"), MemoryScreen::Menu);
    }
}
