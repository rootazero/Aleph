//! Card renderers for the memory console.
//!
//! Replaces the two HTML tables this view used to carry. Tables clamped every
//! row to one or two lines and overflowed on narrow windows; a card can show
//! the fields `memory.listFacts` stopped discarding (tags, link count, both
//! timestamps) and, for raw rows, both halves of a turn.
//!
//! Pure presentation: every mutation is a callback the parent owns (R4).

use leptos::prelude::*;

use super::data::{format_ts, Loadable};
use crate::api::{CompressedFact, RawMemory};
use crate::canvas_engine::category_color::category_color;
use crate::components::ui::{Badge, BadgeVariant, ConfirmButton};
use crate::i18n::{t, t_string, use_i18n};

/// Category → badge tone. Mirrors the stripe colour but in the panel's badge
/// vocabulary, so a note reads the same in a card as it does in the galaxy.
fn category_variant(category: &str) -> BadgeVariant {
    match category {
        "preference" | "personal" => BadgeVariant::Indigo,
        "learning" | "lesson" | "skill" | "goal-lessons" => BadgeVariant::Emerald,
        "plan" | "project" => BadgeVariant::Amber,
        "feedback" => BadgeVariant::Red,
        _ => BadgeVariant::Slate,
    }
}

// ─── Three-state shell ──────────────────────────────────────────────────────

/// Wraps a card list with its loading / failed / empty states.
///
/// `state` carries the row count so the shell can tell "loaded and empty" from
/// "still loading" from "failed" — the distinction the old table could not make,
/// because a failed fetch and an empty store both rendered as "No facts".
#[component]
#[must_use]
pub fn CardListShell(
    state: Signal<Loadable<usize>>,
    empty_label: Signal<String>,
    on_retry: impl Fn() + Clone + Send + 'static,
    children: ChildrenFn,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || match state.get() {
            Loadable::Loading => view! {
                <div class="space-y-2" aria-busy="true">
                    {(0..5).map(|_| view! {
                        <div class="h-[82px] rounded-xl bg-surface-sunken animate-pulse"></div>
                    }).collect_view()}
                </div>
            }.into_any(),

            Loadable::Failed(err) => {
                let on_retry = on_retry.clone();
                view! {
                    <div class="rounded-xl border border-danger/20 bg-danger-subtle p-6 flex items-start gap-4">
                        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                             class="text-danger flex-shrink-0 mt-0.5">
                            <circle cx="12" cy="12" r="10" />
                            <line x1="12" y1="8" x2="12" y2="12" />
                            <line x1="12" y1="16" x2="12.01" y2="16" />
                        </svg>
                        <div class="min-w-0">
                            <h3 class="text-danger font-semibold mb-1">{t!(i18n, memory.load_failed)}</h3>
                            <p class="text-xs text-text-secondary break-words font-mono">{err}</p>
                            <button
                                class="mt-3 px-3 py-1 text-xs rounded-lg border border-border \
                                       text-text-secondary hover:text-text-primary"
                                on:click=move |_| on_retry()
                            >
                                {t!(i18n, memory.retry)}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }

            Loadable::Ready(0) => view! {
                <div class="rounded-xl border border-border-subtle bg-surface-raised p-10 text-center">
                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"
                         class="mx-auto mb-3 text-text-tertiary">
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    </svg>
                    <p class="text-sm text-text-tertiary">{move || empty_label.get()}</p>
                </div>
            }.into_any(),

            Loadable::Ready(_) => view! { <div class="space-y-2">{children()}</div> }.into_any(),
        }}
    }
}

// ─── Note card ──────────────────────────────────────────────────────────────

