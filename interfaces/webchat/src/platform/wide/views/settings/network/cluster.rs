//! Section 2 — Aleph cluster: fleet list (online + offline merged) + enrollment + deregistration.
//! Node command dispatch is LLM-driven through conversation (R8); the Panel only provides a read-only list + lifecycle management.
//!
//! LAN-trust: **no token**. Nodes register themselves inside `connect`, so what is given to the
//! operator is not a token string, but **the command to run on the target machine**. "Enroll" is only
//! a pre-reservation: occupy the name in the fleet first (appearing as offline), and when a node dials in with the same name it merges into this row.
//!
//! The fleet is **real-time**: `node.connected` / `node.disconnected` have been broadcasting on the event bus all along,
//! but there were never any subscribers — the list was fetched once on page entry and node connect/disconnect was never visible.
//!
//! **Authorization lives on the server, not here** (2026-08-07). This page used
//! to withhold the fleet behind `DashboardState::is_operator()` — the sole
//! consumer of that predicate in the whole frontend — while `environments.list`
//! was open-read on the backend. A client-side role check can never be the
//! enforcement point: the role is captured once at `connect` and
//! `restamp_live_connections` can invalidate it with no notification, so it is
//! wrong in both directions after `users.update`. The RPC family is now
//! admin-gated (`method_admin.rs`'s `environments.` prefix) and the live feed
//! with it (`event_scope.rs`'s `node.` rule), so this page adopts the posture
//! every other settings page already has — no page holds a role gate now:
//! render, call, and show whatever the server says (see [`fleet_error_label`]).

use crate::api::cluster::{ClusterApi, EnrollResult, Environment};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, td_string, use_i18n, Locale};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Node join command. Pure function — `host` is injected by the caller for testability (no `window()` outside the browser).
fn join_command(host: &str, node_name: &str) -> String {
    format!("aleph-server node --center ws://{host} --name {node_name}")
}

/// The host the Panel itself connects to — the Panel is served by this core, so its origin is
/// the center address nodes should dial.
fn center_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "<center-host>:18790".to_string())
}

/// Render Unix seconds as a coarse-grained "how long ago". Offline nodes only need magnitude, not precise timestamps.
///
/// Takes a `Locale` rather than the context: this is a pure function with host
/// unit tests, and `td_string!` resolves against a locale value, so the tests
/// can assert both languages without standing up a reactive owner.
///
/// The magnitude is composed as `{n} {unit}` rather than interpolated into a
/// sentence, which sidesteps plural forms entirely — `t_string!` cannot take a
/// plural key, and "1 minutes ago" is the alternative. Both languages put the
/// number first, so the composition is not smuggling in an English word order.
fn last_seen_label(locale: Locale, last_seen_at: Option<i64>, now_unix: i64) -> String {
    let Some(ts) = last_seen_at else {
        return td_string!(locale, cluster.never_connected).to_string();
    };
    match (now_unix - ts).max(0) {
        s if s < 60 => td_string!(locale, cluster.just_now).to_string(),
        s if s < 3600 => format!("{} {}", s / 60, td_string!(locale, cluster.minutes_ago)),
        s if s < 86_400 => format!("{} {}", s / 3600, td_string!(locale, cluster.hours_ago)),
        s => format!("{} {}", s / 86_400, td_string!(locale, cluster.days_ago)),
    }
}

