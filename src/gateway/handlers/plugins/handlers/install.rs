use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::json;

use super::super::types::{InstallFromZipParams, InstallParams, PluginInfoJson};
use crate::extension::manifest::adapter::AdapterRegistry;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Install a plugin from Git repository
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Clone repository and install
    let plugins_dir = crate::extension::default_plugins_dir();

    // Use git2 to clone
    let repo_name = params
        .url
        .split('/')
        .next_back()
        .unwrap_or("plugin")
        .trim_end_matches(".git");
    // The repo name becomes a directory under plugins_dir (and is
    // remove_dir_all'd on validation failure) — reject anything that is not
    // a single normal path component (e.g. "", "..", absolute paths).
    {
        use std::path::Component;
        let mut comps = std::path::Path::new(repo_name).components();
        if !matches!(
            (comps.next(), comps.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot derive a safe plugin directory name from URL: {}",
                    params.url
                ),
            );
        }
    }
    let dest_path = plugins_dir.join(repo_name);

    if let Err(reason) =
        crate::extension::ensure_plugin_destination_is_safe(&plugins_dir, &dest_path)
    {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, reason);
    }

    if dest_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin already exists at: {}", dest_path.display()),
        );
    }

    match git2::Repository::clone(&params.url, &dest_path) {
        Ok(_) => {
            // Catch clones whose destination resolved outside the
            // authoritative plugins root (e.g. via a symlinked parent that
            // was missed at install time).
            if let Err(reason) =
                crate::extension::ensure_plugin_root_within_authoritative(&plugins_dir, &dest_path)
            {
                let _ = std::fs::remove_dir_all(&dest_path);
                return JsonRpcResponse::error(request.id, INTERNAL_ERROR, reason);
            }
            // Validate the installed plugin via AdapterRegistry
            let registry = AdapterRegistry::with_defaults();
            match registry.parse_dir(&dest_path) {
                Ok(output) => {
                    if let Ok(manager) = get_extension_manager() {
                        if let Err(e) = manager.reload().await {
                            tracing::warn!("Failed to refresh extensions after install: {}", e);
                        }
                    }
                    // Count declared capabilities by kind so the install
                    // response mirrors what `plugins.list` reports (the old
                    // code reported every capability as a "skill").
                    let count_kind = |kind: &str| -> u32 {
                        output
                            .capabilities
                            .iter()
                            .filter(|c| c.kind_name() == kind)
                            .count() as u32
                    };
                    let info = PluginInfoJson {
                        name: output.name.unwrap_or_else(|| output.plugin_id.clone()),
                        version: output.version.unwrap_or_default(),
                        description: output.description.unwrap_or_default(),
                        enabled: true,
                        path: dest_path.to_string_lossy().to_string(),
                        skills_count: count_kind("skill"),
                        commands_count: count_kind("command"),
                        agents_count: count_kind("agent"),
                        hooks_count: count_kind("hook"),
                        mcp_servers_count: count_kind("mcp_server"),
                        status: "loaded".to_string(),
                        error: None,
                    };
                    JsonRpcResponse::success(request.id, json!({ "plugin": info }))
                }
                Err(e) => {
                    // Cleanup on failure
                    let _ = std::fs::remove_dir_all(&dest_path);
                    JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to load installed plugin: {e}"),
                    )
                }
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clone repository: {e}"),
        ),
    }
}

/// How the daemon should install a raw `source` string. This is the R4-owned
/// classification that used to live in the CLI shell: a bare name is a
/// marketplace lookup; anything carrying a path/host/scheme separator is a
/// direct git source.
#[derive(Debug, PartialEq, Eq)]
pub enum PluginSourceKind {
    Marketplace,
    GitUrl,
}

/// Classify a plugin source. Mirrors the retired CLI heuristic verbatim:
/// only a bare identifier (no `/`, `.`, or `:`) routes to the marketplace.
pub fn classify_plugin_source(source: &str) -> PluginSourceKind {
    let bare = !source.contains('/') && !source.contains('.') && !source.contains(':');
    if bare {
        PluginSourceKind::Marketplace
    } else {
        PluginSourceKind::GitUrl
    }
}

