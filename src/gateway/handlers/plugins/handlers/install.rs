use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::json;

use aleph_protocol::plugins::{PluginRow, PluginRuntimeStatus};

use super::super::access::current_plugin_access;
use super::super::types::{InstallFromZipParams, InstallParams, plugin_row};
use crate::extension::PluginScope;
use crate::extension::marketplace::installer::{
    copy_dir_recursive, new_install_staging, publish_install,
};
use crate::extension::plugin_target::PluginTarget;
use crate::extension::validation::validate_install_candidate;
use crate::extension::visibility::{PluginInstanceKey, ScopeKey, scope_install_dir};
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse};

fn install_destination(scope: Option<&str>) -> Result<(ScopeKey, PathBuf), String> {
    let scope = match scope {
        None | Some("user") => PluginScope::User,
        Some("project") => PluginScope::Project,
        Some("local") => PluginScope::Local,
        Some(_) => return Err("Unknown plugin install scope".to_owned()),
    };
    let access = current_plugin_access();
    let key = access
        .install_scope(scope)
        .map_err(|error| error.to_string())?;
    let path = scope_install_dir(scope, access.project_root())?;
    Ok((key, path))
}

fn check_project_install_root(scope: &ScopeKey, path: &Path) -> Result<(), String> {
    if let ScopeKey::Project(project) = scope {
        crate::extension::ensure_plugin_destination_is_safe(project, path)?;
    }
    Ok(())
}

