//! The curated hot tier (`MEMORY.md`) — the memory console's third pillar.
//!
//! This layer is what `remember` writes and what the prompt builder injects
//! into **every** turn, which makes it the tier a user most wants to inspect
//! and the only one that had no surface at all: the console showed notes and
//! raw conversation rows, and the block the model actually reads on every
//! request was invisible from every client.
//!
//! ## Manage, don't author
//!
//! There is no "add entry" control, mirroring the notes layer's deliberate
//! lack of a "new note" button (R7/R8): what is worth remembering is the
//! model's call through `remember`, and a hand-authored entry would be a
//! second, unaccountable producer. Correcting a wrong entry and dropping a
//! stale one are management, and those are here.
//!
//! ## The ledger below the entries
//!
//! `memory_write_decisions` records **every** attempted curated write —
//! including the refused ones — which is the only place the question "why was
//! this never remembered?" is answerable. The tool face (`memory_trace`) has
//! read it since it was written; no client ever could, because the Panel's
//! `TraceKind` had no `WriteDecision` variant to name.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use super::toast::{push_toast, ToastKind, ToastMsg};
use crate::api::{CuratedSnapshot, MemoryApi, TraceKind, WriteDecisionRow};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// How many ledger rows to ask for. The ledger is a recent-history read, not
/// an archive browser — an unbounded fetch here would grow with the agent's
/// whole lifetime for a section that lives under a fold.
const LEDGER_LIMIT: usize = 20;