/// Resolve the install source string from request params, accepting the new
/// `source` key and falling back to the legacy `url` key so the pre-existing
/// `plugin.install` (which took `{url}`) stays backward-compatible.
fn install_source(params: Option<&serde_json::Value>) -> Option<String> {
    params
        .and_then(|p| p.get("source").or_else(|| p.get("url")))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Unified `plugin.install` entry: classify `source` server-side and dispatch
/// to the marketplace or git-clone installer. Keeps the shell a pure forwarder
/// (R4). Local `.zip` / `github:` sources stay client-side (they need local
/// file / GitHub I/O) and continue to use `plugins.installFromZip`.
pub async fn handle_install_unified(request: JsonRpcRequest) -> JsonRpcResponse {
    let source = install_source(request.params.as_ref());
    let Some(source) = source else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing source");
    };
    let scope = request
        .params
        .as_ref()
        .and_then(|p| p.get("scope"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    match classify_plugin_source(&source) {
        PluginSourceKind::Marketplace => {
            let sub = JsonRpcRequest {
                jsonrpc: request.jsonrpc.clone(),
                method: "plugin.marketplace.install".to_string(),
                params: Some(json!({ "name": source, "scope": scope })),
                id: request.id.clone(),
            };
            super::marketplace::handle_marketplace_install(sub).await
        }
        PluginSourceKind::GitUrl => {
            let sub = JsonRpcRequest {
                jsonrpc: request.jsonrpc.clone(),
                method: "plugins.install".to_string(),
                params: Some(json!({ "url": source })),
                id: request.id.clone(),
            };
            handle_install(sub).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bare_name_is_marketplace() {
        assert_eq!(classify_plugin_source("hello-world"), PluginSourceKind::Marketplace);
        assert_eq!(classify_plugin_source("my_plugin"), PluginSourceKind::Marketplace);
    }

    #[test]
    fn classify_urls_and_paths_are_git() {
        assert_eq!(
            classify_plugin_source("https://github.com/x/y"),
            PluginSourceKind::GitUrl
        );
        assert_eq!(classify_plugin_source("owner/repo"), PluginSourceKind::GitUrl);
        assert_eq!(classify_plugin_source("git@github.com:x/y.git"), PluginSourceKind::GitUrl);
        assert_eq!(classify_plugin_source("./local.thing"), PluginSourceKind::GitUrl);
    }

    #[test]
    fn install_source_prefers_source_then_url() {
        use serde_json::json;
        assert_eq!(install_source(Some(&json!({"source":"a","url":"b"}))).as_deref(), Some("a"));
        assert_eq!(install_source(Some(&json!({"url":"https://x/y"}))).as_deref(), Some("https://x/y"));
        assert_eq!(install_source(Some(&json!({}))), None);
        assert_eq!(install_source(None), None);
    }
}

/// Install plugins from a zip file
pub async fn handle_install_from_zip(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallFromZipParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Decode base64
    let zip_data = match BASE64.decode(&params.data) {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 data: {e}"),
            );
        }
    };

    // Extract and install
    let plugins_dir = crate::extension::default_plugins_dir();
    let temp_path = std::env::temp_dir().join(format!("aleph-plugin-{}.zip", uuid::Uuid::new_v4()));

    // Write temp file
    if let Err(e) = tokio::fs::write(&temp_path, &zip_data).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to write temp file: {e}"),
        );
    }

    // Extract zip
    let zip_bytes = match tokio::fs::read(&temp_path).await {
        Ok(b) => b,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open zip file: {e}"),
            );
        }
    };
    let cursor = std::io::Cursor::new(zip_bytes);

    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read zip archive: {e}"),
            );
        }
    };

    if let Err(e) = archive.extract(&plugins_dir) {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to extract zip: {e}"),
        );
    }

    let _ = tokio::fs::remove_file(&temp_path).await;

    // Return list of installed plugin names
    // For simplicity, return empty list - caller should use plugins.list to refresh
    if let Ok(manager) = get_extension_manager() {
        if let Err(e) = manager.reload().await {
            tracing::warn!("Failed to refresh extensions after zip install: {}", e);
        }
    }
    JsonRpcResponse::success(
        request.id,
        json!({ "installedNames": Vec::<String>::new() }),
    )
}
