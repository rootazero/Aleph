//! Section 1 — 服务连接(Feature A):切换 shell 连接的 Aleph 服务(本地/远程)。
//! 仅桌面 Tauri shell 内可交互;纯浏览器内只读降级。

use crate::api::tauri_bridge;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Resolve the connection-target string the "Apply" action should send, or
/// `None` when the form is incomplete (remote selected but no address). A blank
/// remote address must never be sent: the shell's `ConnectionTarget::parse("")`
/// treats it as `Local`, so an empty box would silently switch to local against
/// the user's stated intent. We block it here at the source of intent (and the
/// Apply button is disabled on the same predicate). Pure — unit-tested below.
fn resolve_apply_target(use_remote: bool, remote_input: &str) -> Option<String> {
    if use_remote {
        let trimmed = remote_input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        Some("local".to_string())
    }
}

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
        // 远程模式空地址不能静默回退本地(shell 端 `ConnectionTarget::parse("")`
        // 会判为 Local)——在意图源头拦截。Apply 按钮亦据此禁用,这里是逻辑兜底。
        let Some(raw) = resolve_apply_target(use_remote.get(), &remote_input.get()) else {
            show_confirm.set(false);
            return;
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
                <h2 class="text-lg font-semibold text-text-primary mb-1">
                    {t!(i18n, settings.network.section_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.network.description)}
                </p>
            </div>

            <Show
                when=move || in_shell
                fallback=move || view! {
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <p class="text-sm text-text-secondary">
                            {t!(i18n, settings.network.browser_only)}
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
                            {t!(i18n, settings.network.preview)}" "
                            {move || tauri_bridge::normalize_endpoint_preview(&remote_input.get())}
                        </p>
                    </Show>

                    <div class="flex items-center gap-3 pt-2">
                        <button
                            class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                            disabled=move || busy.get()
                                || resolve_apply_target(use_remote.get(), &remote_input.get()).is_none()
                            on:click=move |_| show_confirm.set(true)>
                            {t!(i18n, settings.network.apply)}
                        </button>
                    </div>

                    {move || error.get().map(|e| view! { <p class="text-sm text-error">{e}</p> })}
                </div>
            </Show>

            <Show when=move || show_confirm.get()>
                <div class="aleph-scrim fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="glass bg-surface-overlay/85 rounded-lg border border-border p-6 max-w-md space-y-4">
                        <p class="text-text-primary">
                            {t!(i18n, settings.network.confirm_switch,
                                target = move || if use_remote.get() {
                                    remote_input.get()
                                } else {
                                    t_string!(i18n, settings.network.local_target).to_string()
                                })}
                        </p>
                        <div class="flex justify-end gap-3">
                            <button class="px-3 py-2 text-text-secondary"
                                on:click=move |_| show_confirm.set(false)>
                                {t!(i18n, common.cancel)}
                            </button>
                            <button class="px-4 py-2 bg-primary text-white rounded-lg"
                                disabled=move || busy.get()
                                on:click=apply>
                                {t!(i18n, settings.network.confirm_switch_action)}
                            </button>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_apply_target;

    #[test]
    fn blank_remote_address_is_blocked() {
        // 选「远程服务」但地址为空/纯空白 → 不下发:否则 shell 的
        // `ConnectionTarget::parse("")` 会把它当作 Local 静默切换(违背用户意图)。
        assert_eq!(resolve_apply_target(true, ""), None);
        assert_eq!(resolve_apply_target(true, "   "), None);
    }

    #[test]
    fn remote_address_is_trimmed_and_sent() {
        assert_eq!(
            resolve_apply_target(true, "  box.lan:9000 "),
            Some("box.lan:9000".to_string())
        );
    }

    #[test]
    fn local_always_resolves_regardless_of_input() {
        assert_eq!(resolve_apply_target(false, ""), Some("local".to_string()));
        assert_eq!(
            resolve_apply_target(false, "ignored"),
            Some("local".to_string())
        );
    }
}