fn now_unix() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// Copy for a failed fleet call. Pure function — the page renders an error
/// STATE, it never claims a verdict of its own.
///
/// **Called at the write, not at the render.** It used to run inside the view,
/// with the raw protocol string sitting in the signal until something printed
/// it; every render site was then one `{e}` away from showing a member the
/// English refusal, and the signal had to carry `action` around just to reach
/// here. Classifying where the error arrives leaves exactly one place the raw
/// string exists, which is also what lets `admin_refusal`'s scanner see this
/// page at all — it reads writes.
///
/// The admin gate refuses with `AUTH_REQUIRED` + a fixed message, but the
/// Panel's RPC layer keeps only `error.message` (the code is dropped in
/// `context.rs`'s response arm), so the message text is the only recognisable
/// part. Recognition lives in [`crate::components::admin_refusal`] and is
/// matched through [`ADMIN_REQUIRED_MESSAGE`] — **the same `aleph_protocol`
/// constant the server emits**, not a copy of it — so a reword moves the server
/// and every consumer in one edit and can no longer strand a member on the raw
/// English string. Everything else falls through verbatim: degraded copy beats
/// a wrong claim, and this is never a permission decision.
///
/// # `action` exists because this page has three verbs, not one
///
/// The one refusal sentence used to say "…cannot READ the node topology", and
/// it was rendered for all three of them: the fleet read, `+ Enroll`, and a
/// row's `注销`. Two of those are writes, and both told the operator their
/// WRITE had failed to read something. The verdict is shared; the sentence
/// describing what was refused is not.
fn fleet_error_label(locale: Locale, err: &str, action: FleetAction) -> String {
    crate::components::admin_refusal::labeled(err, &action.refusal(locale))
}

/// The three things this page asks the server to do. An enum rather than three
/// `&str` constants so the call sites still read as verbs *and* a fourth one
/// cannot quietly borrow a third one's sentence — with a `match` that has to
/// be exhaustive, adding a verb without giving it copy stops compiling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FleetAction {
    ReadFleet,
    Enroll,
    Deregister,
}

impl FleetAction {
    /// The whole refusal sentence, not a verb spliced into a shared frame.
    ///
    /// One frame plus an interpolated verb would need `interpolate_display`
    /// (a crate-wide leptos_i18n feature) for the sake of a single string, and
    /// it assumes every language can build this sentence by substitution —
    /// verb agreement does not survive that in general. Three complete
    /// sentences also make the property this enum exists for structural rather
    /// than argued: a fourth verb cannot borrow a third one's sentence,
    /// because there is no shared sentence to borrow.
    fn refusal(self, locale: Locale) -> String {
        match self {
            Self::ReadFleet => td_string!(locale, cluster.refused_read_fleet).to_string(),
            Self::Enroll => td_string!(locale, cluster.refused_enroll).to_string(),
            Self::Deregister => td_string!(locale, cluster.refused_deregister).to_string(),
        }
    }
}

