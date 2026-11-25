//! Chat-window model picker — compact pill + popover for picking the
//! per-turn provider/model, with an explicit session action via `/model`.
//!
//! Each opening fetches the Gateway's usable choices. This component only
//! filters for display; the Gateway authorizes and binds the selection.
//!
//! ## Keyboard walk
//!
//! The popover is two levels deep on screen (a provider heading, then its
//! model ids) and one level deep to the keyboard: `walk_targets` flattens it
//! into the rows ↑/↓ can actually land on, with `Default` at index 0 because
//! that is where it is drawn. The highlight moves only through
//! `step_highlight`, shared with the settings disclosure and the ⌘K palette.
//!
//! The flattening is why the provider loop is a plain `map` and not `<For>`:
//! a row's flat index depends on how many models every *preceding* provider
//! offers, so narrowing an earlier provider's roster shifts every later index.
//! Keyed by provider id alone, `<For>` would keep those children and their
//! captured — now stale — offsets; keying by `(id, offset)` would rebuild the
//! list on exactly the keystrokes that change it, which is what `map` already
//! does, for less.

use leptos::html::{Div, Input};
use leptos::prelude::*;
use leptos::task::spawn_local;

use aleph_protocol::providers::{
    search::filter_chat_model_choices as filter_choices, ChatModelChoice,
};

use crate::api::chat::ChatApi;
use crate::api::providers::ModelOverride;
use crate::components::picker_nav::{
    publish_more_below, row_dom_id, scroll_row_into_view, step_highlight,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::sessions::SessionMap;
use crate::views::chat::state::ChatState;
use shared_ui_logic::state::session_dials_for_send;

/// Namespace for this popover's row ids — see [`row_dom_id`].
const LIST: &str = "model";

/// A row the keyboard walk can land on, in the order the popover draws them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickTarget {
    /// Clear the per-turn override and fall back to the agent's chain. Always
    /// index 0 — the row is rendered above the list and outside the filter, so
    /// it is reachable even while the catalogue is loading or empty.
    Default,
    Model {
        provider: String,
        model: String,
    },
}

/// Linear keyboard order matches the rendered rows, with Default first.
fn walk_targets(entries: &[ChatModelChoice]) -> Vec<PickTarget> {
    std::iter::once(PickTarget::Default)
        .chain(entries.iter().map(|entry| PickTarget::Model {
            provider: entry.provider.clone(),
            model: entry.model.clone(),
        }))
        .collect()
}

fn request_is_current(open: bool, issued: u64, current: u64, same_conversation: bool) -> bool {
    open && issued == current && same_conversation
}

/// Format the existing slash syntax without interpreting the model id.
///
/// The slash dispatcher forwards `provider/model` verbatim to `select_model`.
/// Nested aggregator ids must survive; provider resolution belongs to core.
#[must_use]
fn model_command(provider: &str, model: &str) -> String {
    format!("/model {provider}/{model}")
}