#[component]
#[must_use]
pub fn NoteCard(
    fact: CompressedFact,
    checked: Signal<bool>,
    highlighted: Signal<bool>,
    on_toggle: impl Fn() + Clone + Send + 'static,
    on_open: impl Fn() + Clone + Send + 'static,
    on_locate: impl Fn() + Clone + Send + 'static,
    on_copy_link: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let stripe = category_color(&fact.category);
    let variant = category_variant(&fact.category);

    let title = fact.content.clone();
    let path = fact.path.clone();
    let agent_id = fact.agent_id.clone();
    let category = fact.category.clone();
    let tags = fact.tags.clone();
    let link_count = fact.link_count;
    let created = format_ts(fact.created_at);
    // Only show "updated" when it actually differs — a note written once
    // carries updated_at == created_at and repeating it is noise.
    let updated = (fact.updated_at > fact.created_at).then(|| format_ts(fact.updated_at));

    view! {
        <div
            class=move || {
                let base = "group relative flex items-start gap-3 rounded-xl border bg-surface-raised \
                            p-4 pl-5 cursor-pointer transition-colors";
                if highlighted.get() {
                    format!("{base} border-primary/50 ring-1 ring-primary/30")
                } else {
                    format!("{base} border-border-subtle hover:bg-surface-sunken")
                }
            }
            on:click={
                let on_open = on_open.clone();
                move |_| on_open()
            }
        >
            <div
                class="absolute left-0 top-3 bottom-3 w-[3px] rounded-full"
                style=format!("background:{stripe}")
            ></div>

            <input
                type="checkbox"
                class="mt-0.5 cursor-pointer flex-shrink-0"
                aria-label=t_string!(i18n, memory.batch_selected)
                prop:checked=move || checked.get()
                on:click=move |ev| ev.stop_propagation()
                on:change=move |_| on_toggle()
            />

            <div class="min-w-0 flex-1">
                <div class="text-sm font-medium text-text-primary break-words">{title}</div>
                <div class="text-xs text-text-tertiary font-mono mt-0.5 break-all">{path}</div>

                <div class="flex items-center gap-1.5 flex-wrap mt-2">
                    <Badge variant=variant>{category}</Badge>
                    <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
                    {tags.into_iter().map(|tag| view! {
                        <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-surface-sunken \
                                     text-text-secondary border border-border">
                            {tag}
                        </span>
                    }).collect_view()}
                    {(link_count > 0).then(|| view! {
                        <span class="inline-flex items-center gap-1 text-[10px] text-text-tertiary font-mono tabular-nums">
                            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                                <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                            </svg>
                            {link_count.to_string()}" "{move || t_string!(i18n, memory.links).to_string()}
                        </span>
                    })}
                    <span class="text-[10px] text-text-tertiary font-mono tabular-nums">
                        {move || t_string!(i18n, memory.created).to_string()}" "{created.clone()}
                        {move || updated.clone().map(|u| format!(" · {} {u}", t_string!(i18n, memory.updated)))}
                    </span>
                </div>
            </div>

            <div class="flex items-center gap-1 flex-shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                    class="p-1.5 rounded text-text-tertiary hover:text-primary"
                    title=t_string!(i18n, memory.copy_link)
                    on:click=move |ev| { ev.stop_propagation(); on_copy_link(); }
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                        <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                    </svg>
                </button>
                <button
                    class="p-1.5 rounded text-text-tertiary hover:text-primary"
                    title=t_string!(i18n, memory.view_in_graph)
                    on:click=move |ev| { ev.stop_propagation(); on_locate(); }
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                        <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" />
                        <line x1="17.4" y1="7.4" x2="13.4" y2="16.4" />
                        <line x1="7" y1="6" x2="17" y2="6" />
                    </svg>
                </button>
            </div>
        </div>
    }
}

// ─── Raw card ───────────────────────────────────────────────────────────────