#[component]
pub fn ClusterSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let nodes = RwSignal::new(Vec::<Environment>::new());
    // Already-classified copy. The fleet READ and a row's deregister both land
    // here and one sentence cannot honestly describe both, so each write picks
    // its own `ACTION_*` — which is why this is a finished `String` and not
    // `(message, action)`: the verb is known at the write, and carrying it to
    // the render only postponed the moment the raw error stopped existing.
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    // node_id currently being deregistered → disables that row's button.
    let removing = RwSignal::new(Option::<String>::None);

    let show_enroll = RwSignal::new(false);
    let enroll_name = RwSignal::new(String::new());
    let enroll_result = RwSignal::new(Option::<EnrollResult>::None);
    let enroll_err = RwSignal::new(Option::<String>::None);
    // The name the enroll actually succeeded with — the join command must echo
    // that one, not whatever the field happens to hold afterwards.
    let enrolled_name = RwSignal::new(String::new());

    let load = move || {
        let locale = i18n.get_locale_untracked();
        spawn_local(async move {
            match ClusterApi::list_environments(&state).await {
                Ok(list) => {
                    nodes.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(fleet_error_label(locale, &e, FleetAction::ReadFleet))),
            }
            loading.set(false);
        });
    };
    // Ask the server. `environments.list` is admin-gated at the one chokepoint
    // (`method_admin.rs`), so a member gets a refusal here and `error` renders
    // it; nothing is withheld on a guess about who is connected.
    load();

    // Live fleet feed. The center publishes `node.connected` /
    // `node.disconnected` on every node handshake and teardown. Subscribing
    // unconditionally is safe for the same reason: `event_scope.rs`'s `node.`
    // rule refuses delivery to a member's socket, so a non-operator simply
    // never gets a tick.
    Effect::new(move |_| {
        spawn_local(async move {
            let _ = state.subscribe_topic("node.**").await;
        });
    });
    let sub_id = state.subscribe_events(move |evt| {
        if evt.topic == "node.connected" || evt.topic == "node.disconnected" {
            load();
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    let submit_enroll = move |_| {
        let locale = i18n.get_locale_untracked();
        let name = enroll_name.get();
        if name.trim().is_empty() {
            enroll_err.set(Some(td_string!(locale, cluster.name_required).to_string()));
            return;
        }
        enroll_err.set(None);
        spawn_local(async move {
            match ClusterApi::enroll_node(&state, name.clone()).await {
                Ok(r) => {
                    enrolled_name.set(name);
                    enroll_result.set(Some(r));
                    load();
                }
                // `fleet_error_label`, not `settings_load_error`: enrolling is
                // a WRITE, and the generic sentence tells the operator their
                // enrolment failed to *read* server configuration. It also
                // used to run twice — once here and once at the render below —
                // which is exactly the shape that made the wrong sentence hard
                // to see.
                Err(e) => enroll_err.set(Some(fleet_error_label(locale, &e, FleetAction::Enroll))),
            }
        });
    };

    view! {
        <section class="space-y-4">
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, cluster.title)}</h2>
                    <p class="text-sm text-text-secondary">
                        {t!(i18n, cluster.subtitle)}
                    </p>
                </div>
                <button
                    class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                    on:click=move |_| {
                        enroll_result.set(None);
                        enroll_err.set(None);
                        enroll_name.set(String::new());
                        show_enroll.set(true);
                    }
                >
                    "+ Enroll"
                </button>
            </div>

            <div class="bg-surface-raised rounded-lg border border-border p-6">
                <Show when=move || loading.get()>
                    <p class="text-text-secondary text-sm">{t!(i18n, common.loading)}</p>
                </Show>
                // "Nothing here" only means an empty fleet when the read
                // actually succeeded — with an error in hand it would be a
                // second, contradictory claim about the same fact.
                <Show when=move || {
                    !loading.get() && nodes.get().is_empty() && error.get().is_none()
                }>
                    <p class="text-text-secondary text-sm">{t!(i18n, cluster.empty)}</p>
                </Show>
                <For
                    each=move || nodes.get()
                    key=|n| (n.id.clone(), n.status.clone())
                    children=move |node: Environment| {
                        let online = node.is_online();
                        let tags = node.tags.clone();
                        let last_seen = last_seen_label(
                            i18n.get_locale_untracked(),
                            node.last_seen_at,
                            now_unix(),
                        );
                        let cmd_count = node.commands.len();
                        view! {
                            <div class="flex items-center justify-between py-3 border-b border-border last:border-0">
                                <div class="min-w-0">
                                    <div class="text-text-primary font-medium">{node.name.clone()}</div>
                                    <div class="text-xs text-text-tertiary font-mono">
                                        {node.id.clone()}
                                        // The build this machine runs. Skew is allowed (the
                                        // fleet upgrades on its own schedule), but "that node
                                        // behaves differently" needs something to correlate
                                        // against. Absent on offline rows and on nodes older
                                        // than the version handshake.
                                        {node
                                            .version
                                            .clone()
                                            .map(|v| view! { <span class="ml-2">"v" {v}</span> })}
                                    </div>
                                    <Show when={
                                        let has = !tags.is_empty();
                                        move || has
                                    }>
                                        <div class="flex flex-wrap gap-1 mt-1">
                                            {tags
                                                .iter()
                                                .map(|t| {
                                                    view! {
                                                        <span class="px-1.5 py-0.5 rounded bg-surface text-text-secondary text-[11px] font-mono">
                                                            {t.clone()}
                                                        </span>
                                                    }
                                                })
                                                .collect_view()}
                                        </div>
                                    </Show>
                                </div>
                                <div class="flex items-center gap-4 text-xs text-text-secondary">
                                    <span class="flex items-center gap-1">
                                        // Honest status: the merged fleet view carries offline
                                        // nodes too, and every row used to be painted green.
                                        <span class=if online {
                                            "w-2 h-2 rounded-full inline-block bg-success"
                                        } else {
                                            "w-2 h-2 rounded-full inline-block bg-text-tertiary"
                                        }></span>
                                        {node.status.clone()}
                                    </span>
                                    <Show
                                        when=move || online
                                        fallback={
                                            let seen = last_seen.clone();
                                            move || {
                                                view! {
                                                    <span title=move || {
                                                        t_string!(i18n, cluster.last_seen_title).to_string()
                                                    }>{seen.clone()}</span>
                                                }
                                            }
                                        }
                                    >
                                        <span>{cmd_count} " cmds"</span>
                                    </Show>
                                    <button
                                        class="px-2 py-1 rounded border border-border text-error hover:bg-error/10 disabled:opacity-40 disabled:cursor-not-allowed"
                                        disabled={
                                            let id = node.id.clone();
                                            move || removing.get().as_deref() == Some(id.as_str())
                                        }
                                        title=move || t_string!(i18n, cluster.deregister_title).to_string()
                                        on:click={
                                            let node_id = node.id;
                                            move |_| {
                                                let locale = i18n.get_locale_untracked();
                                                let node_id = node_id.clone();
                                                removing.set(Some(node_id.clone()));
                                                spawn_local(async move {
                                                    match ClusterApi::deregister_node(&state, node_id).await {
                                                        Ok(()) => error.set(None),
                                                        Err(e) => {
                                                            error
                                                                .set(
                                                                    Some(
                                                                        fleet_error_label(locale, &e, FleetAction::Deregister),
                                                                    ),
                                                                )
                                                        }
                                                    }
                                                    removing.set(None);
                                                    load();
                                                });
                                            }
                                        }
                                    >
                                        {t!(i18n, cluster.deregister)}
                                    </button>
                                </div>
                            </div>
                        }
                    }
                />
                {move || {
                    error
                        .get()
                        .map(|e| {
                            view! { <p class="text-sm text-error mt-3">{e}</p> }
                        })
                }}
            </div>

            <Show when=move || show_enroll.get()>
                <div class="aleph-scrim fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="glass bg-surface-overlay/85 rounded-lg border border-border p-6 max-w-lg w-full space-y-4">
                        <h3 class="text-text-primary font-semibold">{t!(i18n, cluster.enroll_heading)}</h3>
                        <Show
                            when=move || enroll_result.get().is_none()
                            fallback=move || {
                                let r = enroll_result.get().unwrap();
                                let cmd = join_command(&center_host(), &enrolled_name.get());
                                view! {
                                    <div class="space-y-2">
                                        <p class="text-sm text-text-secondary">
                                            {t!(i18n, cluster.run_command)}
                                        </p>
                                        <textarea
                                            readonly=true
                                            rows="3"
                                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary font-mono text-xs"
                                            prop:value=cmd
                                        ></textarea>
                                        <p class="text-xs text-text-tertiary">
                                            {t!(i18n, cluster.tag_hint)}
                                        </p>
                                        <p class="text-xs text-text-tertiary">"node_id: " {r.node_id}</p>
                                        // Enroll is idempotent: re-enrolling a name returns its
                                        // existing id. Say so, otherwise the operator sees the
                                        // same dialog twice and assumes they created two nodes.
                                        <Show when={
                                            let reused = r.reused;
                                            move || reused
                                        }>
                                            <p class="text-xs text-text-tertiary">
                                                {t!(i18n, cluster.name_reused)}
                                            </p>
                                        </Show>
                                    </div>
                                }
                            }
                        >
                            <input
                                type="text"
                                placeholder=move || t_string!(i18n, cluster.name_placeholder).to_string()
                                class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                                prop:value=move || enroll_name.get()
                                on:input=move |ev| enroll_name.set(event_target_value(&ev))
                            />
                            {move || {
                                enroll_err
                                    .get()
                                    .map(|e| {
                                        view! { <p class="text-sm text-error">{e}</p> }
                                    })
                            }}
                        </Show>
                        <div class="flex justify-end gap-3">
                            <button
                                class="px-3 py-2 text-text-secondary"
                                on:click=move |_| {
                                    show_enroll.set(false);
                                    load();
                                }
                            >
                                {t!(i18n, cluster.close)}
                            </button>
                            <Show when=move || enroll_result.get().is_none()>
                                <button
                                    class="px-4 py-2 bg-primary text-white rounded-lg"
                                    on:click=submit_enroll
                                >
                                    {t!(i18n, cluster.enroll)}
                                </button>
                            </Show>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::{fleet_error_label, join_command, last_seen_label, FleetAction};
    use crate::i18n::Locale;
    // The production code no longer names this constant — recognition moved to
    // `components::admin_refusal`. The tests still feed the SERVER's own words
    // in, which is what keeps them able to fail on a drift.
    use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

    /// Fed the SERVER's own refusal — `aleph_protocol`'s constant, which
    /// `gateway::server::handler` emits verbatim — not a local transcription of
    /// it. That is what makes this assertion able to fail: if this page's
    /// recognition ever drifts away from the words the server actually sends,
    /// the refusal falls through to the raw string and `assert_ne!` fires.
    #[test]
    fn a_refused_fleet_read_renders_as_a_role_explanation() {
        let label = fleet_error_label(Locale::en, ADMIN_REQUIRED_MESSAGE, FleetAction::ReadFleet);
        assert!(
            label.contains("operator"),
            "the refusal must be explained, not echoed as a bare protocol \
             string: {label}"
        );
        assert_ne!(
            label, ADMIN_REQUIRED_MESSAGE,
            "the operator-privilege refusal is the one case this page has \
             better copy for"
        );
    }

    /// One verdict, three verbs. A refused `+ Enroll` and a refused `注销` used
    /// to be told, in so many words, that the connection "cannot READ the node
    /// topology" — a sentence about a read, printed under a write.
    #[test]
    fn each_refused_action_is_described_as_the_action_it_was() {
        // Both locales: the verb is now a locale lookup, so a key that resolves
        // in one language and falls back in the other would put the read's
        // sentence under a write again — in exactly one of the two languages.
        for locale in [Locale::en, Locale::zh] {
            let read = fleet_error_label(locale, ADMIN_REQUIRED_MESSAGE, FleetAction::ReadFleet);
            let enroll = fleet_error_label(locale, ADMIN_REQUIRED_MESSAGE, FleetAction::Enroll);
            let deregister =
                fleet_error_label(locale, ADMIN_REQUIRED_MESSAGE, FleetAction::Deregister);

            for (label, name) in [(&enroll, "enroll"), (&deregister, "deregister")] {
                assert_ne!(label, &read, "{name} must not reuse the read's sentence");
            }
            assert_ne!(
                enroll, deregister,
                "the two writes must not share a sentence"
            );
        }

        // The English sentences must each name their own verb. Asserting on the
        // words is the point: this is the locale an operator sees when the
        // Chinese original was the only copy that existed.
        let en = |a: FleetAction| fleet_error_label(Locale::en, ADMIN_REQUIRED_MESSAGE, a);
        assert!(en(FleetAction::ReadFleet).contains("read the node topology"));
        assert!(en(FleetAction::Enroll).contains("enroll"));
        assert!(en(FleetAction::Deregister).contains("deregister"));
        {}
    }

    #[test]
    fn every_other_failure_shows_the_servers_own_words() {
        // A transport failure, a store error, a malformed response: none of
        // these are permission verdicts, and inventing copy for them would put
        // a guess in front of the operator instead of the cause. The same holds
        // for this page's OWN local validation message, which shares the enroll
        // error slot with the server's replies.
        for raw in [
            "Invalid response: missing environments",
            "WebSocket disconnected",
            "Internal error: failed to read enrolled node devices",
            "请先填写节点名称",
        ] {
            for action in [
                FleetAction::ReadFleet,
                FleetAction::Enroll,
                FleetAction::Deregister,
            ] {
                assert_eq!(
                    fleet_error_label(Locale::en, raw, action),
                    raw,
                    "{raw} must pass through"
                );
            }
        }
    }

    /// The page holds no permission opinion of its own any more. A source-level
    /// pin because the failure it guards against is additive — someone
    /// reintroducing a client-side role check would compile, pass every other
    /// test, and silently restore a gate that is wrong in both directions after
    /// `users.update` re-stamps a live connection's role.
    #[test]
    fn the_cluster_page_holds_no_client_side_role_gate() {
        // `production_lines` is this crate's one answer to "where does
        // production code end" — it walks `#[cfg(test)]` ITEMS instead of
        // cutting at the first marker, and it drops whole-line comments on
        // the way out. Both matter here: the module doc above names the
        // deleted predicate, and that mention must not be what keeps the pin
        // green; and a `#[cfg(test)]` on anything but the trailing test module
        // used to truncate this scan there, which is a green that means "I
        // could not see you".
        let code: String = crate::i18n_census::production_lines(include_str!("cluster.rs"))
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("is_operator"),
            "this page must not gate on a client-captured role — \
             `environments.` is admin-gated in method_admin.rs and `node.` in \
             event_scope.rs; the server is the enforcement point"
        );
    }

    #[test]
    fn never_connected_node_says_so() {
        assert_eq!(last_seen_label(Locale::zh, None, 1_700_000_000), "从未连入");
        assert_eq!(
            last_seen_label(Locale::en, None, 1_700_000_000),
            "Never connected"
        );
    }

    #[test]
    fn last_seen_buckets_by_magnitude() {
        let now = 1_700_000_000;
        assert_eq!(last_seen_label(Locale::zh, Some(now - 30), now), "刚刚");
        assert_eq!(
            last_seen_label(Locale::zh, Some(now - 600), now),
            "10 分钟前"
        );
        assert_eq!(
            last_seen_label(Locale::zh, Some(now - 7200), now),
            "2 小时前"
        );
        assert_eq!(
            last_seen_label(Locale::zh, Some(now - 172_800), now),
            "2 天前"
        );
        // A clock skew that puts last_seen in the future must not underflow.
        assert_eq!(last_seen_label(Locale::zh, Some(now + 500), now), "刚刚");
    }

    /// The English bucket labels are the reason this page was localised at all:
    /// an operator who picked English used to read the fleet's ages in Chinese.
    #[test]
    fn the_buckets_speak_english_too() {
        let now = 1_700_000_000;
        assert_eq!(last_seen_label(Locale::en, Some(now - 30), now), "Just now");
        assert_eq!(
            last_seen_label(Locale::en, Some(now - 600), now),
            "10 min ago"
        );
        assert_eq!(
            last_seen_label(Locale::en, Some(now - 7200), now),
            "2 h ago"
        );
        assert_eq!(
            last_seen_label(Locale::en, Some(now - 172_800), now),
            "2 d ago"
        );
    }

    #[test]
    fn join_command_targets_the_center_the_panel_is_served_from() {
        assert_eq!(
            join_command("10.0.0.4:18790", "worker-1"),
            "aleph-server node --center ws://10.0.0.4:18790 --name worker-1"
        );
    }
}
