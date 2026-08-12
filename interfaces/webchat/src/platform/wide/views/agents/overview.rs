// Overview Tab — identity, model config, and inference parameters editor

use crate::api::agents::AgentsApi;
use crate::api::providers::{CatalogEntry, CatalogView, ProvidersApi};
use crate::api::users::{UserInfo, UsersApi};
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

/// One row of the admission-list picker.
pub struct AccessRow {
    pub user_id: String,
    pub label: String,
    /// The id is on the agent's `allowed_users` but `users.list` does not know
    /// it — a principal deleted after the grant was written.
    pub unknown_principal: bool,
}

/// The rows to render: every known principal, plus every id already granted
/// that is no longer one.
///
/// The union is the point. Rendering only `users.list` would make a stale grant
/// invisible, and then the next Save — which writes the checked set verbatim —
/// would silently revoke it. Silent is the problem, not the revocation: an
/// operator who wants that grant gone should see it and uncheck it. Pure so it
/// can be pinned; the same reason `roster_empty_state` was lifted out of markup.
#[must_use]
pub fn access_rows(known: &[UserInfo], granted: &[String], unknown_label: &str) -> Vec<AccessRow> {
    let mut rows: Vec<AccessRow> = known
        .iter()
        .map(|u| AccessRow {
            user_id: u.user_id.clone(),
            label: format!("{} ({})", u.display_name, u.user_id),
            unknown_principal: false,
        })
        .collect();
    for id in granted {
        if !known.iter().any(|u| &u.user_id == id) {
            rows.push(AccessRow {
                user_id: id.clone(),
                label: format!("{id} — {unknown_label}"),
                unknown_principal: true,
            });
        }
    }
    rows
}

