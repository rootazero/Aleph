//! Section 1 — 服务连接(Feature A):反映并(在可行时)切换本 Panel 连接的 Aleph
//! 服务(本地/远程)。
//!
//! 关键约束:Panel 由它所连接的核心 serve(`rust_embed`)。当壳连远程时,Panel 跑在
//! 远程 origin,壳的 Tauri IPC 被 capability 限定在 loopback、对远程 origin 不可达。
//! 因此本段不能盲目依赖 `get_connection_target` IPC,而是:
//!   · 用 `location.host` 判定"当前连的是哪个核心"(权威、永远新鲜,IPC 不可用时仍有);
//!   · 用壳注入的 `data-shell-variant` 标记判定宿主形态(full/lite/无=浏览器);
//! 据此决定:loopback(IPC 通)→ 交互式切换;远程 origin → 只读指示 + 托盘指引;
//! 纯浏览器 → 只读说明。

use crate::api::tauri_bridge;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Which shell (if any) hosts this Panel document — read from the one-way
/// `data-shell-variant` marker the desktop shell injects on the document root
/// (see `desktop/shell/src/main.rs`). The marker rides every navigation, so it
/// is present even on a remote-origin Panel where the Tauri IPC is scoped out.
/// `None` ⇒ a plain browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellVariant {
    /// Full app — embeds and supervises a loopback `aleph-server`.
    Full,
    /// Panel-only (lite) shell — points the webview at a remote core.
    Lite,
}

/// What the section should render. Pure decision (unit-tested) so the routing
/// rules stay out of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionMode {
    /// Shell-hosted at a loopback origin: the connection-config IPC is reachable
    /// (capability scopes IPC to loopback), so the interactive local/remote
    /// switch works.
    Switchable,
    /// Shell-hosted at a remote origin: the Panel is served by the remote core
    /// and cannot drive the shell's target from here (IPC scoped out). Read-only
    /// indicator + tray guidance. `lite` distinguishes the panel-only shell from
    /// a full app that happens to be pointed at a remote.
    RemoteReadOnly { lite: bool },
    /// Plain browser tab — no shell to switch.
    Browser,
}

/// Decide what to render from the host context. `variant` is the injected shell
/// marker; `loopback` whether the document origin is a loopback host. Pure.
fn decide_mode(variant: Option<ShellVariant>, loopback: bool) -> ConnectionMode {
    match variant {
        None => ConnectionMode::Browser,
        Some(_) if loopback => ConnectionMode::Switchable,
        Some(v) => ConnectionMode::RemoteReadOnly {
            lite: matches!(v, ShellVariant::Lite),
        },
    }
}

/// The `host:port` of the core this Panel is served by (and talks to) — the
/// authoritative, always-fresh answer to "which core am I connected to",
/// available even when the Tauri IPC is not. Empty string if unavailable.
fn current_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default()
}

/// Read the shell-variant marker the desktop shell injects on the document root.
fn shell_variant() -> Option<ShellVariant> {
    let v = web_sys::window()?
        .document()?
        .document_element()?
        .get_attribute("data-shell-variant")?;
    match v.as_str() {
        "full" => Some(ShellVariant::Full),
        "lite" => Some(ShellVariant::Lite),
        _ => None,
    }
}

/// Strip the port from a `host[:port]`, handling IPv6 literals (`[::1]:port`).
/// Pure.
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}

