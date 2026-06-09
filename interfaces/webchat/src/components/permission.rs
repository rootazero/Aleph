//! 权限分层 UI:配置页闸门 (ConfigGate)、全局身份横幅 (PermissionBanner)、
//! 以及 RPC 权限拒绝错误的友好映射。复用 DashboardState::is_operator()
//! —— 后端 2 层 tier 在前端的诚实投影。后端零改动。

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