#[component]
#[must_use]
pub fn RawCard(
    raw: RawMemory,
    checked: Signal<bool>,
    on_toggle: impl Fn() + Clone + Send + 'static,
    on_open: impl Fn() + Clone + Send + 'static,
    on_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let confirm = RwSignal::new(false);

    let agent_id = raw.agent_id.clone();
    let session_id = raw.session_id.clone();
    let created_at = raw
        .created_at
        .clone()
        .unwrap_or_else(|| "\u{2014}".to_string());
    let question = (!raw.user_input.is_empty()).then(|| raw.user_input.clone());
    let answer = (!raw.ai_output.is_empty()).then(|| raw.ai_output.clone());

    view! {
        <div
            class="group flex items-start gap-3 rounded-xl border border-border-subtle bg-surface-raised \
                   p-4 cursor-pointer hover:bg-surface-sunken transition-colors"
            on:click={
                let on_open = on_open.clone();
                move |_| on_open()
            }
        >
            <input
                type="checkbox"
                class="mt-0.5 cursor-pointer flex-shrink-0"
                aria-label=t_string!(i18n, memory.batch_selected)
                prop:checked=move || checked.get()
                on:click=move |ev| ev.stop_propagation()
                on:change=move |_| on_toggle()
            />

            <div class="min-w-0 flex-1 space-y-1">
                {question.map(|q| view! {
                    <div class="text-sm text-text-primary line-clamp-2 break-words">
                        <span class="mr-1.5 text-[10px] font-bold uppercase text-primary">
                            {move || t_string!(i18n, memory.raw_question).to_string()}
                        </span>
                        {q}
                    </div>
                })}
                {answer.map(|a| view! {
                    <div class="text-sm text-text-secondary line-clamp-2 break-words">
                        <span class="mr-1.5 text-[10px] font-bold uppercase text-success">
                            {move || t_string!(i18n, memory.raw_answer).to_string()}
                        </span>
                        {a}
                    </div>
                })}
                <div class="flex items-center gap-2 flex-wrap pt-1">
                    <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
                    {session_id.map(|s| view! {
                        <span class="text-[10px] text-text-tertiary font-mono">
                            {move || t_string!(i18n, memory.session).to_string()}" "{s}
                        </span>
                    })}
                    <span class="text-[10px] text-text-tertiary font-mono tabular-nums">{created_at}</span>
                </div>
            </div>

            <div class="flex-shrink-0" on:click=move |ev| ev.stop_propagation()>
                {move || if confirm.get() {
                    let on_delete = on_delete.clone();
                    view! {
                        <ConfirmButton
                            confirming=confirm
                            on_confirm=move || on_delete()
                            size_class="px-2.5 py-1 text-xs"
                            stop_propagation=true
                        />
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="p-1.5 rounded text-text-tertiary opacity-0 group-hover:opacity-100 \
                                   hover:text-danger transition-all"
                            title=t_string!(i18n, common.delete)
                            on:click=move |_| confirm.set(true)
                        >
                            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="3 6 5 6 21 6" />
                                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                            </svg>
                        </button>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `BadgeVariant` has no `Debug` impl (only `PartialEq`/`Eq`), so these use
    // `assert!(... == ...)` rather than `assert_eq!`, which requires `Debug`
    // to print a diff on failure.

    #[test]
    fn category_variant_maps_known_categories_to_their_badge_tone() {
        assert!(category_variant("preference") == BadgeVariant::Indigo);
        assert!(category_variant("personal") == BadgeVariant::Indigo);
        assert!(category_variant("learning") == BadgeVariant::Emerald);
        assert!(category_variant("lesson") == BadgeVariant::Emerald);
        assert!(category_variant("skill") == BadgeVariant::Emerald);
        assert!(category_variant("goal-lessons") == BadgeVariant::Emerald);
        assert!(category_variant("plan") == BadgeVariant::Amber);
        assert!(category_variant("project") == BadgeVariant::Amber);
        assert!(category_variant("feedback") == BadgeVariant::Red);
    }

    #[test]
    fn category_variant_unknown_category_falls_back_to_slate() {
        // An unrecognised category must not silently pick a curated tone —
        // Slate is the neutral fallback the galaxy stripe also uses.
        assert!(category_variant("mystery") == BadgeVariant::Slate);
        assert!(category_variant("") == BadgeVariant::Slate);
    }
}
