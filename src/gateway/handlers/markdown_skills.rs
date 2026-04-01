//! Markdown Skills RPC Handlers
//!
//! Handlers for runtime Markdown skill management:
//! - Load skills from directories
//! - Reload specific skills
//! - List loaded skills
//! - Unload skills

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::tools::markdown_skill::{load_skills_from_dir, MarkdownCliTool};
use crate::tools::AlephToolServer;

// Global ToolServer for Markdown skills
// This is shared across all RPC calls
static MARKDOWN_SKILLS_SERVER: Lazy<Arc<RwLock<AlephToolServer>>> =
    Lazy::new(|| Arc::new(RwLock::new(AlephToolServer::new())));

// Track loaded skill paths for reload
static SKILL_PATHS: Lazy<Arc<RwLock<std::collections::HashMap<String, PathBuf>>>> =
    Lazy::new(|| Arc::new(RwLock::new(std::collections::HashMap::new())));

/// Markdown skill info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct MarkdownSkillInfo {
    pub name: String,
    pub description: String,
    pub source_path: Option<String>,
    pub sandbox_mode: String,
    pub requires_bins: Vec<String>,
}

impl From<&MarkdownCliTool> for MarkdownSkillInfo {
    fn from(tool: &MarkdownCliTool) -> Self {
        let sandbox_mode = match tool.spec.metadata.aleph.as_ref() {
            Some(aleph_meta) => match &aleph_meta.security.sandbox {
                crate::tools::markdown_skill::SandboxMode::Host => "host".to_string(),
                crate::tools::markdown_skill::SandboxMode::Docker => "docker".to_string(),
                crate::tools::markdown_skill::SandboxMode::VirtualFs => "virtualfs".to_string(),
            },
            None => "host".to_string(),
        };

        Self {
            name: tool.spec.name.clone(),
            description: tool.spec.description.clone(),
            source_path: None,  // Set by caller
            sandbox_mode,
            requires_bins: tool.spec.metadata.requires.bins.clone(),
        }
    }
}

// ============================================================================
// Install (from Git URL or local path)
// ============================================================================

/// Parameters for markdown_skills.install
#[derive(Debug, Deserialize)]
pub struct InstallParams {
    /// Git URL or local path to install skills from
    pub url: String,
    /// If true, clone repo contents directly into skills_dir (no subdirectory)
    #[serde(default)]
    pub flatten: bool,
}

/// Default skills directory (~/.aleph/skills)
fn default_skills_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph")
        .join("skills")
}

/// Detect source type from URL/path string
enum SourceType {
    Git,
    Zip,
    LocalPath,
}

fn detect_source_type(source: &str) -> SourceType {
    let lower = source.to_lowercase();
    if lower.ends_with(".zip") {
        SourceType::Zip
    } else if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.ends_with(".git")
    {
        SourceType::Git
    } else {
        SourceType::LocalPath
    }
}

/// Clone or pull a git repository.
///
/// If `flatten` is false, clone into `skills_dir/<repo-name>/`.
/// If `flatten` is true, clone to a hidden cache dir and copy contents
/// (excluding .git) directly into `skills_dir/`.
fn install_from_git(
    source: &str,
    skills_dir: &std::path::Path,
    flatten: bool,
) -> Result<PathBuf, String> {
    let repo_name = source
        .split('/')
        .next_back()
        .unwrap_or("skills")
        .trim_end_matches(".git");

    // Where to clone/pull the git repo
    let clone_path = if flatten {
        skills_dir.join(".git-cache").join(repo_name)
    } else {
        skills_dir.join(repo_name)
    };

    if clone_path.exists() {
        info!(path = %clone_path.display(), "Skills repo exists, pulling updates");
        match git2::Repository::open(&clone_path) {
            Ok(repo) => {
                if let Err(e) = (|| -> Result<(), git2::Error> {
                    let mut remote = repo.find_remote("origin")?;
                    remote.fetch(&["HEAD"], None, None)?;
                    let fetch_head = repo.find_reference("FETCH_HEAD")?;
                    let commit = repo.reference_to_annotated_commit(&fetch_head)?;
                    let refname = "refs/heads/main";
                    if let Ok(mut reference) = repo.find_reference(refname) {
                        reference.set_target(commit.id(), "pull")?;
                    }
                    repo.set_head(refname)?;
                    repo.checkout_head(Some(
                        git2::build::CheckoutBuilder::default().force(),
                    ))?;
                    Ok(())
                })() {
                    warn!(error = %e, "Git pull failed, using existing directory");
                }
            }
            Err(e) => {
                warn!(error = %e, "Not a git repo, using existing directory");
            }
        }
    } else {
        std::fs::create_dir_all(clone_path.parent().unwrap_or(skills_dir))
            .map_err(|e| format!("Failed to create directory: {}", e))?;
        info!(url = %source, dest = %clone_path.display(), "Cloning skills repository");
        git2::Repository::clone(source, &clone_path)
            .map_err(|e| format!("Failed to clone repository: {}", e))?;
    }

    if flatten {
        // Copy contents (excluding .git and .git-cache) into skills_dir
        copy_dir_contents(&clone_path, skills_dir)
            .map_err(|e| format!("Failed to copy skills: {}", e))?;
        Ok(skills_dir.to_path_buf())
    } else {
        Ok(clone_path)
    }
}

