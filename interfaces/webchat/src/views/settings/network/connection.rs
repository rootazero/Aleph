//! Section 1 — 服务连接(Feature A):切换 shell 连接的 Aleph 服务(本地/远程)。
//! 仅桌面 Tauri shell 内可交互;纯浏览器内只读降级。

use crate::api::tauri_bridge;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn ConnectionSection() -> impl IntoView {
    let i18n = use_i18n();
    let in_shell = tauri_bridge::is_shell();

    let remote_input = RwSignal::new(String::new());
    let use_remote = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let show_confirm = RwSignal::new(false);

    if in_shell {
        spawn_local(async move {
            if let Ok(t) = tauri_bridge::get_connection_target().await {
                let is_remote = t != "local";
                use_remote.set(is_remote);
                if is_remote {
                    remote_input.set(t);
                }
            }
        });
    }

    let apply = move |_| {
        error.set(None);
        let raw = if use_remote.get() {
            remote_input.get()
        } else {
            "local".to_string()
        };
        busy.set(true);
        spawn_local(async move {
            match tauri_bridge::set_connection_target(&raw).await {
                // 成功后 shell reroute webview,本视图销毁
                Ok(()) => {}
                Err(e) => {
                    error.set(Some(e));
                    busy.set(false);
                    show_confirm.set(false);
                }
            }
        });
    };

    view! {
        <section class="space-y-4">
            <div>
                <h2 class="text-lg font-semibold text-text-primary mb-1">"服务连接"</h2>
                <p class="text-sm text-text-secondary">
                    "选择本 Panel 连接的 Aleph 服务(本地或远程)。"
                </p>
            </div>

            <Show
                when=move || in_shell
                fallback=move || view! {
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <p class="text-sm text-text-secondary">
                            "当前在浏览器中运行,连接切换仅在桌面 App 内可用。"
                        </p>
                    </div>
                }
            >
                <div class="bg-surface-raised rounded-lg border border-border p-6 space-y-4">
                    <label class="flex items-center gap-3">
                        <input type="radio" name="conn"
                            prop:checked=move || !use_remote.get()
                            on:change=move |_| use_remote.set(false) />
                        <span class="text-text-primary">{t!(i18n, settings.network.local_service)}</span>
                    </label>
                    <label class="flex items-center gap-3">
                        <input type="radio" name="conn"
                            prop:checked=move || use_remote.get()
                            on:change=move |_| use_remote.set(true) />
                        <span class="text-text-primary">{t!(i18n, settings.network.remote_service)}</span>
                    </label>

                    <Show when=move || use_remote.get()>
                        <input type="text"
                            placeholder="https://core.example:18790"
                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                            prop:value=move || remote_input.get()
                            on:input=move |ev| remote_input.set(event_target_value(&ev)) />
                        <p class="text-xs text-text-tertiary">
                            "预览:" {move || tauri_bridge::normalize_endpoint_preview(&remote_input.get())}
                        </p>
                    </Show>

                    <div class="flex items-center gap-3 pt-2">
                        <button
                            class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                            disabled=move || busy.get()
                            on:click=move |_| show_confirm.set(true)>
                            "应用"
                        </button>
                    </div>

                    {move || error.get().map(|e| view! { <p class="text-sm text-error">{e}</p> })}
                </div>
            </Show>

            <Show when=move || show_confirm.get()>
                <div class="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-md space-y-4">
                        <p class="text-text-primary">
                            "将切换到 "
                            {move || if use_remote.get() { remote_input.get() } else { "本地".to_string() }}
                            " 并重新加载 Panel,确认?"
                        </p>
                        <div class="flex justify-end gap-3">
                            <button class="px-3 py-2 text-text-secondary"
                                on:click=move |_| show_confirm.set(false)>"取消"</button>
                            <button class="px-4 py-2 bg-primary text-white rounded-lg"
                                disabled=move || busy.get()
                                on:click=apply>"确认切换"</button>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}
