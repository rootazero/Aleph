//! Native iPhone Memory (Vault) screens. Mirrors the phone Chat/Settings
//! pattern: a Vault list landing (`/memory`) drilling into a read-only note
//! detail (`/memory/note`). Reuses the memory data layer (R4); only the
//! presentation is phone-specific. Vault-only v1 — the Graph/galaxy toggle and
//! the Raw conversation facet stay desktop-only.

pub mod cell;
pub mod detail;
pub mod list;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::AgentsApi;
use crate::api::{CompressedFact, MemoryApi};
use crate::context::DashboardState;
use crate::state::memory::MemoryState;
use crate::views::memory::data::{MemoryFacet, NOTE_WINDOW};

use self::detail::PhoneMemoryDetail;
use self::list::PhoneMemoryList;

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
                if mem.agent_id.get_untracked() != resp.default_id {
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
                match MemoryApi::list_facts(&dashboard, &agent, Some(NOTE_WINDOW), 0).await {
                    Ok(facts) => st.window.set(facts),
                    Err(e) => st.error.set(Some(e)),
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
    move || {
        if location.pathname.get() == "/memory/note" {
            view! { <PhoneMemoryDetail/> }.into_any()
        } else {
            view! { <PhoneMemoryList/> }.into_any()
        }
    }
}
