//! Section 2 — Aleph cluster: fleet list (online + offline merged) + enrollment + deregistration.
//! Node command dispatch is LLM-driven through conversation (R8); the Panel only provides a read-only list + lifecycle management.
//!
//! LAN-trust: **no token**. Nodes register themselves inside `connect`, so what is given to the
//! operator is not a token string, but **the command to run on the target machine**. "Enroll" is only
//! a pre-reservation: occupy the name in the fleet first (appearing as offline), and when a node dials in with the same name it merges into this row.
//!
//! The fleet is **real-time**: `node.connected` / `node.disconnected` have been broadcasting on the event bus all along,
//! but there were never any subscribers — the list was fetched once on page entry and node connect/disconnect was never visible.

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

#[component]
pub fn ClusterSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let nodes = RwSignal::new(Vec::<Environment>::new());
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
        spawn_local(async move {
            match ClusterApi::list_environments(&state).await {
                Ok(list) => {
                    nodes.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };
    // Gate the list fetch on the operator role captured at connect time.
    // `environments.list` is open-read on the backend, but the UI must not
    // expose cluster topology to non-operators (R8 / spec §4 Section 2).
    if state.is_operator() {
        load();
    } else {
        loading.set(false);
    }

    // Live fleet feed. The center already publishes `node.connected` /
    // `node.disconnected` on every node handshake and teardown; nothing was
    // listening, so the list went stale the moment it rendered.
    if state.is_operator() {
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
    }

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
                Err(e) => enroll_err.set(Some(e)),
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
                    disabled=move || !state.is_operator()
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

            <Show when=move || !state.is_operator()>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <p class="text-sm text-text-secondary">"集群管理需要 operator 权限。"</p>
                </div>
            </Show>

            <Show when=move || state.is_operator()>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <Show when=move || loading.get()>
                        <p class="text-text-secondary text-sm">"加载中…"</p>
                    </Show>
                    <Show when=move || !loading.get() && nodes.get().is_empty()>
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
                                                            Err(e) => error.set(Some(e)),
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
                    {move || error.get().map(|e| view! { <p class="text-sm text-error mt-3">{e}</p> })}
                </div>
            </Show>

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
                            {move || enroll_err.get().map(|e| view! { <p class="text-sm text-error">{e}</p> })}
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
    use super::{join_command, last_seen_label};

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
