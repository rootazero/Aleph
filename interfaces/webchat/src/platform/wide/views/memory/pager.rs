//! Pagination controls for the memory console: prev / indicator / next plus a
//! page-size selector. Pure presentation — the parent owns `page` and
//! `page_size` and re-fetches when either changes (R4).

use leptos::prelude::*;

use super::data::{notes_truncated, Loadable};
use super::loader::NotesWindow;

use super::data::{has_next_page, page_count, PAGE_SIZES};
use crate::i18n::{t, t_string, use_i18n};

/// `total` is `None` when the row count is unknown; the next button then falls
/// back to "this page came back full, so there is probably more".
#[component]
#[must_use]
pub fn Pager(
    page: RwSignal<u32>,
    page_size: RwSignal<u32>,
    total: Signal<Option<u64>>,
    current_len: Signal<usize>,
) -> impl IntoView {
    let i18n = use_i18n();

    let total_pages =
        Signal::derive(move || total.get().map(|t| page_count(t as usize, page_size.get())));
    let has_prev = Signal::derive(move || page.get() > 0);
    let has_next = Signal::derive(move || {
        has_next_page(
            page.get(),
            total_pages.get(),
            current_len.get(),
            page_size.get(),
        )
    });

    view! {
        <div class="flex items-center justify-end gap-3 pt-1">
            <label class="flex items-center gap-1.5 text-xs text-text-tertiary">
                <span>{move || t_string!(i18n, memory.page_size).to_string()}</span>
                <select
                    class="rounded-md bg-surface-sunken border border-border px-1.5 py-1 text-xs \
                           text-text-primary focus:outline-none focus:border-primary/60"
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                            page_size.set(v);
                            // Row N of the old paging is not row N of the new
                            // one; jumping back to the first page is the only
                            // position that means the same thing either way.
                            page.set(0);
                        }
                    }
                >
                    {PAGE_SIZES.iter().map(|n| {
                        let n = *n;
                        view! {
                            <option value=n.to_string() selected=move || page_size.get() == n>
                                {n.to_string()}
                            </option>
                        }
                    }).collect_view()}
                </select>
            </label>

            {move || {
                if !has_prev.get() && !has_next.get() {
                    return ().into_any();
                }
                let indicator = match total_pages.get() {
                    Some(tp) => format!("{} / {}", page.get() + 1, tp),
                    None => format!("{}", page.get() + 1),
                };
                view! {
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised \
                                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken \
                                   disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || !has_prev.get()
                            on:click=move |_| { let p = page.get(); if p > 0 { page.set(p - 1); } }
                        >
                            {t!(i18n, memory.prev_page)}
                        </button>
                        <span class="px-1 text-sm font-mono text-text-secondary tabular-nums">{indicator}</span>
                        <button
                            class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised \
                                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken \
                                   disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || !has_next.get()
                            on:click=move |_| { if has_next.get() { page.set(page.get() + 1); } }
                        >
                            {t!(i18n, memory.next_page)}
                        </button>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// "Loaded N of M · Load more" for the note window.
///
/// A pagination affordance, so it lives beside the pager rather than inline in
/// the console's orchestration. It replaced a static "showing the first 1000"
/// italic: `memory.listFacts` has always accepted `offset`, every caller sent a
/// hard-coded `0`, and that one line of text was all that stood between the
/// user and note 1001.
///
/// Renders nothing when the window already holds everything — including when
/// the total is unknown and the window came back short, which is
/// [`super::data::notes_truncated`]'s job to decide (an un-upgraded core sends
/// no total, and "unknown" must not read as "definitely complete").
#[component]
pub fn LoadMoreNotes(
    notes: RwSignal<Loadable<NotesWindow>>,
    /// True while an append is in flight. Deliberately not the slot's own
    /// `Loading` arm: that renders skeletons in place of the list, and blanking
    /// the rows the user is reading in order to add to them is worse than a
    /// busy button.
    busy: RwSignal<bool>,
    on_more: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || notes.get().as_ready()
            .filter(|w| notes_truncated(w.total, w.facts.len()))
            .map(|w| {
                let loaded = w.facts.len();
                let total = w.total;
                view! {
                    <div class="flex items-center gap-3 pt-1 text-xs">
                        <span class="text-text-tertiary">
                            {move || total.map_or_else(
                                || t_string!(i18n, memory.notes_load_more).to_string(),
                                |t| t_string!(i18n, memory.notes_loaded_of)
                                    .replace("{loaded}", &loaded.to_string())
                                    .replace("{total}", &t.to_string()),
                            )}
                        </span>
                        <button
                            class="text-primary hover:underline disabled:opacity-50"
                            prop:disabled=move || busy.get()
                            on:click=move |_| on_more.run(())
                        >
                            {move || if busy.get() {
                                t_string!(i18n, memory.notes_loading_more).to_string()
                            } else {
                                t_string!(i18n, memory.notes_load_more).to_string()
                            }}
                        </button>
                    </div>
                }
            })}
    }
}