/// Recursively copy directory contents, skipping .git and .git-cache directories
fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == ".git-cache" || name_str == ".gitignore" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Download and extract a ZIP file into skills_dir/<archive-name>/
async fn install_from_zip(source: &str, skills_dir: &std::path::Path) -> Result<PathBuf, String> {
    let zip_data = if source.starts_with("http://") || source.starts_with("https://") {
        // Download ZIP from URL
        info!(url = %source, "Downloading skills ZIP archive");
        let resp = reqwest::get(source)
            .await
            .map_err(|e| format!("Failed to download ZIP: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("Download failed with status: {}", resp.status()));
        }
        resp.bytes()
            .await
            .map_err(|e| format!("Failed to read ZIP data: {}", e))?
            .to_vec()
    } else {
        // Read local ZIP file
        info!(path = %source, "Reading local ZIP archive");
        std::fs::read(source).map_err(|e| format!("Failed to read ZIP file: {}", e))?
    };

    // Derive folder name from filename (strip .zip)
    let file_stem = std::path::Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("skills-zip");
    let dest_path = skills_dir.join(file_stem);

    // Extract ZIP
    let cursor = std::io::Cursor::new(zip_data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP archive: {}", e))?;

    // Clean existing and extract fresh
    if dest_path.exists() {
        let _ = std::fs::remove_dir_all(&dest_path);
    }
    std::fs::create_dir_all(&dest_path)
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    archive
        .extract(&dest_path)
        .map_err(|e| format!("Failed to extract ZIP: {}", e))?;

    info!(dest = %dest_path.display(), entries = archive.len(), "Extracted ZIP archive");
    Ok(dest_path)
}

/// Install skills from a Git URL, ZIP archive, or local path, then load them
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let source = params.url.trim().to_string();

    // Ensure skills directory exists
    let skills_dir = default_skills_dir();
    if let Err(e) = std::fs::create_dir_all(&skills_dir) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create skills directory: {}", e),
        );
    }

    // Determine the actual path to load skills from
    let load_path = match detect_source_type(&source) {
        SourceType::Zip => match install_from_zip(&source, &skills_dir).await {
            Ok(path) => path,
            Err(e) => {
                return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
            }
        },
        SourceType::Git => match install_from_git(&source, &skills_dir, params.flatten) {
            Ok(path) => path,
            Err(e) => {
                return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
            }
        },
        SourceType::LocalPath => PathBuf::from(&source),
    };

    // Load skills from the directory
    let tools = load_skills_from_dir(load_path.clone()).await;

    if tools.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("No skills found in {}", load_path.display()),
        );
    }

    // Add tools to server and track paths
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let mut paths = SKILL_PATHS.write().await;
    let mut loaded_skills = Vec::new();

    for tool in tools {
        let tool_name = tool.spec.name.clone();
        let skill_info = MarkdownSkillInfo {
            source_path: Some(load_path.to_string_lossy().to_string()),
            ..MarkdownSkillInfo::from(&tool)
        };

        let update_info = server.replace_tool(tool).await;

        info!(
            name = %tool_name,
            was_replaced = update_info.was_replaced,
            "Installed Markdown skill"
        );

        paths.insert(tool_name, load_path.clone());
        loaded_skills.push(skill_info);
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": loaded_skills,
            "count": loaded_skills.len()
        }),
    )
}

