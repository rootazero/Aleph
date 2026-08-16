//! Markdown Skills RPC Handlers
//!
//! Handler for runtime Markdown skill installation from Git URLs or local paths.
//!
//! Phase 1 deprecation: this handler still reads `AlephSkillSpec` fields via
//! `tool.spec.*`. The RPC payload shape that exposes those fields will
//! migrate to the unified `domain::skill::SkillManifest` during Phase 2.
//! See docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md.
#![allow(deprecated)]

use crate::sync_primitives::Arc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::skill::{install_allowed, scan_content, scan_skill_directory, ThreatLevel, TrustLevel};
use crate::tools::markdown_skill::{load_skills_from_dir, MarkdownCliTool};
use crate::tools::AlephToolServer;

/// Process-wide markdown-skill tool server. `AlephToolServer` is already
/// internally `Arc<RwLock<..>>`, so no outer lock is needed.
static MARKDOWN_SKILLS_SERVER: Lazy<AlephToolServer> = Lazy::new(AlephToolServer::new);

// Track loaded skill paths for reload
static SKILL_PATHS: Lazy<Arc<RwLock<std::collections::HashMap<String, PathBuf>>>> =
    Lazy::new(|| Arc::new(RwLock::new(std::collections::HashMap::new())));

/// Accessor for the shared markdown-skill server.
#[must_use]
pub fn markdown_skills_server() -> &'static AlephToolServer {
    &MARKDOWN_SKILLS_SERVER
}

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
            source_path: None, // Set by caller
            sandbox_mode,
            requires_bins: tool.spec.metadata.requires.bins.clone(),
        }
    }
}

// ============================================================================
// Install (from Git URL or local path)
// ============================================================================

/// Parameters for `skills.install`
#[derive(Debug, Deserialize)]
pub struct InstallParams {
    /// Git URL or local path to install skills from
    pub url: String,
    /// If true, clone repo contents directly into `skills_dir` (no subdirectory)
    #[serde(default)]
    pub flatten: bool,
}

/// Default skills directory (`<config_dir>/skills`).
///
/// Must be the same resolution `paths::get_skills_dir()` gives, because that
/// is what the skill loader reads — this was the third hand-rolled answer for
/// one directory, and the two that ignored `ALEPH_HOME` installed skills where
/// nothing looked for them.
fn default_skills_dir() -> PathBuf {
    crate::utils::paths::get_skills_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".aleph").join("skills"))
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
                    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
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
            .map_err(|e| format!("Failed to create directory: {e}"))?;
        info!(url = %source, dest = %clone_path.display(), "Cloning skills repository");
        git2::Repository::clone(source, &clone_path)
            .map_err(|e| format!("Failed to clone repository: {e}"))?;
    }

    if flatten {
        // Copy contents (excluding .git and .git-cache) into skills_dir
        copy_dir_contents(&clone_path, skills_dir)
            .map_err(|e| format!("Failed to copy skills: {e}"))?;
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

/// Download and extract a ZIP file into `skills_dir`/<archive-name>/
async fn install_from_zip(source: &str, skills_dir: &std::path::Path) -> Result<PathBuf, String> {
    let zip_data = if source.starts_with("http://") || source.starts_with("https://") {
        // Download ZIP from URL
        info!(url = %source, "Downloading skills ZIP archive");
        let resp = reqwest::get(source)
            .await
            .map_err(|e| format!("Failed to download ZIP: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("Download failed with status: {}", resp.status()));
        }
        resp.bytes()
            .await
            .map_err(|e| format!("Failed to read ZIP data: {e}"))?
            .to_vec()
    } else {
        // Read local ZIP file
        info!(path = %source, "Reading local ZIP archive");
        tokio::fs::read(source)
            .await
            .map_err(|e| format!("Failed to read ZIP file: {e}"))?
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
        zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP archive: {e}"))?;

    // Clean existing and extract fresh
    if dest_path.exists() {
        let _ = std::fs::remove_dir_all(&dest_path);
    }
    tokio::fs::create_dir_all(&dest_path)
        .await
        .map_err(|e| format!("Failed to create directory: {e}"))?;

    archive
        .extract(&dest_path)
        .map_err(|e| format!("Failed to extract ZIP: {e}"))?;

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
    if let Err(e) = tokio::fs::create_dir_all(&skills_dir).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create skills directory: {e}"),
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
        SourceType::Git => {
            // git2 clone/fetch is blocking network I/O — keep it off the
            // async executor (a slow clone can take minutes).
            let src = source.clone();
            let dir = skills_dir.clone();
            let flatten = params.flatten;
            match tokio::task::spawn_blocking(move || install_from_git(&src, &dir, flatten)).await {
                Ok(Ok(path)) => path,
                Ok(Err(e)) => {
                    return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
                }
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("git install task failed: {e}"),
                    );
                }
            }
        }
        SourceType::LocalPath => PathBuf::from(&source),
    };

    // Security gate: scan all files in the installed directory before registering tools.
    // Markdown skills installed via this RPC are arbitrary third-party content → Community trust.
    //
    // The gate must run for *every* source type, not just directories: `detect_source_type`
    // routes anything that isn't a zip / http(s) URL / git URL through `LocalPath`, which
    // can be either a directory or a single `*.skill.md` file. Without the single-file
    // arm, `skills.install /tmp/downloaded.skill.md` would skip the scan entirely and
    // the loader would still pick the file up via `WalkDir`.
    let verdict = if load_path.is_dir() {
        scan_skill_directory(&load_path)
    } else {
        let file_name = load_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skill.md");
        match std::fs::read(&load_path) {
            Ok(bytes) => scan_content(file_name, &bytes),
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("cannot read skill source for security scan: {e}"),
                );
            }
        }
    };
    if !install_allowed(verdict.level, TrustLevel::Community) {
        // Reject: clean up and return an error before any tool is registered.
        let _ = std::fs::remove_dir_all(&load_path);
        let ids: Vec<&str> = verdict.findings.iter().map(|f| f.pattern_id).collect();
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "skill bundle blocked by security scan ({:?}): {}",
                verdict.level,
                ids.join(", ")
            ),
        );
    }
    if matches!(verdict.level, ThreatLevel::Caution) {
        warn!(
            path = %load_path.display(),
            "markdown skill bundle has caution-level security findings; proceeding"
        );
    }

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
    let server = markdown_skills_server();
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