/// Install a plugin from Git repository
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let (scope, plugins_dir) = match install_destination(params.scope.as_deref()) {
        Ok(target) => target,
        Err(error) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, error),
    };
    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(error) => return error.with_id(request.id),
    };

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

    // The clone reaches the network and the manifest scan reads the
    // filesystem — both block, so they run off the async runtime's threads.
    // Every failure below was an `INTERNAL_ERROR` already, so the error
    // channel is unchanged.
    let url = params.url;
    let dest = dest_path.clone();
    let plugins = plugins_dir.clone();
    let stage_scope = scope.clone();
    let installed = crate::skill::run_blocking_fs("plugins.install", move || {
        check_project_install_root(&stage_scope, &plugins)?;
        let staging = new_install_staging(&plugins)?;
        let candidate = staging
            .path()
            .join(dest.file_name().expect("validated plugin name"));
        let local = Path::new(&url);
        if local.is_dir() && git2::Repository::open(local).is_err() {
            copy_dir_recursive(local, &candidate)?;
        } else {
            git2::Repository::clone(&url, &candidate)
                .map_err(|e| format!("Failed to clone repository: {e}"))?;
        }
        let output = validate_install_candidate(&candidate)?;
        Ok::<_, String>((staging, candidate, output))
    })
    .await;

    let (staging, candidate, output) = match installed {
        Some(Ok(prepared)) => prepared,
        Some(Err(reason)) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, reason),
        None => {
            return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Plugin install cancelled");
        }
    };

    let instance = PluginInstanceKey::new(scope.clone(), output.plugin_id.clone());
    let selector = instance.selector();
    let installed_path = dest_path.clone();
    let commit_scope = scope.clone();
    let result = manager
        .publish_plugin_install(std::slice::from_ref(&instance), move || {
            let _staging = staging;
            check_project_install_root(&commit_scope, &plugins_dir)?;
            publish_install(&plugins_dir, &candidate, &installed_path)
        })
        .await;
    if let Err(error) = result {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, error);
    }
    if let Ok((record, _)) = manager
        .plugin_target_snapshot(
            &PluginTarget::Exact(selector.clone()),
            &current_plugin_access(),
        )
        .await
    {
        return JsonRpcResponse::success(
            request.id,
            json!({"plugin": plugin_row(crate::extension::PluginInfo::from(&record))}),
        );
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
    let kind = output
        .runtime_manifest
        .as_ref()
        .map(|manifest| manifest.kind)
        .unwrap_or(crate::extension::PluginKind::Static);
    let info = PluginRow {
        selector: Some(selector),
        scope_kind: Some(scope.kind_label().to_owned()),
        name: output.name.unwrap_or_else(|| output.plugin_id.clone()),
        version: output.version.unwrap_or_default(),
        description: output.description.unwrap_or_default(),
        enabled: false,
        path: dest_path.to_string_lossy().to_string(),
        kind: kind.as_str().to_string(),
        origin: match scope {
            ScopeKey::Global => crate::extension::PluginOrigin::Global,
            ScopeKey::Project(_) => crate::extension::PluginOrigin::Workspace,
        }
        .label()
        .to_owned(),
        status: PluginRuntimeStatus::Pending,
        status_detail: Some("Installed on disk. Runtime discovery is pending.".to_owned()),
        skills_count: count_kind("skill"),
        commands_count: count_kind("command"),
        agents_count: count_kind("agent"),
        hooks_count: count_kind("hook"),
        mcp_servers_count: count_kind("mcp_server"),
        tools_count: count_kind("tool"),
        // A plugin installed one millisecond ago has no usage
        // record and never will have one yet. `None` here says
        // "unknown", which is the truth; `Some(calls: 0)` would
        // say "known never used" about something that has not
        // had the chance.
        usage: None,
    };
    JsonRpcResponse::success(request.id, json!({ "plugin": info }))
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
                params: Some(json!({ "url": source, "scope": scope })),
                id: request.id.clone(),
            };
            handle_install(sub).await
        }
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

    let (scope, plugins_dir) = match install_destination(params.scope.as_deref()) {
        Ok(target) => target,
        Err(error) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, error),
    };
    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(error) => return error.with_id(request.id),
    };
    let stage_root = plugins_dir.clone();
    let stage_scope = scope.clone();
    let prepared = crate::skill::run_blocking_fs("plugins.installFromZip", move || {
        check_project_install_root(&stage_scope, &stage_root)?;
        prepare_zip(&stage_root, zip_data)
    })
    .await;
    let prepared = match prepared {
        Some(Ok(prepared)) => prepared,
        Some(Err(reason)) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, reason),
        None => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Zip install cancelled"),
    };
    let instances: Vec<_> = prepared
        .names
        .iter()
        .map(|name| PluginInstanceKey::new(scope.clone(), name.clone()))
        .collect();
    let installed = manager
        .publish_plugin_install(&instances, move || {
            check_project_install_root(&scope, &plugins_dir)?;
            prepared.publish(&plugins_dir)
        })
        .await;
    match installed {
        Ok(names) => JsonRpcResponse::success(
            request.id,
            json!({
                "installedNames": names,
                "selectors": instances.iter().map(PluginInstanceKey::selector).collect::<Vec<_>>(),
            }),
        ),
        Err(error) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, error),
    }
}

struct PreparedZip {
    _staging: tempfile::TempDir,
    installs: Vec<(PathBuf, PathBuf)>,
    names: Vec<String>,
}

fn prepare_zip(plugins_dir: &Path, bytes: Vec<u8>) -> Result<PreparedZip, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Failed to read zip archive: {e}"))?;
    let staging = new_install_staging(plugins_dir)?;
    let unpacked = staging.path().join("plugin");
    archive
        .extract(&unpacked)
        .map_err(|e| format!("Failed to extract zip: {e}"))?;

    let mut installs = Vec::new();
    let mut names = Vec::new();
    if crate::discovery::has_plugin_manifest(&unpacked) {
        let output = validate_install_candidate(&unpacked)?;
        crate::extension::manifest::validate_plugin_id(&output.plugin_id)
            .map_err(|e| e.to_string())?;
        installs.push((unpacked, plugins_dir.join(&output.plugin_id)));
        names.push(output.plugin_id);
    } else {
        let mut entries = std::fs::read_dir(&unpacked)
            .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("Failed to read extracted zip: {e}"))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let leaf = entry.file_name();
            if leaf.to_string_lossy().starts_with('.')
                || !entry.file_type().map_err(|e| e.to_string())?.is_dir()
            {
                continue;
            }
            let folder = entry.path();
            let mut unreadable = Vec::new();
            let mut candidates = crate::discovery::plugin_candidates(&folder, &mut unreadable);
            if !unreadable.is_empty() {
                return Err(format!(
                    "Failed to read extracted directory: {}",
                    folder.display()
                ));
            }
            candidates.sort();
            for candidate in &candidates {
                names.push(validate_install_candidate(candidate)?.plugin_id);
            }
            if !candidates.is_empty() {
                installs.push((folder, plugins_dir.join(leaf)));
            }
        }
    }
    if installs.is_empty() {
        return Err("Zip archive does not contain a plugin manifest".to_owned());
    }

    Ok(PreparedZip {
        _staging: staging,
        installs,
        names,
    })
}

