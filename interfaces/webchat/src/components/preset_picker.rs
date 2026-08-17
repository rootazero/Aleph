//! The shared "add a provider" disclosure — a button that reveals a searchable,
//! keyboard-walkable list of the catalogue rows you have not set up yet.
//!
//! # Why one component and not one per page
//!
//! Two settings pages ship a catalogue that outgrew its left panel: chat (56
//! rows) and generation (44 across five modality tabs). Written twice, "which
//! row does Enter take" gets two answers and nobody ever compares them — the
//! same failure the shared matcher in [`aleph_protocol::providers::search`] was
//! introduced to end one layer down. This owns the half that has nothing to do
//! with any particular catalogue: disclosure state, the highlight,
//! ↑/↓/Enter/Esc, scrolling the lit row into view, and the empty state.
//!
//! The embedding page deliberately does **not** use this. Its five rows carry
//! `model · <n>d`, and the dimension is what decides whether the vectors
//! already in the memory store survive a switch — so those rows exist to be
//! compared with each other, and collapsing them behind a disclosure removes
//! their only purpose. A catalogue is hideable when its rows are independent
//! alternatives; this one is not.
//!
//! # The seam: this owns interaction, the page owns what is offerable
//!
//! [`PresetPicker`] takes an `offer` closure — "given this query, which rows do
//! you offer, in what order" — rather than a row list plus a
//! [`Searchable`](aleph_protocol::providers::Searchable) bound. Two reasons,
//! both load-bearing:
//!
//! * The chat catalogue ranks through `filter_catalog`, which can surface a
//!   provider because one *model in its roster* matched and then narrows the
//!   roster to the matching ids. Flattening rows to [`PickerRow`] before
//!   filtering would delete that: a `PickerRow` has no children, so searching
//!   `sonnet` would stop finding Anthropic.
//! * Each page excludes different rows for different reasons (MoA has no config
//!   section; OAuth rows have an always-visible section of their own;
//!   generation rows must stay inside the selected modality tab). Those belong
//!   next to the page that can justify them, not in a component that would have
//!   to carry a flag per page.
//!
//! The one contract the closure owes is stated on the prop and unit-tested per
//! page: **an empty query returns everything**. A catalogue that only appeared
//! after you typed would require knowing a vendor's name before you could learn
//! Aleph supports it.
//!
//! # One owner for the highlight index
//!
//! [`step_highlight`] is the only thing that moves the selection, and it
//! clamps. The alternative — bumping an unbounded counter on ↓ and clamping
//! again at the point of use — is what [`super::command_palette`] does, and
//! there the two clamps disagree: pressing ↓ past the end walks the highlight
//! off the list while Enter still runs the last row, so what is lit is not what
//! fires.

use leptos::html::Input;
use leptos::prelude::*;

use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::i18n::{t, t_string, use_i18n};

/// One offerable row, reduced to what a picker draws.
///
/// Deliberately not generic over the page's own row type: everything below the
/// `offer` closure is presentation, and a component that knew about
/// `CatalogEntry` or `EmbeddingPresetEntry` could not be shared by the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRow {
    /// Opaque to this component — handed straight back to `on_choose`. Pages
    /// use whatever they address a row by (a catalogue id, a `__preset__` key).
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub icon_color: String,
    /// Emoji/glyph for the icon tile; `None` falls back to the first character
    /// of `name`, which is what the chat catalogue rows use.
    pub icon_glyph: Option<String>,
    /// Whether the operator already has this one set up. Only marks the row —
    /// configured rows stay offerable so that search can still find them and
    /// so a deleted provider can be set up again.
    pub configured: bool,
    pub badge: BadgeState,
}

/// Move the highlight by `delta`, clamped to the list.
///
/// Returns 0 for an empty list: there is no row to light, and every caller
/// checks emptiness before indexing anyway.
#[must_use]
pub fn step_highlight(len: usize, cur: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    cur.saturating_add_signed(delta).min(len - 1)
}

/// DOM id of the highlighted row, so ↓ past the fold scrolls rather than
/// walking the selection out of sight.
///
/// A bare document id, deliberately: no page mounts two of these at once, and
/// a namespace parameter for a collision that does not exist is an abstraction
/// with zero consumers.
fn row_dom_id(index: usize) -> String {
    format!("aleph-picker-row-{index}")
}