/// Whether a `host[:port]` names a loopback core. Pure.
fn is_loopback_host(host: &str) -> bool {
    let h = host_only(host);
    h.eq_ignore_ascii_case("localhost") || h == "::1" || h.starts_with("127.")
}

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

    // Computed once: the origin and shell marker don't change for the life of a
    // page (a target switch navigates → fresh page → recomputed). No signals.
    let host = current_host();
    let loopback = is_loopback_host(&host);
    let mode = decide_mode(shell_variant(), loopback);
    let remote = !loopback;
    let host_present = !host.is_empty();

    let body = match mode {
        ConnectionMode::Switchable => view! { <SwitchableConnection /> }.into_any(),
        ConnectionMode::RemoteReadOnly { lite } => view! {
            <div class="bg-surface-raised rounded-lg border border-border p-6 space-y-2">
                <p class="text-sm text-text-primary">
                    {if lite {
                        view! { {t!(i18n, settings.network.remote_readonly_lite)} }.into_any()
                    } else {
                        view! { {t!(i18n, settings.network.remote_readonly_full)} }.into_any()
                    }}
                </p>
                <p class="text-xs text-text-tertiary">
                    {t!(i18n, settings.network.remote_readonly_hint)}
                </p>
            </div>
        }
        .into_any(),
        ConnectionMode::Browser => view! {
            <div class="bg-surface-raised rounded-lg border border-border p-6">
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.network.browser_only)}
                </p>
            </div>
        }
        .into_any(),
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

            // Honest, always-fresh indicator of the core this Panel is actually
            // connected to — shown in every mode (derived from `location.host`,
            // not from the possibly-unreachable shell IPC).
            <Show when=move || host_present>
                <div class="flex items-center gap-2 text-sm">
                    <span class="text-text-secondary">
                        {t!(i18n, settings.network.connected_label)}
                    </span>
                    <span class="font-mono text-text-primary">{host.clone()}</span>
                    <span class=move || {
                        if remote {
                            "px-2 py-0.5 rounded-full text-xs bg-warning/15 text-warning"
                        } else {
                            "px-2 py-0.5 rounded-full text-xs bg-success/15 text-success"
                        }
                    }>
                        {if remote {
                            view! { {t!(i18n, settings.network.badge_remote)} }.into_any()
                        } else {
                            view! { {t!(i18n, settings.network.badge_local)} }.into_any()
                        }}
                    </span>
                </div>
            </Show>

            {body}
        </section>
    }
}

/// The interactive local/remote switch — only mounted when the connection-config
/// IPC is reachable (shell-hosted at a loopback origin). Drives the shell's
/// `set_connection_target` command, which re-routes the webview on success.
#[component]
fn SwitchableConnection() -> impl IntoView {
    let i18n = use_i18n();

    let remote_input = RwSignal::new(String::new());
    let use_remote = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let show_confirm = RwSignal::new(false);
    // Whether the *currently persisted* target is Local — captured at load so
    // the confirm dialog can note (only when leaving Local for Remote) that the
    // local service keeps running. Distinct from `use_remote`, which tracks the
    // pending choice. Defaults true (a missing marker resolves to Local).
    let current_is_local = RwSignal::new(true);

    spawn_local(async move {
        if let Ok(t) = tauri_bridge::get_connection_target().await {
            let is_remote = t != "local";
            use_remote.set(is_remote);
            current_is_local.set(!is_remote);
            if is_remote {
                remote_input.set(t);
            }
        }
    });

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
                    <Show when=move || current_is_local.get() && use_remote.get()>
                        <p class="text-xs text-text-tertiary">
                            {t!(i18n, settings.network.local_stays_running)}
                        </p>
                    </Show>
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
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decide_mode, host_only, is_loopback_host, resolve_apply_target, ConnectionMode,
        ShellVariant,
    };

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

    #[test]
    fn host_only_strips_port_and_handles_ipv6() {
        assert_eq!(host_only("127.0.0.1:18790"), "127.0.0.1");
        assert_eq!(host_only("box.lan"), "box.lan");
        assert_eq!(host_only("[::1]:18790"), "::1");
        assert_eq!(host_only("[fe80::1]"), "fe80::1");
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1:18790"));
        assert!(is_loopback_host("127.5.6.7"));
        assert!(is_loopback_host("localhost:18790"));
        assert!(is_loopback_host("LocalHost"));
        assert!(is_loopback_host("[::1]:18790"));
        assert!(!is_loopback_host("172.245.43.211:18790"));
        assert!(!is_loopback_host("core.example:18790"));
    }

    #[test]
    fn mode_browser_when_no_shell_marker() {
        // No shell host → plain browser, regardless of origin.
        assert_eq!(decide_mode(None, true), ConnectionMode::Browser);
        assert_eq!(decide_mode(None, false), ConnectionMode::Browser);
    }

    #[test]
    fn mode_switchable_only_at_loopback() {
        // Shell + loopback origin → IPC reachable → interactive switch.
        assert_eq!(
            decide_mode(Some(ShellVariant::Full), true),
            ConnectionMode::Switchable
        );
        assert_eq!(
            decide_mode(Some(ShellVariant::Lite), true),
            ConnectionMode::Switchable
        );
    }

    #[test]
    fn mode_remote_readonly_carries_variant() {
        // Lite shell at a remote origin → read-only, flagged lite.
        assert_eq!(
            decide_mode(Some(ShellVariant::Lite), false),
            ConnectionMode::RemoteReadOnly { lite: true }
        );
        // Full app pointed at a remote → read-only too (IPC scoped out), not lite.
        assert_eq!(
            decide_mode(Some(ShellVariant::Full), false),
            ConnectionMode::RemoteReadOnly { lite: false }
        );
    }
}
