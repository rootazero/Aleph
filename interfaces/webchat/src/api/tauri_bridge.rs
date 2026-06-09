//! WASM → Tauri v2 invoke 桥(panel 唯一的 shell-command 出口)。
//! 仅在桌面 Tauri shell 内可用(withGlobalTauri=true 暴露
//! window.__TAURI__.core.invoke);纯浏览器内 is_shell()=false,调用方需降级。

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// 是否运行在桌面 Tauri shell 内。
pub fn is_shell() -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    match js_sys::Reflect::get(&win, &JsValue::from_str("__TAURI__")) {
        Ok(v) => !v.is_undefined() && !v.is_null(),
        Err(_) => false,
    }
}

fn js_err(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// 当前连接目标:"local" 或远端 origin URL。
pub async fn get_connection_target() -> Result<String, String> {
    let v = tauri_invoke("get_connection_target", JsValue::NULL)
        .await
        .map_err(js_err)?;
    Ok(v.as_string().unwrap_or_default())
}

/// 切换 shell 连接目标。`raw` 接受 "local" / "host" / "host:port" /
/// "http(s)://host[:port]"。成功后 shell 会 reroute webview(本视图随之销毁)。
pub async fn set_connection_target(raw: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("raw"), &JsValue::from_str(raw))
        .map_err(js_err)?;
    tauri_invoke("set_connection_target", args.into())
        .await
        .map_err(js_err)?;
    Ok(())
}

/// 仅供 UI 预览/即时提示的 endpoint 归一,镜像 shell 端
/// `ConnectionTarget::parse` 的显示形态(补 http scheme + 默认端口 18790)。
/// 权威解析由 shell 的 `set_connection_target` 完成;此处只为预览,
/// IPv6 等边角由权威解析兜底。
pub fn normalize_endpoint_preview(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("local") {
        return "local".to_string();
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    let (scheme, after) = match with_scheme.split_once("://") {
        Some(p) => p,
        None => return with_scheme,
    };
    let (host, path) = match after.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (after.to_string(), String::new()),
    };
    if host.contains(':') {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{host}:18790{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_local_map_to_local() {
        assert_eq!(normalize_endpoint_preview(""), "local");
        assert_eq!(normalize_endpoint_preview("  "), "local");
        assert_eq!(normalize_endpoint_preview("LOCAL"), "local");
    }

    #[test]
    fn bare_host_gets_http_and_default_port() {
        assert_eq!(normalize_endpoint_preview("core.example"), "http://core.example:18790");
    }

    #[test]
    fn explicit_port_preserved() {
        assert_eq!(normalize_endpoint_preview("core.example:9000"), "http://core.example:9000");
    }

    #[test]
    fn https_scheme_preserved_and_port_added() {
        assert_eq!(normalize_endpoint_preview("https://core.example"), "https://core.example:18790");
    }

    #[test]
    fn https_with_port_unchanged() {
        assert_eq!(normalize_endpoint_preview("https://core.example:443"), "https://core.example:443");
    }
}
