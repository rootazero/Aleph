//! 权限分层 UI:配置页闸门 (ConfigGate)、全局身份横幅 (`PermissionBanner`)、
//! 以及 RPC 权限拒绝错误的友好映射。复用 `DashboardState::is_operator()`
//! —— 后端 2 层 tier 在前端的诚实投影。后端零改动。

use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;

/// 配置页闸门:operator 渲染整页 children;非 operator 渲染锁定卡。
/// 在 `SettingsRouter` 路由层包住 config-write 页 —— 门控集中一处。
#[component]
pub fn ConfigGate(children: ChildrenFn) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    view! {
        <Show
            when=move || state.is_operator()
            fallback=move || view! { <LockedNotice /> }
        >
            {children()}
        </Show>
    }
}

/// 非 operator 打开配置页时的锁定卡。
#[component]
fn LockedNotice() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="p-6">
            <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-2xl">
                <h2 class="text-lg font-semibold text-text-primary mb-2">
                    {t!(i18n, settings.permission.locked_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.permission.locked_notice)}
                </p>
            </div>
        </div>
    }
}

/// Settings 区顶部常驻横幅:仅非 operator 显示,解释配置已锁定 + 如何提权。
#[component]
#[must_use]
pub fn PermissionBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    view! {
        <Show when=move || !state.is_operator()>
            <div class="mx-4 mt-3 px-4 py-2.5 rounded-lg border border-warning/40 bg-warning/10 text-sm text-text-secondary flex items-start gap-2">
                <svg class="w-4 h-4 mt-0.5 shrink-0 text-warning" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 9v4" /><path d="M12 17h.01" />
                    <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                </svg>
                <span>{t!(i18n, settings.permission.banner_chat)}</span>
            </div>
        </Show>
    }
}

/// 后端 RPC 错误消息是否为"权限不足"类(operator-only 方法 / 配置工具闸口)。
/// 纯字符串匹配,host 可测。后端消息为英文,沿用 raw-RPC-error-英文 约定。
pub(crate) fn is_permission_denied(raw: &str) -> bool {
    let l = raw.to_ascii_lowercase();
    l.contains("operator privileges required")
        || l.contains("permission denied")
        || l.contains("permissiondenied")
}

/// 把 RPC 错误消息映射为面向用户的展示串。权限拒绝替换成可操作提示
/// (指向「设置 → 安全」提权 / 重新配对选 Config);其余原样透传。
#[must_use]
pub fn friendly_error(raw: &str) -> String {
    if is_permission_denied(raw) {
        "This action requires Config-tier permission. Ask an operator to grant it in \
         Settings → Security, or re-pair selecting Config."
            .to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_operator_only_method_denial() {
        assert!(is_permission_denied(
            "Operator privileges required for this method"
        ));
    }

    #[test]
    fn detects_tool_permission_denied() {
        assert!(is_permission_denied("tool error: PermissionDenied"));
        assert!(is_permission_denied("Permission denied"));
    }

    #[test]
    fn passes_through_unrelated_errors() {
        assert!(!is_permission_denied("connection timeout"));
        assert_eq!(friendly_error("connection timeout"), "connection timeout");
    }

    #[test]
    fn friendly_error_rewrites_denial() {
        let out = friendly_error("Operator privileges required for this method");
        assert!(out.contains("Config-tier permission"));
        assert!(out.contains("Settings → Security"));
    }
}