/// Bring the highlighted row into view. Best-effort: a missing element simply
/// means the list re-rendered underneath us, which the next keypress fixes.
///
/// `block: nearest` scrolls the row's own list and stops. The argument-less
/// overload aligns to an edge and walks **every** scrollable ancestor, so each
/// ArrowDown would also jerk the settings panel this disclosure sits inside.
fn scroll_row_into_view(index: usize) {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&row_dom_id(index)))
    else {
        return;
    };
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

/// Button + the searchable catalogue it reveals.
///
/// `open` is owned by the parent because a first-load seed — expand when the
/// operator has configured nothing — has to reach it from wherever that page's
/// catalogue fetch lands.
#[component]
pub fn PresetPicker(
    /// Rows to offer for a query, best match first. **An empty query must
    /// return every offerable row**, in the catalogue's own order.
    offer: impl Fn(&str) -> Vec<PickerRow> + Copy + Send + Sync + 'static,
    /// Receives the chosen row's [`PickerRow::id`]. Choosing never writes
    /// config — it selects, exactly as clicking a card in the left panel does.
    on_choose: impl Fn(String) + Copy + Send + Sync + 'static,
    /// Whether the disclosure is expanded.
    open: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let search = RwSignal::new(String::new());
    let highlight = RwSignal::new(0usize);
    let search_ref = NodeRef::<Input>::new();

    let rows = Memo::new(move |_| offer(&search.get()));

    // A new list means a new first row, and a bare Enter takes the first row.
    // Carrying the old index over would confirm whatever happens to sit at that
    // offset in a list the operator has not looked at yet.
    //
    // Tracks `rows`, not `search`, because the invariant is about the list the
    // highlight indexes into, not about which upstream signal happened to
    // change it: a highlight left pointing past the end lights nothing while
    // Enter still fires the clamped last row. `rows` is a memo, so a keystroke
    // that leaves the offered set identical does not reset anything.
    Effect::new(move |_| {
        rows.track();
        highlight.set(0);
    });

    // Focus the search box when the disclosure opens — the whole point of the
    // button is that the next thing you do is type.
    Effect::new(move |_| {
        if open.get() {
            if let Some(el) = search_ref.get() {
                let _ = el.focus();
            }
        }
    });

    let choose = move |row: &PickerRow| {
        on_choose(row.id.clone());
        open.set(false);
        search.set(String::new());
    };

    // Scoped to this subtree, not the window, for two independent reasons:
    //
    // 1. `window_event_listener` registers **no** cleanup — its handle has to
    //    be removed by hand. Every page mounting this unmounts on navigation,
    //    so a window listener would leak one per visit and each stale copy
    //    would go on reading `open`, a signal its owner has disposed. (The
    //    palette gets away with it only because it is mounted at the app root
    //    forever.)
    // 2. The disclosure can be open while the operator is typing in the detail
    //    editor on the right. A window-level Enter would confirm a catalogue
    //    row out from under a form they were submitting.
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if !open.get_untracked() {
            return;
        }
        let len = rows.get_untracked().len();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                let next = step_highlight(len, highlight.get_untracked(), 1);
                highlight.set(next);
                scroll_row_into_view(next);
            }
            "ArrowUp" => {
                ev.prevent_default();
                let next = step_highlight(len, highlight.get_untracked(), -1);
                highlight.set(next);
                scroll_row_into_view(next);
            }
            "Enter" => {
                ev.prevent_default();
                let current = rows.get_untracked();
                // Clamp on read as well: the list can shrink between the last
                // keypress and this one (the query is still being typed).
                if let Some(row) =
                    current.get(step_highlight(current.len(), highlight.get_untracked(), 0))
                {
                    choose(row);
                }
            }
            "Escape" => {
                ev.prevent_default();
                open.set(false);
                search.set(String::new());
            }
            _ => {}
        }
    };

    view! {
        <div on:keydown=on_key>
            <button
                on:click=move |_| {
                    let next = !open.get_untracked();
                    open.set(next);
                    if !next {
                        search.set(String::new());
                    }
                }
                class="w-full px-4 py-3 rounded-lg border-2 border-dashed border-border text-text-secondary hover:border-primary hover:text-primary transition-colors flex items-center justify-center gap-2"
            >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                        d=move || if open.get() { "M19 15l-7-7-7 7" } else { "M12 5v14M5 12h14" } />
                </svg>
                {t!(i18n, settings.picker.add)}
            </button>

            <Show when=move || open.get() fallback=|| ()>
                <div class="mt-3 border border-border rounded-lg bg-surface-sunken overflow-hidden">
                    <div class="p-3 border-b border-border">
                        <input
                            node_ref=search_ref
                            type="text"
                            prop:value=move || search.get()
                            on:input=move |ev| search.set(event_target_value(&ev))
                            placeholder=move || t_string!(i18n, settings.picker.search).to_string()
                            class="w-full px-3 py-2 bg-surface-raised border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                        <div class="mt-2 text-xs text-text-tertiary">
                            {t!(i18n, settings.picker.hint)}
                        </div>
                    </div>

                    // No padding, and dividers instead of gaps: the rows are
                    // members of this bordered container, not cards floating
                    // inside it. Padded cards drew a third border, inset from
                    // both the container above them and the configured-provider
                    // rows below, so scanning the column meant tracking an edge
                    // that kept stepping in and out.
                    <div class="max-h-96 overflow-y-auto">
                        {move || {
                            let items = rows.get();
                            if items.is_empty() {
                                return view! {
                                    <div class="px-3 py-6 text-center text-sm text-text-tertiary">
                                        {t!(i18n, settings.picker.empty)}
                                    </div>
                                }.into_any();
                            }
                            view! {
                                <div class="grid grid-cols-1 divide-y divide-border">
                                    {items.into_iter().enumerate().map(|(idx, row)| {
                                        let held = StoredValue::new(row.clone());
                                        let configured = row.configured;
                                        let badge = row.badge;
                                        view! {
                                            // `grid`, not a bare wrapper: the
                                            // id anchor this div exists for
                                            // would otherwise become the grid
                                            // item and leave the button inside
                                            // it inline-block, so every row
                                            // would shrink to its own text
                                            // width. One column stretches the
                                            // button back to full width, which
                                            // is what the sibling sections get
                                            // by being direct grid children.
                                            <div id=row_dom_id(idx) class="grid">
                                                <ProviderRowCard
                                                    flush=true
                                                    name=row.name.clone()
                                                    icon_color=row.icon_color.clone()
                                                    icon_glyph=row.icon_glyph.clone()
                                                    subtitle=row.subtitle.clone()
                                                    is_selected=move || highlight.get() == idx
                                                    is_configured=move || configured
                                                    dot=move || if configured { RowDot::Inactive } else { RowDot::None }
                                                    badge=move || view! {
                                                        <span class="flex items-center gap-1">
                                                            {configured.then(|| view! {
                                                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary border border-border">
                                                                    {t!(i18n, settings.picker.configured)}
                                                                </span>
                                                            })}
                                                            <ProviderBadges state=badge />
                                                        </span>
                                                    }.into_any()
                                                    on_click=move || held.with_value(|r| choose(r))
                                                />
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_past_the_end_stays_on_the_last_row() {
        assert_eq!(step_highlight(3, 2, 1), 2);
    }

    #[test]
    fn stepping_before_the_first_row_stays_on_the_first() {
        assert_eq!(step_highlight(3, 0, -1), 0);
    }

    #[test]
    fn stepping_walks_one_row_at_a_time() {
        assert_eq!(step_highlight(5, 1, 1), 2);
        assert_eq!(step_highlight(5, 3, -1), 2);
    }

    #[test]
    fn stepping_an_empty_list_is_zero() {
        assert_eq!(step_highlight(0, 4, 1), 0);
        assert_eq!(step_highlight(0, 0, -1), 0);
    }

    #[test]
    fn a_shrinking_list_pulls_the_highlight_back_in_range() {
        // The query narrowed the list under a highlight that was past its new
        // end; a delta of 0 is the read-side clamp Enter performs.
        assert_eq!(step_highlight(2, 9, 0), 1);
    }
}