/// Pill + dropdown for selecting the per-turn chat model.
#[component]
#[must_use]
pub fn ModelPicker() -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let session_sending = RwSignal::new(false);
    let session_notice: RwSignal<Option<String>> = RwSignal::new(None);
    let open = RwSignal::new(false);
    let entries: RwSignal<Vec<ChatModelChoice>> = RwSignal::new(Vec::new());
    let generation = RwSignal::new(0u64);
    let opened_for = RwSignal::new(sessions.active_conv());
    let same_conversation = move || opened_for.get_untracked() == sessions.active_conv();
    let can_pick = move || open.get_untracked() && same_conversation();
    let loading = RwSignal::new(false);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    // Live filter term for the popover's search box. Order-preserving substring
    // match (`aleph_protocol::providers::search`); reset when the popover closes.
    let search = RwSignal::new(String::new());

    let load_choices = move || {
        let issued = generation.get_untracked() + 1;
        generation.set(issued);
        opened_for.set(sessions.active_conv());
        entries.set(Vec::new());
        loading.set(true);
        load_error.set(None);
        spawn_local(async move {
            let result = ChatApi::models(&dashboard).await;
            if !request_is_current(
                open.get_untracked(),
                issued,
                generation.get_untracked(),
                same_conversation(),
            ) {
                return;
            }
            match result {
                Ok(items) => entries.set(items),
                Err(error) => load_error.set(Some(error)),
            }
            loading.set(false);
        });
    };

    Effect::new(move |_| {
        if open.get() && opened_for.get() != sessions.active.get() {
            open.set(false);
        }
    });

    // What the pill names, in the same precedence core resolves
    // (`effective_model_directive`): this turn's pick, else the conversation's
    // `select_model` pin, else nothing chosen at all.
    //
    // The middle arm is the one that was missing. A pin is per-session state
    // the model itself sets (R8) and the run loop honours from the next
    // provider request boundary onward,
    // so a pill that only knew about its own per-turn override answered
    // "Default" for a conversation that was pinned — naming, of all the models
    // available, the one that was not going to serve at its next request.
    let trigger_label = move || -> String {
        if let Some(mo) = chat.selected_model.get() {
            return match mo.provider() {
                Some(p) => format!("{}/{}", p, mo.model()),
                None => mo.model().to_string(),
            };
        }
        chat.session_model_pin
            .get()
            .unwrap_or_else(|| t_string!(i18n, model_picker.default_label).to_string())
    };

    let select_entry = move |provider: String, model: String| {
        if !can_pick() {
            return;
        }
        chat.selected_model
            .set(Some(ModelOverride::Qualified { provider, model }));
        open.set(false);
    };

    // A distinct action: never alter `selected_model` or optimistically write
    // `session_model_pin`. The server's slash tool is the only pin writer.
    let select_session = move |provider: String, model: String| {
        if !can_pick() || session_sending.get_untracked() || chat.team_id.get_untracked().is_some()
        {
            return;
        }
        let Some(send_conv) = sessions.active_conv() else {
            session_notice.set(Some(
                t_string!(i18n, model_picker.no_active_conversation).to_string(),
            ));
            return;
        };
        let command = model_command(&provider, &model);
        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let room_project_id = chat.room_project_id.get_untracked();
        let project_root = if room_project_id.is_some() {
            None
        } else {
            chat.active_project_root.get_untracked()
        };
        let dials = session_dials_for_send(session_key.is_some(), &chat.session_knobs());
        session_sending.set(true);
        session_notice.set(None);
        spawn_local(async move {
            // No attachments, voice flag or per-turn override on a control
            // command. The existing per-turn selection remains untouched.
            let result = ChatApi::send(
                &dashboard,
                &command,
                session_key.as_deref(),
                Vec::new(),
                agent_id.as_deref(),
                project_root.as_deref(),
                room_project_id.as_deref(),
                None,
                &dials,
                false,
            )
            .await;
            match result {
                Ok(resp) => {
                    sessions.bind_run(&resp.run_id, send_conv, Some(&resp.session_key));
                    // The user may have switched tabs while the RPC was in
                    // flight. Backfill the captured conversation, not the new
                    // foreground singleton (and never recreate a closed tab).
                    if let Some(target) = sessions.chat_for(send_conv, chat) {
                        target.session_key.set(Some(resp.session_key));
                    }
                    // ChatSendResponse has no selection generation or applied
                    // selection. An accepted send is not proof of a changed
                    // pin: leave its projection to the existing server feed.
                    if sessions.active_conv() == Some(send_conv) {
                        session_notice.set(Some(
                            t_string!(i18n, model_picker.sent_notice, command = command.clone())
                                .to_string(),
                        ));
                    }
                }
                Err(error) if sessions.active_conv() == Some(send_conv) => {
                    session_notice.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &error, |e| {
                            t_string!(
                                i18n,
                                model_picker.send_failed,
                                command = command.clone(),
                                error = e.to_string()
                            )
                            .to_string()
                        }),
                    ));
                }
                Err(_) => {}
            }
            session_sending.set(false);
        });
    };

    let clear_selection = move || {
        if can_pick() {
            chat.selected_model.set(None);
            open.set(false);
        }
    };

    // Clear the filter every time the popover closes so the next open starts
    // from the full catalog (mirrors `command_palette.rs`'s reset-on-close).
    Effect::new(move |_| {
        if !open.get() {
            search.set(String::new());
        }
    });

    let highlight = RwSignal::new(0usize);
    let popover_ref = NodeRef::<Div>::new();
    let search_ref = NodeRef::<Input>::new();
    let more_below = RwSignal::new(false);

    // The flat walk, shared by the keyboard handler and by the per-row
    // `is_highlighted` predicates below.
    let filtered = Memo::new(move |_| filter_choices(&entries.get(), &search.get()));
    let targets = Memo::new(move |_| walk_targets(&filtered.get()));

    // A new list means a new first row (see `preset_picker` for the full
    // argument). `Default` is index 0, so a narrowed search lands the highlight
    // on "clear the override" rather than on whatever survived the filter —
    // deliberate: the alternative is arming Enter on a model the operator has
    // not looked at.
    Effect::new(move |_| {
        targets.track();
        highlight.set(0);
    });

    let remeasure = move || publish_more_below(popover_ref, more_below);

    // Focus so the popover can receive keys at all, and re-measure the well.
    // Both re-run when the catalogue lands, because the search box only exists
    // once it has: an effect that only tracked `open` would fire while the
    // popover still says "loading" and find nothing to focus.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let _ = loading.get();
        let _ = entries.get();
        targets.track();
        request_animation_frame(move || {
            if let Some(el) = search_ref.get_untracked() {
                let _ = el.focus();
            } else if let Some(el) = popover_ref.get_untracked() {
                // No search box yet (loading / empty / error). The container
                // is `tabindex=-1` so Escape and the Default row stay reachable.
                let _ = el.focus();
            }
            remeasure();
        });
    });

    let commit = move |target: &PickTarget| match target {
        PickTarget::Default => clear_selection(),
        PickTarget::Model { provider, model } => select_entry(provider.clone(), model.clone()),
    };

    // Scoped to the popover subtree, not the window: this component unmounts
    // with the composer, and `window_event_listener` registers no cleanup.
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if !open.get_untracked() {
            return;
        }
        let current = targets.get_untracked();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                let next = step_highlight(current.len(), highlight.get_untracked(), 1);
                highlight.set(next);
                scroll_row_into_view(LIST, next);
            }
            "ArrowUp" => {
                ev.prevent_default();
                let next = step_highlight(current.len(), highlight.get_untracked(), -1);
                highlight.set(next);
                scroll_row_into_view(LIST, next);
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(target) =
                    current.get(step_highlight(current.len(), highlight.get_untracked(), 0))
                {
                    commit(target);
                }
            }
            "Escape" => {
                ev.prevent_default();
                open.set(false);
            }
            _ => {}
        }
    };

    view! {
        <div class="relative">
            <button
                on:click=move |_| {
                    let next = !open.get_untracked();
                    if next {
                        load_choices();
                    }
                    open.set(next);
                }
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, model_picker.pick_model_title).to_string()
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 2L2 7l10 5 10-5-10-5z" />
                    <path d="M2 17l10 5 10-5" />
                    <path d="M2 12l10 5 10-5" />
                </svg>
                <span class="max-w-[200px] truncate">{trigger_label}</span>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            <Show when=move || session_notice.get().is_some()>
                <p role="status" class="text-[10px] text-text-secondary max-w-80 break-words">
                    {move || session_notice.get().unwrap_or_default()}
                </p>
            </Show>

            <Show when=move || open.get()>
                <div node_ref=popover_ref
                    tabindex="-1"
                    on:keydown=on_key
                    on:scroll=move |_| remeasure()
                    class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1 outline-none"
                    class:aleph-scroll-more=move || more_below.get()
                    on:mouseleave=move |_| open.set(false)>
                    // Filter box — only meaningful once a non-empty catalog has
                    // loaded. Order-preserving substring filter, deliberately
                    // not fuzzy-ranked so the daemon's curated provider/model
                    // order survives. The matcher is shared with the TUI's
                    // picker (`aleph_protocol::providers::search`): two
                    // independently written filters do not merely look
                    // different, they disagree about which row a bare Enter
                    // selects.
                    {move || (!loading.get()
                        && load_error.get().is_none()
                        && !entries.get().is_empty())
                        .then(|| view! {
                            <input
                                node_ref=search_ref
                                type="text"
                                placeholder=move || {
                                    t_string!(i18n, model_picker.filter_placeholder).to_string()
                                }
                                class="w-full px-2.5 py-1.5 mb-1 rounded-md text-xs bg-surface-sunken \
                                       text-text-primary placeholder:text-text-tertiary outline-none \
                                       border border-border focus:border-primary/40"
                                on:input=move |ev| search.set(event_target_value(&ev))
                                prop:value=move || search.get()
                            />
                        })}

                    // Default option — index 0 of the keyboard walk.
                    <button
                        id=row_dom_id(LIST, 0)
                        on:click=move |_| clear_selection()
                        class=move || {
                            let base = "w-full text-left px-2.5 py-2 rounded-md text-xs \
                                        transition-colors flex items-center justify-between gap-2";
                            if chat.selected_model.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
                            } else if highlight.get() == 0 {
                                format!("{base} bg-surface-sunken text-text-primary border border-border")
                            } else {
                                format!("{base} hover:bg-surface-sunken text-text-secondary border border-transparent")
                            }
                        }
                    >
                        <span class="font-medium">{t!(i18n, model_picker.default_label)}</span>
                        <span class="text-text-tertiary text-[10px]">{t!(i18n, model_picker.fallback_chain)}</span>
                    </button>

                    // Loading / error / list
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.loading_catalog)}
                                </div>
                            }.into_any()
                        } else if let Some(err) = load_error.get() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-danger/80 text-center">
                                    {t!(i18n, model_picker.load_failed, error = err)}
                                </div>
                            }.into_any()
                        } else if entries.get().is_empty() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.no_providers)}
                                </div>
                            }.into_any()
                        } else if filtered.with(Vec::is_empty) {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.no_match)}
                                </div>
                            }.into_any()
                        } else {
                            // Plain `map` with a running flat index, not
                            // `<For>` — see the module doc. `flat` starts at 1
                            // because the Default row above took index 0.
                            let mut flat = 1usize;
                            let mut groups: Vec<AnyView> = Vec::new();
                            let choices = filtered.get();
                            for choices in choices.chunk_by(|left, right| left.provider == right.provider) {
                                let display = choices[0].provider_label.clone();
                                let mut rows: Vec<AnyView> = Vec::new();
                                for choice in choices {
                                    let idx = flat;
                                    flat += 1;
                                    let pid = choice.provider.clone();
                                    let mid = choice.model.clone();
                                    let session_pid = pid.clone();
                                    let session_mid = mid.clone();
                                    let session_title = t_string!(
                                        i18n,
                                        model_picker.session_title,
                                        command = model_command(&session_pid, &session_mid)
                                    )
                                    .to_string();
                                    let pid_active = pid.clone();
                                    let mid_active = mid.clone();
                                    let is_active = move || {
                                        matches!(
                                            chat.selected_model.get(),
                                            Some(ModelOverride::Qualified { provider, model })
                                                if provider == pid_active && model == mid_active
                                        )
                                    };
                                    let display_text = choice.model_label.clone();
                                    rows.push(view! {
                                        <div class="flex items-center gap-1">
                                        <button
                                            type="button"
                                            id=row_dom_id(LIST, idx)
                                            on:click=move |_| select_entry(pid.clone(), mid.clone())
                                            class=move || {
                                                let base = "w-full text-left px-2.5 py-1.5 rounded-md \
                                                            text-xs font-mono transition-colors \
                                                            border";
                                                if is_active() {
                                                    format!("{base} bg-primary/10 text-primary border-primary/30")
                                                } else if highlight.get() == idx {
                                                    format!("{base} bg-surface-sunken text-text-primary border-border")
                                                } else {
                                                    format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                                }
                                            }
                                        >
                                            {display_text}
                                        </button>
                                        <button
                                            type="button"
                                            title=session_title
                                            aria-label=t_string!(
                                                i18n,
                                                model_picker.session_aria_label,
                                                command = model_command(&session_pid, &session_mid)
                                            )
                                            .to_string()
                                            disabled=move || session_sending.get()
                                                || chat.team_id.get().is_some()
                                                || sessions.active.get().is_none()
                                            // Let native Enter/Space activate THIS
                                            // button, not the popover's per-turn
                                            // keyboard commit handler.
                                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                if matches!(ev.key().as_str(), "Enter" | " ") {
                                                    ev.stop_propagation();
                                                }
                                            }
                                            on:click=move |_| select_session(session_pid.clone(), session_mid.clone())
                                            class="shrink-0 px-1.5 py-1 rounded-md text-[10px] border border-border
                                                   text-text-secondary hover:bg-surface-sunken disabled:opacity-40"
                                        >
                                            {t!(i18n, model_picker.session_action)}
                                        </button>
                                        </div>
                                    }.into_any());
                                }
                                groups.push(view! {
                                    <div class="pt-1.5">
                                        <div class="flex items-center gap-1.5 px-2.5 pb-1">
                                            <span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                                                {display}
                                            </span>
                                        </div>
                                        {rows}
                                    </div>
                                }.into_any());
                            }
                            view! { <div>{groups}</div> }.into_any()
                        }
                    }}
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue(groups: &[(&str, &[&str])]) -> Vec<ChatModelChoice> {
        groups
            .iter()
            .flat_map(|(provider, models)| {
                models.iter().map(|model| ChatModelChoice {
                    provider: (*provider).into(),
                    model: (*model).into(),
                    provider_label: (*provider).into(),
                    model_label: (*model).into(),
                })
            })
            .collect()
    }

    #[test]
    fn picker_reopen_ignores_late_result() {
        assert!(request_is_current(true, 2, 2, true));
        assert!(!request_is_current(true, 1, 2, true));
        assert!(!request_is_current(false, 2, 2, true));
        assert!(!request_is_current(true, 2, 2, false));
    }

    #[test]
    fn model_command_payload_is_the_existing_qualified_slash_form() {
        assert_eq!(model_command("openai", "gpt-4o"), "/model openai/gpt-4o");
        assert_eq!(
            model_command("openrouter", "anthropic/claude-sonnet-4"),
            "/model openrouter/anthropic/claude-sonnet-4"
        );
        assert_eq!(
            model_command("custom-provider", "vendor/model:free"),
            "/model custom-provider/vendor/model:free"
        );
    }

    fn model(provider: &str, model: &str) -> PickTarget {
        PickTarget::Model {
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn an_empty_catalogue_still_offers_default() {
        // Loading, empty and errored popovers all draw the Default row, so the
        // walk must contain it even when there is nothing to filter.
        assert_eq!(walk_targets(&[]), vec![PickTarget::Default]);
    }

    #[test]
    fn default_is_index_zero_and_models_follow_in_render_order() {
        let cat = catalogue(&[("alpha", &["a1", "a2"]), ("beta", &["b1"])]);
        assert_eq!(
            walk_targets(&cat),
            vec![
                PickTarget::Default,
                model("alpha", "a1"),
                model("alpha", "a2"),
                model("beta", "b1"),
            ]
        );
    }

    /// The property that rules out `<For key=provider_id>`: narrowing an
    /// earlier provider's roster moves every later provider's flat index, so a
    /// keyed child holding a captured index would light the wrong row.
    #[test]
    fn narrowing_an_earlier_roster_shifts_later_indices() {
        let cat = catalogue(&[("alpha", &["a1", "a2"]), ("beta", &["b1"])]);
        let wide = walk_targets(&cat);
        let narrow = walk_targets(&filter_choices(&cat, "a2"));

        assert_eq!(
            wide.iter().position(|t| *t == model("alpha", "a2")),
            Some(2)
        );
        assert_eq!(
            narrow.iter().position(|t| *t == model("alpha", "a2")),
            Some(1),
            "the surviving model moved up once its sibling was filtered out"
        );
    }

    /// The walk must be the buttons on screen, not "every model of every
    /// matching provider": the renderer and keyboard walk use the same rows.
    #[test]
    fn a_model_query_walks_only_the_matching_ids() {
        let cat = catalogue(&[("alpha", &["gpt-4o", "gpt-4o-mini"])]);
        assert_eq!(
            walk_targets(&filter_choices(&cat, "mini")),
            vec![PickTarget::Default, model("alpha", "gpt-4o-mini")]
        );
    }

    #[test]
    fn an_empty_query_offers_every_row() {
        // Same contract the settings picker's `offer` closure owes: browsing
        // must not require knowing a vendor's name first.
        let cat = catalogue(&[("alpha", &["a1"]), ("beta", &["b1", "b2"])]);
        assert_eq!(walk_targets(&filter_choices(&cat, "")).len(), 4);
    }
}