// ============================================================================
// Load
// ============================================================================

/// Parameters for markdown_skills.load
#[derive(Debug, Deserialize)]
pub struct LoadParams {
    /// Path to directory containing SKILL.md files
    pub path: String,
}

/// Load Markdown skills from a directory
pub async fn handle_load(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LoadParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let path = PathBuf::from(&params.path);

    // Load skills from directory
    let tools = load_skills_from_dir(path.clone()).await;

    if tools.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "No skills found in directory".to_string(),
        );
    }

    // Add tools to server and track paths
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let mut paths = SKILL_PATHS.write().await;
    let mut loaded_skills = Vec::new();

    for tool in tools {
        let tool_name = tool.spec.name.clone();
        let skill_info = MarkdownSkillInfo {
            source_path: Some(params.path.clone()),
            ..MarkdownSkillInfo::from(&tool)
        };

        let update_info = server.replace_tool(tool).await;

        info!(
            name = %tool_name,
            was_replaced = update_info.was_replaced,
            "Loaded Markdown skill"
        );

        paths.insert(tool_name, path.clone());
        loaded_skills.push(skill_info);
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": loaded_skills,
            "count": loaded_skills.len()
        }),
    )
}

// ============================================================================
// Reload
// ============================================================================

/// Parameters for markdown_skills.reload
#[derive(Debug, Deserialize)]
pub struct ReloadParams {
    /// Skill name to reload
    pub name: String,
}

/// Reload a specific Markdown skill
pub async fn handle_reload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ReloadParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get skill path
    let paths = SKILL_PATHS.read().await;
    let path = match paths.get(&params.name) {
        Some(p) => p.clone(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Skill '{}' not found in registry", params.name),
            );
        }
    };
    drop(paths);

    // Reload from path
    let tools = load_skills_from_dir(path).await;
    let tool = match tools.into_iter().find(|t| t.spec.name == params.name) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Skill '{}' not found after reload", params.name),
            );
        }
    };

    // Replace in server
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let update_info = server.replace_tool(tool.clone()).await;

    info!(
        name = %params.name,
        was_replaced = update_info.was_replaced,
        "Reloaded Markdown skill"
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "skill": MarkdownSkillInfo::from(&tool),
            "was_replaced": update_info.was_replaced
        }),
    )
}

// ============================================================================
// List
// ============================================================================

/// List all loaded Markdown skills
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let paths = SKILL_PATHS.read().await;

    let mut skills = Vec::new();

    for name in server.list_names().await {
        // Check if this is a Markdown skill (in our registry)
        if let Some(path) = paths.get(&name) {
            if let Some(def) = server.get_definition(&name).await {
                skills.push(json!({
                    "name": name,
                    "description": def.description,
                    "source_path": path.to_string_lossy(),
                }));
            }
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": skills,
            "count": skills.len()
        }),
    )
}

// ============================================================================
// Unload
// ============================================================================

/// Parameters for markdown_skills.unload
#[derive(Debug, Deserialize)]
pub struct UnloadParams {
    /// Skill name to unload
    pub name: String,
}

/// Unload a Markdown skill
pub async fn handle_unload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UnloadParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Remove from server
    let server = MARKDOWN_SKILLS_SERVER.read().await;
    let removed = server.remove_tool(&params.name).await;

    if !removed {
        warn!(name = %params.name, "Attempted to unload non-existent skill");
    }

    // Remove from path registry
    let mut paths = SKILL_PATHS.write().await;
    paths.remove(&params.name);

    info!(name = %params.name, removed = removed, "Unloaded Markdown skill");

    JsonRpcResponse::success(
        request.id,
        json!({
            "removed": removed
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_params() {
        let json = json!({"path": "/path/to/skills"});
        let params: LoadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.path, "/path/to/skills");
    }

    #[test]
    fn test_reload_params() {
        let json = json!({"name": "my-skill"});
        let params: ReloadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "my-skill");
    }

    #[test]
    fn test_unload_params() {
        let json = json!({"name": "my-skill"});
        let params: UnloadParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "my-skill");
    }
}