impl PreparedZip {
    fn publish(self, plugins_dir: &Path) -> Result<Vec<String>, String> {
        let mut published: Vec<&(PathBuf, PathBuf)> = Vec::new();
        for entry in &self.installs {
            let (candidate, dest) = entry;
            if let Err(error) = publish_install(plugins_dir, candidate, dest) {
                for (original, installed) in published.into_iter().rev() {
                    if let Err(rollback) = std::fs::rename(installed, original) {
                        return Err(format!(
                            "{error}. Could not roll back '{}': {rollback}",
                            installed.display()
                        ));
                    }
                }
                return Err(error);
            }
            published.push(entry);
        }
        Ok(self.names)
    }
}

#[cfg(test)]
fn install_zip(plugins_dir: &Path, bytes: Vec<u8>) -> Result<Vec<String>, String> {
    prepare_zip(plugins_dir, bytes)?.publish(plugins_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_plugins(plugins: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, manifest) in plugins {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn zip_install_validates_every_candidate_before_publishing() {
        let root = tempfile::tempdir().unwrap();
        let one = ("one/.claude-plugin/plugin.json", r#"{"name":"one"}"#);
        let bad = ("two/.claude-plugin/plugin.toml", "name = [");
        assert!(install_zip(root.path(), zip_plugins(&[one, bad])).is_err());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());

        let two = ("two/.claude-plugin/plugin.json", r#"{"name":"two"}"#);
        let archive = zip_plugins(&[one, two]);
        assert_eq!(
            install_zip(root.path(), archive.clone()).unwrap(),
            ["one", "two"]
        );
        assert!(install_zip(root.path(), archive).is_err());
        assert!(root.path().join(one.0).is_file());
        assert!(root.path().join(two.0).is_file());
    }

    #[test]
    fn classify_bare_name_is_marketplace() {
        assert_eq!(
            classify_plugin_source("hello-world"),
            PluginSourceKind::Marketplace
        );
        assert_eq!(
            classify_plugin_source("my_plugin"),
            PluginSourceKind::Marketplace
        );
    }

    #[test]
    fn classify_urls_and_paths_are_git() {
        assert_eq!(
            classify_plugin_source("https://github.com/x/y"),
            PluginSourceKind::GitUrl
        );
        assert_eq!(
            classify_plugin_source("owner/repo"),
            PluginSourceKind::GitUrl
        );
        assert_eq!(
            classify_plugin_source("git@github.com:x/y.git"),
            PluginSourceKind::GitUrl
        );
        assert_eq!(
            classify_plugin_source("./local.thing"),
            PluginSourceKind::GitUrl
        );
    }

    #[test]
    fn install_source_prefers_source_then_url() {
        use serde_json::json;
        assert_eq!(
            install_source(Some(&json!({"source":"a","url":"b"}))).as_deref(),
            Some("a")
        );
        assert_eq!(
            install_source(Some(&json!({"url":"https://x/y"}))).as_deref(),
            Some("https://x/y")
        );
        assert_eq!(install_source(Some(&json!({}))), None);
        assert_eq!(install_source(None), None);
    }
}