#[component]
#[must_use]
pub fn OverviewTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Editable fields
    let emoji = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    // "" = inherit system default; otherwise "provider|model" (catalog selected item)
    let selected_model = RwSignal::new(String::new());
    let model_touched = RwSignal::new(false);
    let catalog: RwSignal<Vec<CatalogEntry>> = RwSignal::new(Vec::new());
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);

    // Admission list (`allowed_users`) — who may start a run AS this agent.
    let known_users: RwSignal<Vec<UserInfo>> = RwSignal::new(Vec::new());
    let users_error = RwSignal::new(Option::<String>::None);
    let allowed: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    // Same guard as `model_touched`, for a sharper reason: the checked set is
    // written verbatim, so an untouched (or failed-to-load) picker that got
    // sent would read as "revoke everyone's grant" — a permission change
    // nobody asked for, from a page that merely failed to load.
    let allowed_touched = RwSignal::new(false);

    // Nothing may be saved before the current values are known. Without this
    // the Save button writes the empty initial `name` / `description` over
    // whatever is on disk whenever `agents.get` failed — the load-failure
    // wipe that `model_touched` already protected `model` from, left open on
    // the two fields that have no such flag.
    let detail_loaded = RwSignal::new(false);
    let load_error = RwSignal::new(Option::<String>::None);

    // Fetch catalog once
    {
        let dash = state;
        spawn_local(async move {
            if let Ok(items) = ProvidersApi::catalog(&dash, CatalogView::Configured).await {
                catalog.set(items);
            }
        });
    }

    // Load agent detail
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let id = id_for_load.clone();
        spawn_local(async move {
            // The principals the picker offers. Refused / unreachable is NOT
            // "there are no users": an empty list would invite the operator to
            // read an unknown roster as an empty one and save a revocation.
            match UsersApi::list(&dash).await {
                Ok(users) => {
                    known_users.set(users);
                    users_error.set(None);
                }
                Err(e) => users_error.set(Some(admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |detail| {
                        format!(
                            "{}: {detail}",
                            t_string!(i18n, agents.overview.access_users_failed)
                        )
                    },
                ))),
            }

            match AgentsApi::get(&dash, &id).await {
                Err(e) => {
                    detail_loaded.set(false);
                    load_error.set(Some(admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |detail| {
                            format!("{}: {detail}", t_string!(i18n, agents.overview.load_failed))
                        },
                    )));
                }
                Ok(detail) => {
                    load_error.set(None);
                    let def = &detail.definition;
                    allowed.set(
                        def.get("allowed_users")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    );
                    name.set(
                        def.get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );

                    if let Some(identity) = def.get("identity") {
                        emoji.set(
                            identity
                                .get("emoji")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        );
                        description.set(
                            identity
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        );
                    }

                    // Read stored model: Qualified object -> "provider|model"; Legacy string / absent -> leave empty = inherit
                    if let Some(mv) = def.get("model") {
                        if let Some(obj) = mv.as_object() {
                            let p = obj.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                            let m = obj.get("model").and_then(|v| v.as_str()).unwrap_or("");
                            if !p.is_empty() && !m.is_empty() {
                                selected_model.set(format!("{p}\u{1f}{m}"));
                            }
                        }
                        // Legacy bare string: no provider context → leave empty (=inherit); user can re-pick.
                    }
                    // Last, so a panic-free early return above can never leave
                    // the Save button enabled over half-populated fields.
                    detail_loaded.set(true);
                }
            }
        });
    });

    // Save handler
    let id_for_save = agent_id.clone();
    let handle_save = move |_: web_sys::MouseEvent| {
        is_saving.set(true);
        save_message.set(None);
        let id = id_for_save.clone();
        let dash = state;

        let sel = selected_model.get();
        let model_patch = if sel.is_empty() {
            serde_json::Value::Null // inherit system default
        } else if let Some((p, m)) = sel.split_once('\u{1f}') {
            json!({ "provider": p, "model": m })
        } else {
            serde_json::Value::Null
        };

        let mut patch = json!({
            "name": name.get(),
            "identity": {
                "emoji": emoji.get(),
                "description": description.get(),
            },
        });

        // Only write the model key when the user actually changes the dropdown:
        // untouched -> key absent -> backend AgentPatch.model = None -> preserve original value (legacy/qualified).
        if model_touched.get() {
            patch["model"] = model_patch;
        }

        // Same rule for the admission list, and the reason is stronger: the
        // checked set is written verbatim, so sending an untouched picker
        // would turn every Save of an unrelated field into a permission
        // change. Absent key -> `AgentPatch.allowed_users = None` -> the TOML
        // key and the live registry are both left exactly as they were.
        let admission_touched = allowed_touched.get();
        if admission_touched {
            patch["allowed_users"] = json!(allowed.get());
        }

        spawn_local(async move {
            match AgentsApi::update(&dash, &id, patch).await {
                Ok(outcome) => {
                    // Three different facts, and collapsing them is what the
                    // server-side `takes_effect` split exists to prevent: the
                    // write succeeded; the fields with no runtime half wait for
                    // a restart; the admission list — if it was part of this
                    // save — either reached the live registry or did not.
                    let mut msg = format!(
                        "{} {}",
                        t_string!(i18n, agents.overview.saved),
                        t_string!(i18n, agents.overview.saved_restart_note)
                    );
                    if admission_touched {
                        msg.push(' ');
                        msg.push_str(&if outcome.allowed_users_applied_live {
                            t_string!(i18n, agents.overview.access_applied_live).to_string()
                        } else {
                            t_string!(i18n, agents.overview.access_not_applied_live).to_string()
                        });
                    }
                    save_message.set(Some((
                        outcome.allowed_users_applied_live || !admission_touched,
                        msg,
                    )));
                }
                Err(e) => save_message.set(Some((
                    false,
                    admin_refusal::settings_write_error(i18n, &e, |detail| {
                        format!("{}: {detail}", t_string!(i18n, common.save))
                    }),
                ))),
            }
            is_saving.set(false);
        });
    };

    view! {
        <div class="space-y-6">
            // Identity section
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.overview.title)}</h2>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.agent_id)}</label>
                        <div class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-tertiary font-mono text-sm select-all">
                            {agent_id}
                        </div>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.emoji)}</label>
                        <input
                            type="text"
                            prop:value=move || emoji.get()
                            on:input=move |ev| emoji.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary text-lg"
                            placeholder=move || t_string!(i18n, agents.overview.emoji_placeholder).to_string()
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.display_name)}</label>
                        <input
                            type="text"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder=move || t_string!(i18n, agents.overview.name_placeholder).to_string()
                        />
                    </div>
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.description)}</label>
                        <textarea
                            prop:value=move || description.get()
                            on:input=move |ev| description.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary resize-none"
                            rows="2"
                            placeholder=move || t_string!(i18n, agents.overview.description_placeholder).to_string()
                        />
                    </div>
                </div>
            </div>

            // Model Configuration
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.overview.model_config)}</h2>
                <div class="space-y-2">
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.primary_model)}</label>
                    <select
                        prop:value=move || selected_model.get()
                        on:change=move |ev| { selected_model.set(event_target_value(&ev)); model_touched.set(true); }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono text-sm"
                    >
                        <option value="">"继承系统默认 (inherit system default)"</option>
                        {move || {
                            catalog.get().into_iter().flat_map(|entry: CatalogEntry| {
                                let provider_id = entry.id.clone();
                                // `roster` verbatim (R4). The local rebuild
                                // this replaced was `models` else
                                // `[default_model]`, which missed the curated
                                // rungs entirely and had no way to apply the
                                // "operator moved base_url ⇒ no curated rungs"
                                // guard the backend merge does.
                                let models = entry.roster;
                                let dn = entry.display_name;
                                models.into_iter().map(move |m| {
                                    let m = m.id;
                                    let val = format!("{provider_id}\u{1f}{m}");
                                    let label = format!("{dn} / {m}");
                                    view! { <option value=val>{label}</option> }
                                }).collect::<Vec<_>>()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                    {move || {
                        let sel = selected_model.get();
                        let in_catalog = sel.is_empty() || catalog.get().iter().any(|e| {
                            e.roster.iter().any(|m| format!("{}\u{1f}{}", e.id, m.id) == sel)
                        });
                        (!in_catalog).then(|| view! {
                            <p class="mt-1 text-xs text-danger/80">
                                "\u{26a0} 当前选中的 model 已失效(provider 被删/禁用),保存后将回退系统默认"
                            </p>
                        })
                    }}
                </div>
            </div>

            // Access — who may run AS this agent (`allowed_users`)
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, agents.overview.access_title)}</h2>
                <p class="text-xs text-text-tertiary mb-4">{t!(i18n, agents.overview.access_live_note)}</p>

                // A refused / failed roster read is never rendered as "there
                // are no users" — the empty state below is only reachable when
                // the read SUCCEEDED and returned nothing.
                {move || users_error.get().map(|msg| view! {
                    <div class="p-3 mb-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{msg}</div>
                })}

                {move || {
                    let known = known_users.get();
                    let granted = allowed.get();
                    let unknown_label = t_string!(i18n, agents.overview.access_unknown_user).to_string();
                    let rows = access_rows(&known, &granted, &unknown_label);
                    if rows.is_empty() {
                        return (users_error.get().is_none()).then(|| view! {
                            <p class="text-sm text-text-tertiary">{t!(i18n, agents.overview.access_no_users)}</p>
                        }).into_any();
                    }
                    rows.into_iter().map(|row| {
                        let uid = row.user_id.clone();
                        let checked = granted.contains(&row.user_id);
                        let label_class = if row.unknown_principal {
                            "text-sm text-warning"
                        } else {
                            "text-sm text-text-primary"
                        };
                        view! {
                            <label class="flex items-center gap-2 py-1 cursor-pointer">
                                <input
                                    type="checkbox"
                                    prop:checked=checked
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        allowed_touched.set(true);
                                        allowed.update(|list| {
                                            if on {
                                                if !list.contains(&uid) { list.push(uid.clone()); }
                                            } else {
                                                list.retain(|u| u != &uid);
                                            }
                                        });
                                    }
                                    class="accent-primary"
                                />
                                <span class=label_class>{row.label}</span>
                            </label>
                        }
                    }).collect::<Vec<_>>().into_any()
                }}

                <p class="mt-3 text-xs text-text-tertiary">
                    {move || if allowed.get().is_empty() {
                        t_string!(i18n, agents.overview.access_everyone).to_string()
                    } else {
                        t_string!(i18n, agents.overview.access_restricted).to_string()
                    }}
                </p>
            </div>

            // A failed load must block the Save button, not merely be reported:
            // the patch writes `name` / `identity` unconditionally, so saving
            // over unloaded fields overwrites them with the empty defaults.
            {move || load_error.get().map(|msg| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{msg}</div>
            })}

            // Status message and save button
            {move || save_message.get().map(|(success, msg)| {
                let class = if success {
                    "p-3 bg-success-subtle border border-success/30 rounded-lg text-success text-sm"
                } else {
                    "p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm"
                };
                view! { <div class=class>{msg}</div> }
            })}

            <div class="flex justify-end items-center gap-3 pt-4 border-t border-border">
                {move || (!detail_loaded.get()).then(|| view! {
                    <span class="text-xs text-text-tertiary">{t!(i18n, agents.overview.save_disabled_unloaded)}</span>
                })}
                <button
                    on:click=handle_save
                    disabled=move || is_saving.get() || !detail_loaded.get()
                    class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                    {move || if is_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, name: &str) -> UserInfo {
        UserInfo {
            user_id: id.to_string(),
            display_name: name.to_string(),
            role: "member".to_string(),
            status: "active".to_string(),
        }
    }

    #[test]
    fn every_known_principal_is_offered() {
        let rows = access_rows(&[user("u-a", "Alice"), user("u-b", "Bob")], &[], "unknown");
        assert_eq!(
            rows.iter().map(|r| r.user_id.as_str()).collect::<Vec<_>>(),
            vec!["u-a", "u-b"]
        );
        assert!(rows.iter().all(|r| !r.unknown_principal));
        assert!(rows[0].label.contains("Alice") && rows[0].label.contains("u-a"));
    }

    /// A grant whose principal no longer exists must still be rendered, or the
    /// next Save — which writes the checked set verbatim — would silently drop
    /// it. Dropping it may well be what the operator wants; doing it without
    /// showing them is what this row prevents.
    #[test]
    fn a_grant_to_a_deleted_principal_stays_visible() {
        let rows = access_rows(
            &[user("u-a", "Alice")],
            &["u-a".to_string(), "u-ghost".to_string()],
            "not a known principal",
        );
        let ghost = rows
            .iter()
            .find(|r| r.user_id == "u-ghost")
            .expect("a granted id absent from users.list must still get a row");
        assert!(ghost.unknown_principal);
        assert!(ghost.label.contains("not a known principal"));
        assert_eq!(rows.len(), 2, "no duplicate row for the id that IS known");
    }

    /// The empty-roster row and the everyone-may-run hint are different facts,
    /// and only this one is derived from the grant list.
    #[test]
    fn no_grants_and_no_users_are_independent() {
        assert!(access_rows(&[], &[], "unknown").is_empty());
        assert_eq!(
            access_rows(&[], &["u-ghost".to_string()], "unknown").len(),
            1
        );
    }
}
