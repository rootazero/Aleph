//! Bulk-action bar for the memory console.
//!
//! Sits ABOVE the list, not fixed to the viewport bottom. A bottom-fixed bar
//! covers the pager and, on a short window, puts "select page" out of reach —
//! the reference implementation (memos `MemoriesView`) documents having had to
//! back that out.

use std::collections::HashSet;

use leptos::prelude::*;

use super::data::EXPORT_MAX;
use crate::components::ui::{Button, ButtonSize, ButtonVariant, ConfirmButton};
use crate::i18n::{t, t_string, use_i18n};

/// Whether a clipboard export of `selected_count` staged entries fits under
/// [`EXPORT_MAX`], and how many entries a copy would actually carry.
///
/// `data::notes_to_markdown` / `data::raws_to_markdown` deliberately do not
/// cap themselves — they render whatever slice they are handed. Silently
/// copying only the first 50 of an 80-item selection would hand the user a
/// partial export that looks complete, so above the cap nothing is copied at
/// all: the copy button disables and the on-screen message both read
/// [`ExportPlan::is_capped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportPlan {
    /// Selection fits; carries how many entries a copy would include.
    Runs(usize),
    /// Selection exceeds `EXPORT_MAX`; a copy is blocked until the user
    /// narrows the selection. Carries the two numbers the cap message needs.
    Capped { selected: usize, max: usize },
}

impl ExportPlan {
    fn is_capped(self) -> bool {
        matches!(self, Self::Capped { .. })
    }
}

fn plan_export(selected_count: usize) -> ExportPlan {
    if selected_count > EXPORT_MAX {
        ExportPlan::Capped {
            selected: selected_count,
            max: EXPORT_MAX,
        }
    } else {
        ExportPlan::Runs(selected_count)
    }
}

#[component]
#[must_use]
pub fn BatchBar(
    selected: RwSignal<HashSet<String>>,
    /// Ids currently rendered, for the select-page toggle.
    page_ids: Signal<Vec<String>>,
    /// `Some((done, total))` while a clipboard export is in flight.
    exporting: RwSignal<Option<(usize, usize)>>,
    on_copy_md: impl Fn() + Clone + Send + 'static,
    on_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let confirm = RwSignal::new(false);

    let count = Signal::derive(move || selected.get().len());
    let page_all_selected = Signal::derive(move || {
        let sel = selected.get();
        let ids = page_ids.get();
        !ids.is_empty() && ids.iter().all(|id| sel.contains(id))
    });
    let over_cap = Signal::derive(move || plan_export(count.get()).is_capped());
    let busy = Signal::derive(move || exporting.get().is_some());

    view! {
        {move || {
            // `Show` wraps children in `Arc<dyn Fn() -> View + Send + Sync>`,
            // which would force `on_copy_md`/`on_delete` to be `Sync` too --
            // widening the signature beyond what the brief specifies. A bare
            // reactive closure only needs `Send`, matching the analogous
            // early-return pattern the pre-refactor batch bar in `mod.rs`
            // already used for the same "nothing selected" case.
            if count.get() == 0 {
                return view! { <div></div> }.into_any();
            }
            view! {
            <div
                class="flex items-center justify-between gap-3 flex-wrap rounded-lg border border-border \
                       bg-surface-sunken px-4 py-2"
                role="region"
                aria-label="bulk actions"
            >
                <span class="text-sm text-text-secondary tabular-nums">
                    {move || count.get().to_string()}" "{t!(i18n, memory.batch_selected)}
                </span>

                <div class="flex items-center gap-2 flex-wrap">
                    <button
                        class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary \
                               hover:text-text-primary transition-colors"
                        on:click=move |_| {
                            let ids = page_ids.get_untracked();
                            let all_in = page_all_selected.get_untracked();
                            selected.update(|s| {
                                for id in ids {
                                    if all_in { s.remove(&id); } else { s.insert(id); }
                                }
                            });
                        }
                    >
                        {move || if page_all_selected.get() {
                            t_string!(i18n, memory.batch_deselect_page).to_string()
                        } else {
                            t_string!(i18n, memory.batch_select_page).to_string()
                        }}
                    </button>

                    <div class="flex items-center gap-1.5">
                        <button
                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary \
                                   hover:text-text-primary disabled:opacity-40 \
                                   disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || over_cap.get() || busy.get()
                            on:click={
                                let on_copy_md = on_copy_md.clone();
                                move |_| on_copy_md()
                            }
                        >
                            {move || match exporting.get() {
                                Some((done, total)) => format!(
                                    "{} {done}/{total}",
                                    t_string!(i18n, memory.batch_exporting)
                                ),
                                None => t_string!(i18n, memory.batch_copy_md).to_string(),
                            }}
                        </button>
                        // A cap the user cannot see is a cap that looks like a bug.
                        <Show when=move || over_cap.get()>
                            <span class="text-[10px] text-warning">
                                {move || t_string!(i18n, memory.batch_copy_cap).to_string()}
                            </span>
                        </Show>
                    </div>

                    {
                        // The enclosing reactive closure (this whole bar, one
                        // nesting level up) must stay `Fn`, callable again on
                        // the next `count` tick. Cloning here -- once per that
                        // outer call -- before this reactive closure captures
                        // it is what keeps the capture a fresh, disposable
                        // copy each time rather than the outer closure's only
                        // copy of `on_delete`.
                        let on_delete = on_delete.clone();
                        move || if confirm.get() {
                            let on_delete = on_delete.clone();
                            view! {
                                <ConfirmButton
                                    confirming=confirm
                                    on_confirm=move || on_delete()
                                    size_class="px-3 py-1 text-xs"
                                />
                            }.into_any()
                        } else {
                            view! {
                                <Button
                                    variant=ButtonVariant::Destructive
                                    size=ButtonSize::Sm
                                    class="px-3 py-1 text-xs".to_string()
                                    on:click=move |_| confirm.set(true)
                                >
                                    {t!(i18n, memory.batch_delete)}
                                </Button>
                            }.into_any()
                        }
                    }

                    <button
                        class="text-xs text-text-tertiary hover:text-text-secondary"
                        on:click=move |_| { selected.set(HashSet::new()); confirm.set(false); }
                    >
                        {t!(i18n, memory.batch_clear)}
                    </button>
                </div>
            </div>
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_cap_exports_the_full_selection() {
        let plan = plan_export(EXPORT_MAX - 1);
        assert_eq!(plan, ExportPlan::Runs(EXPORT_MAX - 1));
        assert!(!plan.is_capped());
    }

    #[test]
    fn exactly_at_cap_still_runs_in_full() {
        // The boundary itself must not tip into "capped" — EXPORT_MAX entries
        // is a full, uncapped export.
        let plan = plan_export(EXPORT_MAX);
        assert_eq!(plan, ExportPlan::Runs(EXPORT_MAX));
        assert!(!plan.is_capped());
    }

    #[test]
    fn above_cap_is_reported_as_blocked_not_silently_shrunk() {
        // The user-visible truth is that the export is capped and blocked.
        // Asserting only "the plan carries 50" would also pass a design that
        // silently copied the first 50 of 80 selected entries -- exactly the
        // silent-truncation bug this component exists to prevent.
        let plan = plan_export(EXPORT_MAX + 30);
        assert!(plan.is_capped());
        assert_eq!(
            plan,
            ExportPlan::Capped {
                selected: EXPORT_MAX + 30,
                max: EXPORT_MAX,
            }
        );
    }
}
