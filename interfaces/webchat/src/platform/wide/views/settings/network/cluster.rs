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
fn last_seen_label(last_seen_at: Option<i64>, now_unix: i64) -> String {
    let Some(ts) = last_seen_at else {
        return "从未连入".to_string();
    };
    match (now_unix - ts).max(0) {
        s if s < 60 => "刚刚".to_string(),
        s if s < 3600 => format!("{} 分钟前", s / 60),
        s if s < 86_400 => format!("{} 小时前", s / 3600),
        s => format!("{} 天前", s / 86_400),
    }
}

fn now_unix() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// Copy for a failed fleet call. Pure function — the page renders an error
/// STATE, it never claims a verdict of its own.
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
fn fleet_error_label(err: &str, action: &str) -> String {
    crate::components::admin_refusal::labeled(
        err,
        &format!("集群管理需要 operator 权限,当前连接的角色无法{action}。"),
    )
}

/// The three things this page asks the server to do. Named so the call sites
/// read as verbs and a fourth one cannot quietly borrow a third one's sentence.
const ACTION_READ_FLEET: &str = "读取节点拓扑";
const ACTION_ENROLL: &str = "登记新节点";
const ACTION_DEREGISTER: &str = "注销节点";

#[component]
pub fn ClusterSection() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let state = expect_context::<DashboardState>();
    let nodes = RwSignal::new(Vec::<Environment>::new());
    // (message, what was being attempted) — the fleet READ and a row's
    // deregister both land here, and one sentence cannot honestly describe both.
    let error = RwSignal::new(Option::<(String, &'static str)>::None);
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
        spawn_local(async move {
            match ClusterApi::list_environments(&state).await {
                Ok(list) => {
                    nodes.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some((e, ACTION_READ_FLEET))),
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
        let name = enroll_name.get();
        if name.trim().is_empty() {
            enroll_err.set(Some("请先填写节点名称".to_string()));
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
                Err(e) => enroll_err.set(Some(
                    crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    };

    view! {
        <section class="space-y-4">
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary mb-1">"Aleph 集群"</h2>
                    <p class="text-sm text-text-secondary">
                        "本服务作为 center 登记并管理的 node 执行臂。"
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
                    <p class="text-text-secondary text-sm">"加载中…"</p>
                </Show>
                // "Nothing here" only means an empty fleet when the read
                // actually succeeded — with an error in hand it would be a
                // second, contradictory claim about the same fact.
                <Show when=move || {
                    !loading.get() && nodes.get().is_empty() && error.get().is_none()
                }>
                    <p class="text-text-secondary text-sm">"暂无已登记节点。"</p>
                </Show>
                <For
                    each=move || nodes.get()
                    key=|n| (n.id.clone(), n.status.clone())
                    children=move |node: Environment| {
                        let online = node.is_online();
                        let tags = node.tags.clone();
                        let last_seen = last_seen_label(node.last_seen_at, now_unix());
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
                                                view! { <span title="最近一次在线">{seen.clone()}</span> }
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
                                        title="注销该节点(驱逐会话并吊销设备记录;它无法再连回来)"
                                        on:click={
                                            let node_id = node.id;
                                            move |_| {
                                                let node_id = node_id.clone();
                                                removing.set(Some(node_id.clone()));
                                                spawn_local(async move {
                                                    match ClusterApi::deregister_node(&state, node_id).await {
                                                        Ok(()) => error.set(None),
                                                        Err(e) => error.set(Some((e, ACTION_DEREGISTER))),
                                                    }
                                                    removing.set(None);
                                                    load();
                                                });
                                            }
                                        }
                                    >
                                        "注销"
                                    </button>
                                </div>
                            </div>
                        }
                    }
                />
                {move || {
                    error
                        .get()
                        .map(|(e, action)| {
                            view! {
                                <p class="text-sm text-error mt-3">{fleet_error_label(&e, action)}</p>
                            }
                        })
                }}
            </div>

            <Show when=move || show_enroll.get()>
                <div class="aleph-scrim fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="glass bg-surface-overlay/85 rounded-lg border border-border p-6 max-w-lg w-full space-y-4">
                        <h3 class="text-text-primary font-semibold">"登记新节点"</h3>
                        <Show
                            when=move || enroll_result.get().is_none()
                            fallback=move || {
                                let r = enroll_result.get().unwrap();
                                let cmd = join_command(&center_host(), &enrolled_name.get());
                                view! {
                                    <div class="space-y-2">
                                        <p class="text-sm text-text-secondary">
                                            "在目标机器上运行这条命令,节点会自己接入(无需 token):"
                                        </p>
                                        <textarea
                                            readonly=true
                                            rows="3"
                                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary font-mono text-xs"
                                            prop:value=cmd
                                        ></textarea>
                                        <p class="text-xs text-text-tertiary">
                                            "追加 --tag gpu --tag region=us 可给节点打标签(node_invoke_many 按标签扇出)。"
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
                                                "这个名字此前已登记过,沿用原有 node_id(未新建节点)。"
                                            </p>
                                        </Show>
                                    </div>
                                }
                            }
                        >
                            <input
                                type="text"
                                placeholder="node 名称(须与 --name 一致)"
                                class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                                prop:value=move || enroll_name.get()
                                on:input=move |ev| enroll_name.set(event_target_value(&ev))
                            />
                            {move || {
                                enroll_err
                                    .get()
                                    .map(|e| {
                                        view! {
                                            <p class="text-sm text-error">{fleet_error_label(&e, ACTION_ENROLL)}</p>
                                        }
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
                                "关闭"
                            </button>
                            <Show when=move || enroll_result.get().is_none()>
                                <button
                                    class="px-4 py-2 bg-primary text-white rounded-lg"
                                    on:click=submit_enroll
                                >
                                    "登记"
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
    use super::{
        fleet_error_label, join_command, last_seen_label, ACTION_DEREGISTER, ACTION_ENROLL,
        ACTION_READ_FLEET,
    };
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
        let label = fleet_error_label(ADMIN_REQUIRED_MESSAGE, ACTION_READ_FLEET);
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
        let read = fleet_error_label(ADMIN_REQUIRED_MESSAGE, ACTION_READ_FLEET);
        let enroll = fleet_error_label(ADMIN_REQUIRED_MESSAGE, ACTION_ENROLL);
        let deregister = fleet_error_label(ADMIN_REQUIRED_MESSAGE, ACTION_DEREGISTER);

        assert!(enroll.contains(ACTION_ENROLL), "{enroll}");
        assert!(deregister.contains(ACTION_DEREGISTER), "{deregister}");
        for (label, name) in [(&enroll, "enroll"), (&deregister, "deregister")] {
            assert!(
                !label.contains(ACTION_READ_FLEET),
                "a refused {name} must not be described as a failed read: {label}"
            );
            assert_ne!(label, &read, "{name} must not reuse the read's sentence");
        }
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
            for action in [ACTION_READ_FLEET, ACTION_ENROLL, ACTION_DEREGISTER] {
                assert_eq!(
                    fleet_error_label(raw, action),
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
        let src = include_str!("cluster.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one segment");
        // Comments are excluded on purpose: the module doc above names the
        // deleted predicate so a reader can find this decision, and that
        // mention must not be what keeps the pin green.
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
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
        assert_eq!(last_seen_label(None, 1_700_000_000), "从未连入");
    }

    #[test]
    fn last_seen_buckets_by_magnitude() {
        let now = 1_700_000_000;
        assert_eq!(last_seen_label(Some(now - 30), now), "刚刚");
        assert_eq!(last_seen_label(Some(now - 600), now), "10 分钟前");
        assert_eq!(last_seen_label(Some(now - 7200), now), "2 小时前");
        assert_eq!(last_seen_label(Some(now - 172_800), now), "2 天前");
        // A clock skew that puts last_seen in the future must not underflow.
        assert_eq!(last_seen_label(Some(now + 500), now), "刚刚");
    }

    #[test]
    fn join_command_targets_the_center_the_panel_is_served_from() {
        assert_eq!(
            join_command("10.0.0.4:18790", "worker-1"),
            "aleph-server node --center ws://10.0.0.4:18790 --name worker-1"
        );
    }
}
