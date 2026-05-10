use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::json;

use crate::extension::manifest::adapter::AdapterRegistry;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::types::*;

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
    let dest_path = plugins_dir.join(repo_name);

    if dest_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin already exists at: {}", dest_path.display()),
        );
    }

    match git2::Repository::clone(&params.url, &dest_path) {
        Ok(_) => {
            // Validate the installed plugin via AdapterRegistry
            let registry = AdapterRegistry::with_defaults();
            match registry.parse_dir(&dest_path) {
                Ok(output) => {
                    if let Ok(manager) = get_extension_manager() {
                        if let Err(e) = manager.reload().await {
                            tracing::warn!("Failed to refresh extensions after install: {}", e);
                        }
                    }
                    let info = PluginInfoJson {
                        name: output.name.unwrap_or_else(|| output.plugin_id.clone()),
                        version: output.version.unwrap_or_default(),
                        description: output.description.unwrap_or_default(),
                        enabled: true,
                        path: dest_path.to_string_lossy().to_string(),
                        skills_count: output.capabilities.len() as u32,
                        agents_count: 0,
                        hooks_count: 0,
                        mcp_servers_count: 0,
                    };
                    JsonRpcResponse::success(request.id, json!({ "plugin": info }))
                }
                Err(e) => {
                    // Cleanup on failure
                    let _ = std::fs::remove_dir_all(&dest_path);
                    JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to load installed plugin: {}", e),
                    )
                }
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clone repository: {}", e),
        ),
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
                format!("Invalid base64 data: {}", e),
            );
        }
    };

    // Extract and install
    let plugins_dir = crate::extension::default_plugins_dir();
    let temp_path = std::env::temp_dir().join(format!("aleph-plugin-{}.zip", uuid::Uuid::new_v4()));

    // Write temp file
    if let Err(e) = std::fs::write(&temp_path, &zip_data) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to write temp file: {}", e),
        );
    }

    // Extract zip
    let file = match std::fs::File::open(&temp_path) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open zip file: {}", e),
            );
        }
    };

    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read zip archive: {}", e),
            );
        }
    };

    if let Err(e) = archive.extract(&plugins_dir) {
        let _ = std::fs::remove_file(&temp_path);
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to extract zip: {}", e),
        );
    }

    let _ = std::fs::remove_file(&temp_path);

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