/// The curated facet: budget header, entry list with per-entry edit/remove,
/// and the write-decision ledger.
#[component]
pub fn CuratedPanel(
    state: DashboardState,
    /// Base agent id (never a composed partition — the server refuses those
    /// and composes the caller's own scope itself).
    agent: Signal<String>,
    /// Bumped by the console's Refresh control.
    refresh_nonce: RwSignal<u32>,
    toast_slot: RwSignal<Option<ToastMsg>>,
    /// Reports the entry count up to the facet chip. `None` while the fetch is
    /// in flight or failed — the chip renders a blank badge there, because a
    /// `0` would claim an empty hot tier on the strength of a failed read.
    on_count: Callback<Option<usize>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let snapshot = RwSignal::new(Loadable::<CuratedSnapshot>::Loading);
    let ledger = RwSignal::new(Loadable::<Vec<WriteDecisionRow>>::Loading);
    // Which entry is open in the inline editor, addressed by its ORIGINAL
    // text: that string is also how the server addresses it (`old_text` is a
    // `match_unique` substring), so an index would be a second addressing
    // scheme that drifts the moment the list re-sorts under a concurrent
    // `remember` call.
    let editing = RwSignal::new(None::<String>);
    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let ledger_open = RwSignal::new(false);

    Effect::new(move || {
        refresh_nonce.get();
        let agent = agent.get();
        if !state.is_connected.get() {
            snapshot.set(Loadable::Loading);
            return;
        }
        snapshot.set(Loadable::Loading);
        editing.set(None);
        on_count.run(None);
        spawn_local(async move {
            let res = MemoryApi::curated_list(&state, &agent).await;
            on_count.run(res.as_ref().ok().map(|s| s.entries.len()));
            snapshot.set(Loadable::from_rpc(res));
        });
    });

    // The ledger is a second, independent fetch: it must not be able to blank
    // the entry list, and a store with no ledger backend at all still shows
    // its entries.
    Effect::new(move || {
        refresh_nonce.get();
        if !ledger_open.get() {
            return;
        }
        let agent = agent.get();
        if !state.is_connected.get() {
            return;
        }
        ledger.set(Loadable::Loading);
        spawn_local(async move {
            // An empty `target` is the ledger's "no subject filter" form —
            // `recent_write_decisions` drops a blank filter — so this asks for
            // the agent's recent decisions rather than one fact's history.
            // Pinned server-side by
            // `an_empty_write_decision_target_lists_the_recent_ledger`.
            let res = MemoryApi::trace(&state, &agent, "", TraceKind::WriteDecision, LEDGER_LIMIT)
                .await
                .map(|t| t.write_decisions);
            ledger.set(Loadable::from_rpc(res));
        });
    });

    let apply = move |old_text: String, new_text: Option<String>| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let agent = agent.get_untracked();
        spawn_local(async move {
            let res = match &new_text {
                Some(content) => {
                    MemoryApi::curated_replace(&state, &agent, &old_text, content).await
                }
                None => MemoryApi::curated_remove(&state, &agent, &old_text).await,
            };
            // Post-`.await`: see `crate::disposed_reads`. `try_set` hands the
            // value back when the signal is gone.
            if busy.try_set(false).is_some() {
                return;
            }
            match res {
                // The mutation answered with the whole post-write state, so
                // the list is the server's own — not a locally patched guess
                // that a concurrent `remember` could have already invalidated.
                Ok(snap) => {
                    on_count.run(Some(snap.entries.len()));
                    let _ = snapshot.try_set(Loadable::Ready(snap));
                    let _ = editing.try_set(None);
                    push_toast(
                        toast_slot,
                        t_string!(i18n, memory.toast_saved).to_string(),
                        ToastKind::Success,
                    );
                }
                // Substring matched nothing / matched two entries / would blow
                // the budget: all of those are things the operator can fix by
                // sending different text, and the server says which.
                Err(e) => push_toast(toast_slot, e, ToastKind::Error),
            }
        });
    };

    view! {
        <div class="space-y-3">
            {move || match snapshot.get() {
                Loadable::Loading => view! {
                    <div class="space-y-2">
                        {(0..3).map(|_| view! {
                            <div class="h-12 rounded-lg bg-surface-raised animate-pulse"></div>
                        }).collect_view()}
                    </div>
                }.into_any(),
                Loadable::Failed(e) => view! {
                    <div class="rounded-lg border border-error/40 bg-error-subtle p-4 space-y-2">
                        <p class="text-sm text-error">{t!(i18n, memory.load_failed)}</p>
                        <p class="text-xs font-mono text-text-tertiary break-words">{e}</p>
                        <button
                            class="text-xs text-primary hover:underline"
                            on:click=move |_| refresh_nonce.update(|n| *n += 1)
                        >
                            {t!(i18n, memory.refresh)}
                        </button>
                    </div>
                }.into_any(),
                Loadable::Ready(snap) => {
                    let entries = snap.entries.clone();
                    let legacy = snap.legacy;
                    let (used, limit, pct) = (snap.usage_chars, snap.limit, snap.usage_pct);
                    view! {
                        <BudgetBar used=used limit=limit pct=pct />
                        {legacy.then(|| view! {
                            <p class="text-xs text-warning bg-warning-subtle rounded-lg px-3 py-2">
                                {t!(i18n, memory.curated_legacy)}
                            </p>
                        })}
                        {if entries.is_empty() {
                            view! {
                                <p class="text-sm text-text-tertiary py-8 text-center">
                                    {t!(i18n, memory.curated_empty)}
                                </p>
                            }.into_any()
                        } else {
                            view! {
                                <ul class="space-y-2">
                                    {entries.into_iter().map(|entry| {
                                        let text = entry.text.clone();
                                        let for_edit = text.clone();
                                        let for_remove = text.clone();
                                        let is_editing = {
                                            let t = text.clone();
                                            Signal::derive(move || editing.get().as_deref() == Some(t.as_str()))
                                        };
                                        view! {
                                            <li class="rounded-lg border border-border bg-surface-raised p-3 space-y-2">
                                                <Show
                                                    when=move || is_editing.get()
                                                    fallback=move || {
                                                        let shown = text.clone();
                                                        view! {
                                                            <p class="text-sm text-text-primary whitespace-pre-wrap break-words">
                                                                {shown}
                                                            </p>
                                                        }
                                                    }
                                                >
                                                    <textarea
                                                        class="w-full text-sm bg-surface border border-border rounded-md p-2 font-mono"
                                                        rows="3"
                                                        prop:value=move || draft.get()
                                                        on:input=move |ev| draft.set(event_target_value(&ev))
                                                    />
                                                </Show>
                                                <div class="flex items-center gap-3 text-xs">
                                                    <span class="font-mono text-text-tertiary tabular-nums">
                                                        {entry.chars}" "{move || t_string!(i18n, memory.curated_chars).to_string()}
                                                    </span>
                                                    <span class="flex-1"></span>
                                                    <Show
                                                        when=move || is_editing.get()
                                                        fallback={
                                                            let for_edit = for_edit.clone();
                                                            let for_remove = for_remove.clone();
                                                            move || {
                                                                let open = for_edit.clone();
                                                                let drop_it = for_remove.clone();
                                                                view! {
                                                                    <button
                                                                        class="text-primary hover:underline disabled:opacity-50"
                                                                        prop:disabled=move || busy.get()
                                                                        on:click=move |_| {
                                                                            draft.set(open.clone());
                                                                            editing.set(Some(open.clone()));
                                                                        }
                                                                    >
                                                                        {t!(i18n, memory.edit)}
                                                                    </button>
                                                                    <button
                                                                        class="text-error hover:underline disabled:opacity-50"
                                                                        prop:disabled=move || busy.get()
                                                                        on:click={
                                                                            let drop_it = drop_it.clone();
                                                                            move |_| apply(drop_it.clone(), None)
                                                                        }
                                                                    >
                                                                        {t!(i18n, memory.curated_remove)}
                                                                    </button>
                                                                }
                                                            }
                                                        }
                                                    >
                                                        {
                                                            let save_target = for_edit.clone();
                                                            view! {
                                                                <button
                                                                    class="text-primary hover:underline disabled:opacity-50"
                                                                    prop:disabled=move || busy.get()
                                                                    on:click={
                                                                        let save_target = save_target.clone();
                                                                        move |_| apply(
                                                                            save_target.clone(),
                                                                            Some(draft.get_untracked()),
                                                                        )
                                                                    }
                                                                >
                                                                    {move || if busy.get() {
                                                                        t_string!(i18n, memory.saving).to_string()
                                                                    } else {
                                                                        t_string!(i18n, memory.save).to_string()
                                                                    }}
                                                                </button>
                                                                <button
                                                                    class="text-text-tertiary hover:underline"
                                                                    on:click=move |_| editing.set(None)
                                                                >
                                                                    {t!(i18n, memory.cancel)}
                                                                </button>
                                                            }
                                                        }
                                                    </Show>
                                                </div>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
                    }.into_any()
                }
            }}

            <LedgerSection open=ledger_open ledger=ledger />
        </div>
    }
}

/// Budget header: how much of the always-on block is spent.
///
/// Chars, not bytes — the store bills chars (a byte count hands a CJK user a
/// third of the advertised budget), so the bar has to speak the same unit the
/// refusal will.
#[component]
fn BudgetBar(used: usize, limit: usize, pct: u8) -> impl IntoView {
    let i18n = use_i18n();
    let tone = if pct >= 90 {
        "bg-error"
    } else if pct >= 70 {
        "bg-warning"
    } else {
        "bg-primary"
    };
    view! {
        <div class="space-y-1">
            <div class="flex items-baseline gap-2 text-xs">
                <span class="text-text-secondary">{t!(i18n, memory.curated_budget)}</span>
                <span class="font-mono text-text-tertiary tabular-nums">
                    {used}" / "{limit}
                </span>
                <span class="flex-1"></span>
                <span class="font-mono text-text-tertiary tabular-nums">{pct}"%"</span>
            </div>
            <div class="h-1.5 w-full rounded-full bg-surface-raised overflow-hidden">
                <div
                    class=format!("h-full {tone} transition-all")
                    style=format!("width: {}%", pct.min(100))
                ></div>
            </div>
        </div>
    }
}

/// Collapsible write-decision ledger.
#[component]
fn LedgerSection(
    open: RwSignal<bool>,
    ledger: RwSignal<Loadable<Vec<WriteDecisionRow>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="pt-2 border-t border-border">
            <button
                class="flex items-center gap-1.5 text-xs text-text-secondary hover:text-text-primary"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <span class="font-mono">{move || if open.get() { "▾" } else { "▸" }}</span>
                {t!(i18n, memory.curated_ledger)}
            </button>
            <Show when=move || open.get()>
                <p class="text-[11px] text-text-tertiary pt-1 pb-2">
                    {t!(i18n, memory.curated_ledger_hint)}
                </p>
                {move || match ledger.get() {
                    Loadable::Loading => view! {
                        <div class="h-8 rounded bg-surface-raised animate-pulse"></div>
                    }.into_any(),
                    Loadable::Failed(e) => view! {
                        <p class="text-xs font-mono text-error break-words">{e}</p>
                    }.into_any(),
                    Loadable::Ready(rows) if rows.is_empty() => view! {
                        <p class="text-xs text-text-tertiary">{t!(i18n, memory.curated_ledger_empty)}</p>
                    }.into_any(),
                    Loadable::Ready(rows) => view! {
                        <ul class="space-y-1">
                            {rows.into_iter().map(|row| view! {
                                <li class="flex items-baseline gap-2 text-xs">
                                    <span class="font-mono text-[10px] uppercase text-text-tertiary w-16 flex-shrink-0">
                                        {row.action}
                                    </span>
                                    // The reason is a server-side enum, never
                                    // free text: rendering it verbatim keeps
                                    // the Panel from inventing a vocabulary
                                    // the tool face does not share.
                                    <span class="font-mono text-[10px] text-primary flex-shrink-0">
                                        {row.reason}
                                    </span>
                                    <span class="text-text-secondary truncate">{row.subject}</span>
                                </li>
                            }).collect_view()}
                        </ul>
                    }.into_any(),
                }}
            </Show>
        </div>
    }
}
