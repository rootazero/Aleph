//! Section 2 — Aleph 集群(Feature B 骨架):列出节点 + Enroll。
//! Invoke / bash / deregister 待 feat/cluster-phase0c-core 合并(此处禁用占位)。

use crate::api::cluster::{ClusterApi, EnrollResult, Environment};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn ClusterSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let nodes = RwSignal::new(Vec::<Environment>::new());
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);

    let show_enroll = RwSignal::new(false);
    let enroll_name = RwSignal::new(String::new());
    let enroll_result = RwSignal::new(Option::<EnrollResult>::None);
    let enroll_err = RwSignal::new(Option::<String>::None);

    let load = move || {
        spawn_local(async move {
            loading.set(true);
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

    let submit_enroll = move |_| {
        let name = enroll_name.get();
        enroll_err.set(None);
        spawn_local(async move {
            match ClusterApi::enroll_node(&state, name).await {
                Ok(r) => enroll_result.set(Some(r)),
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
                        key=|n| n.id.clone()
                        children=move |node: Environment| {
                            view! {
                                <div class="flex items-center justify-between py-3 border-b border-border last:border-0">
                                    <div class="min-w-0">
                                        <div class="text-text-primary font-medium">{node.name.clone()}</div>
                                        <div class="text-xs text-text-tertiary font-mono">{node.id.clone()}</div>
                                    </div>
                                    <div class="flex items-center gap-4 text-xs text-text-secondary">
                                        <span class="flex items-center gap-1">
                                            <span class="w-2 h-2 rounded-full bg-success inline-block"></span>
                                            {node.status.clone()}
                                        </span>
                                        <span>{node.commands.len()} " cmds"</span>
                                        <button
                                            class="px-2 py-1 rounded border border-border opacity-40 cursor-not-allowed"
                                            disabled=true
                                            title="feat/cluster-phase0c-core 收尾后启用"
                                        >
                                            "Invoke"
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
                <div class="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-md w-full space-y-4">
                        <h3 class="text-text-primary font-semibold">"登记新节点"</h3>
                        <Show
                            when=move || enroll_result.get().is_none()
                            fallback=move || {
                                let r = enroll_result.get().unwrap();
                                view! {
                                    <div class="space-y-2">
                                        <p class="text-sm text-text-secondary">"在目标机器上用此 token 加入:"</p>
                                        <textarea
                                            readonly=true
                                            rows="3"
                                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary font-mono text-xs"
                                            prop:value=r.token.clone()
                                        ></textarea>
                                        <p class="text-xs text-text-tertiary">"node_id: " {r.node_id.clone()}</p>
                                    </div>
                                }
                            }
                        >
                            <input
                                type="text"
                                placeholder="node 名称"
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
                                    "生成 token"
                                </button>
                            </Show>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}
