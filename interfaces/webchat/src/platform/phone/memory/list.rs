//! Phone Vault list (`/memory`): search field, facet chips, count line, note
//! cells, and a "Load more" affordance. Reads the router-owned
//! `PhoneMemoryState`; reuses the memory data layer (R4). Tapping a cell stores
//! the note and drills into `/memory/note`.

use crate::i18n::{t, t_string};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::CompressedFact;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::memory::data::{
    bucket_counts, facet_slice, filter_notes, page_slice, MemoryFacet, NOTE_WINDOW, PAGE_SIZE,
};

use super::cell::PhoneMemoryCell;
use super::PhoneMemoryState;

/// The four note-layer facets shown on phone (Raw and the hot tier are
/// desktop-only). Index aligns with `bucket_counts` →
/// `[AllNotes, Facts, Feedback, Lessons]`; labels are resolved through i18n
/// at render time rather than baked in as literals here.
const FACETS: [MemoryFacet; 4] = [
    MemoryFacet::AllNotes,
    MemoryFacet::Facts,
    MemoryFacet::Feedback,
    MemoryFacet::Lessons,
];

#[component]
#[must_use]
pub fn PhoneMemoryList() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let st = expect_context::<PhoneMemoryState>();
    let navigate = use_navigate();

    // Reset pagination whenever the facet or the query changes.
    Effect::new(move || {
        st.facet.get();
        st.query.get();
        st.page.set(0);
    });

    // window → facet_slice → filter_notes  (the faceted, filtered view).
    let visible = move || {
        let w = st.window.get();
        let faceted = facet_slice(&w, st.facet.get());
        filter_notes(&faceted, &st.query.get())
    };
    // Chip badges count the whole window (independent of the search box).
    let counts = move || bucket_counts(&st.window.get());

    view! {
        // `back_label` stays an English literal: the prop is `&'static str`
        // by design (see PhoneShell's doc) and all 21 phone screens pass one.
        // Translating it is a crate-wide type change, not a memory-tab fix.
        <PhoneShell title=t_string!(i18n, memory.phone_title).to_string() back="/memory" back_label="Memory">
        // Single element child for PhoneShell (mixed static+dynamic siblings
        // must live inside one element — see the PhoneShell footgun note).
        <div style="display:flex; flex-direction:column; gap:12px;">
            <input
                class="field"
                type="text"
                placeholder=move || t_string!(i18n, memory.phone_search_placeholder).to_string()
                prop:value=move || st.query.get()
                on:input=move |ev| st.query.set(event_target_value(&ev))
            />

            <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                {FACETS.iter().enumerate().map(|(i, f)| {
                    let f = *f;
                    let label = move || match f {
                        MemoryFacet::Facts => t_string!(i18n, memory.facet_facts).to_string(),
                        MemoryFacet::Feedback => t_string!(i18n, memory.facet_feedback).to_string(),
                        MemoryFacet::Lessons => t_string!(i18n, memory.facet_lessons).to_string(),
                        _ => t_string!(i18n, memory.facet_all_notes).to_string(),
                    };
                    view! {
                        <button
                            class="chip"
                            class:chip-active=move || st.facet.get() == f
                            style="flex:none;"
                            on:click=move |_| st.facet.set(f)
                        >
                            {label}
                            <span class="tabular-nums" style="opacity:0.7;">
                                {move || counts()[i].to_string()}
                            </span>
                        </button>
                    }
                }).collect_view()}
            </div>

            <div style="display:flex; align-items:center; justify-content:space-between; padding:0 2px;">
                <span style="font-size:12px; font-weight:600; letter-spacing:0.03em; text-transform:uppercase; color:var(--color-text-tertiary);">
                    {move || format!("{} {}", visible().len(), t_string!(i18n, memory.phone_count_suffix))}
                </span>
                {move || (st.window.get().len() >= NOTE_WINDOW).then(|| view! {
                    <span style="font-size:11px; color:var(--color-text-tertiary);">{t!(i18n, memory.phone_capped)}</span>
                })}
            </div>

            {move || {
                if !st.loaded.get() {
                    let label = if dashboard.is_connected.get() {
                        t_string!(i18n, memory.phone_loading).to_string()
                    } else {
                        t_string!(i18n, memory.phone_connecting).to_string()
                    };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = st.error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">{t!(i18n, memory.phone_load_failed)}</div><div class="cell-sub">{err}</div></div></div>
                            <div class="cell" on:click=move |_| st.reload_nonce.update(|n| *n += 1)>
                                <div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">{t!(i18n, memory.phone_retry)}</div></div>
                            </div>
                        </div>
                    }.into_any();
                }
                let items = visible();
                if items.is_empty() {
                    return view! { <div class="list-header">{t!(i18n, memory.phone_empty)}</div> }.into_any();
                }
                let total = items.len();
                let shown = (st.page.get() + 1) * PAGE_SIZE; // u32
                let page_items = page_slice(&items, 0, shown);
                view! {
                    <div style="display:flex; flex-direction:column; gap:12px;">
                        <div class="list">
                            {page_items.into_iter().map(|fact: CompressedFact| {
                                let navigate = navigate.clone();
                                let on_open = move |f: CompressedFact| {
                                    st.selected.set(Some(f));
                                    navigate("/memory/note", NavigateOptions::default());
                                };
                                view! { <PhoneMemoryCell fact=fact on_open=Callback::new(on_open)/> }
                            }).collect_view()}
                        </div>
                        {(total > shown as usize).then(|| view! {
                            <button class="chip" style="align-self:center;" on:click=move |_| st.page.update(|p| *p += 1)>
                                {t!(i18n, memory.phone_load_more)}
                            </button>
                        })}
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
