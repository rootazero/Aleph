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
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::skill::{install_allowed, scan_content, scan_skill_directory, ThreatLevel, TrustLevel};
use crate::tools::markdown_skill::{
    load_skills_from_dir, markdown_skill_registry_owner, MarkdownCliTool,
};
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
/// If `flatten` is true, clone to a hidden cache dir under `skills_dir/`. The
/// caller copies the contents (excluding .git) into `skills_dir/` once the
/// security scan passed, so unvetted content never reaches the skills
/// directory.
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

    // The last URL segment becomes a directory name under the skills
    // directory, so it has to be a plain name: `..` would clone outside the
    // skills directory, `.` or an empty name would clone into it.
    if repo_name.is_empty()
        || repo_name == "."
        || repo_name == ".."
        || repo_name.contains('/')
        || repo_name.contains('\\')
    {
        return Err(format!("Invalid repository name in URL: {source}"));
    }

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

    // The clone stays where it is. A flattened install copies it into the
    // skills directory only after the security scan has passed, so unvetted
    // content never reaches the directory the boot watcher scans.
    Ok(clone_path)
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

    // Derive folder name from filename (strip .zip).
    // Reject any name that is empty, a dot-segment, or contains path separators,
    // or resolves to anything other than a single Normal component.  This prevents
    // an archive named `..zip` (stem "..") or `.zip` (stem ".") from escaping
    // the skills directory.
    let file_stem = std::path::Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("skills-zip");
    if file_stem.is_empty()
        || file_stem == "."
        || file_stem == ".."
        || file_stem.contains('/')
        || file_stem.contains('\\')
        || !matches!(
            Path::new(file_stem).components().next(),
            Some(std::path::Component::Normal(_))
        )
        || Path::new(file_stem).components().count() != 1
    {
        return Err(format!("Invalid archive name: {source}"));
    }
    let dest_path = skills_dir.join(file_stem);

    // Extra guard: the resolved destination must be a direct child of skills_dir.
    // This catches any edge-cases the name check above might not cover on the
    // current platform (e.g. UNC paths on Windows).
    if dest_path.parent() != Some(skills_dir) {
        return Err(format!("Invalid archive name: {source}"));
    }

    tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(zip_data);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP archive: {e}"))?;

        // Clean existing and extract fresh, off the async executor.
        if dest_path.exists() {
            let _ = std::fs::remove_dir_all(&dest_path);
        }
        std::fs::create_dir_all(&dest_path)
            .map_err(|e| format!("Failed to create directory: {e}"))?;

        archive
            .extract(&dest_path)
            .map_err(|e| format!("Failed to extract ZIP: {e}"))?;
        info!(dest = %dest_path.display(), entries = archive.len(), "Extracted ZIP archive");
        Ok(dest_path)
    })
    .await
    .map_err(|e| format!("zip install task failed: {e}"))?
}

/// Whether `path` lives inside `root`.
///
/// Component-wise, so a sibling that shares a name prefix (`/skills-old`
/// against `/skills`) does not count. Both sides are canonicalized first, so
/// a symlinked skills directory is compared by what it points at.
fn is_inside(path: &Path, root: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(root)
}

/// Where an install's content comes from, and what this call owns.
struct InstallPlan {
    /// The path the security scan runs on.
    scan: PathBuf,
    /// The path the skills are loaded from.
    load: PathBuf,
    /// The path this call created. Only an owned path may be removed when
    /// the scan rejects the bundle — a caller-supplied local path never is.
    owned: Option<PathBuf>,
    /// Set for a flattened git install: the clone is copied into the skills
    /// directory once the scan has passed.
    copy_after_scan: bool,
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

    // Determine where the content comes from, where the skills are loaded
    // from, and what this call owns.
    let plan = match detect_source_type(&source) {
        SourceType::Zip => match install_from_zip(&source, &skills_dir).await {
            Ok(path) => InstallPlan {
                owned: Some(path.clone()),
                scan: path.clone(),
                load: path,
                copy_after_scan: false,
            },
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
                Ok(Ok(path)) => InstallPlan {
                    // The clone slot holds what this call just cloned or
                    // pulled, so it is this call's to remove.
                    owned: Some(path.clone()),
                    scan: path.clone(),
                    // A flattened install loads from the skills directory and
                    // copies the clone out only after the scan passed.
                    load: if flatten { skills_dir.clone() } else { path },
                    copy_after_scan: flatten,
                },
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
        SourceType::LocalPath => {
            let path = PathBuf::from(&source);
            // A local directory is adopted as the install root: the loader
            // walks it and every installed skill points at it. It has to
            // live in the skills directory — otherwise the RPC installs from
            // a tree the skills directory never owned (a project, `/etc`,
            // `/`). A single file is not a root and stays allowed.
            if path.is_dir() && !is_inside(&path, &skills_dir) {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "Local skill directories must live inside the skills directory ({})",
                        skills_dir.display()
                    ),
                );
            }
            InstallPlan {
                // The caller's own directory: never this call's to remove.
                owned: None,
                scan: path.clone(),
                load: path,
                copy_after_scan: false,
            }
        }
    };

    // Security gate: scan all files in the installed directory before registering tools.
    // Markdown skills installed via this RPC are arbitrary third-party content → Community trust.
    //
    // The gate must run for *every* source type, not just directories: `detect_source_type`
    // routes anything that isn't a zip / http(s) URL / git URL through `LocalPath`, which
    // can be either a directory or a single `*.skill.md` file. Without the single-file
    // arm, `skills.install /tmp/downloaded.skill.md` would skip the scan entirely and
    // the loader would still pick the file up via `WalkDir`.
    let verdict = if plan.scan.is_dir() {
        scan_skill_directory(&plan.scan)
    } else {
        let file_name = plan
            .scan
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skill.md");
        match tokio::fs::read(&plan.scan).await {
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
        // Reject: clean up what this call installed and return an error
        // before any tool is registered. A caller-supplied local path is
        // not this call's to remove — the operator's files stay.
        if let Some(owned) = &plan.owned {
            let _ = std::fs::remove_dir_all(owned);
        }
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
            path = %plan.scan.display(),
            "markdown skill bundle has caution-level security findings; proceeding"
        );
    }

    // A flattened install copies the vetted clone into the skills directory
    // now: the copy happens after the scan, so nothing unvetted ever reaches
    // the directory the boot watcher scans.
    if plan.copy_after_scan {
        if let Err(e) = copy_dir_contents(&plan.scan, &skills_dir) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to copy skills: {e}"),
            );
        }
    }

    // Load skills from the directory.
    //
    // The report carries the per-file failures as well as the tools. It used
    // to carry only the tools, so a bundle whose every SKILL.md was malformed
    // came back as "No skills found in <path>" — the same sentence a genuinely
    // empty directory produces. "I could not parse what is there" is not
    // "there is nothing there", and the author needs the difference to fix it.
    let mut report = load_skills_from_dir(plan.load.clone()).await;

    if report.tools.is_empty() {
        let message = if report.errors.is_empty() {
            format!("No skills found in {}", plan.load.display())
        } else {
            format!(
                "No skills could be loaded from {}: {} file(s) failed to parse — {}",
                plan.load.display(),
                report.errors.len(),
                report.failure_summary()
            )
        };
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, message);
    }

    // Add tools to server and track paths
    let server = markdown_skills_server();
    let mut paths = SKILL_PATHS.write().await;
    let mut loaded_skills = Vec::new();

    // `failed_count` / `failure_summary` are computed *after* the loop below
    // rather than before it: a tool the owner rejects is appended to the same
    // structured failure list the loader populated, so a partially-failed
    // install never reports as a clean success and the caller sees every tool
    // that did not make it.
    for tool in std::mem::take(&mut report.tools) {
        let tool_name = tool.spec.name.clone();
        let skill_info = MarkdownSkillInfo {
            source_path: Some(plan.load.to_string_lossy().to_string()),
            ..MarkdownSkillInfo::from(&tool)
        };

        // Publish into the markdown-skill owner first so the canonical
        // `ToolHandlerRegistry` reflects the install. The owner lock is a
        // std Mutex; acquire + install + drop complete before any `.await`
        // so no lock is held across the await on `replace_tool` below.
        let tool_for_owner: Arc<dyn crate::tools::AlephToolDyn> = Arc::new(tool.clone());
        if let Err(e) = markdown_skill_registry_owner()
            .lock()
            .unwrap()
            .install_or_replace(tool_for_owner)
        {
            // Fail closed: a tool the owner rejected must not reach the
            // `AlephToolServer` store, the reload path map, or the success
            // list — otherwise the caller is told a skill exists that the
            // canonical registry will never resolve.
            warn!(
                error = %e,
                name = %tool_name,
                "markdown_skill registry owner install_or_replace failed (RPC install)"
            );
            report
                .errors
                .push((PathBuf::from(tool_name.as_str()), anyhow::anyhow!(e)));
            continue;
        }

        let update_info = server.replace_tool(tool).await;

        info!(
            name = %tool_name,
            was_replaced = update_info.was_replaced,
            "Installed Markdown skill"
        );

        paths.insert(tool_name, plan.load.clone());
        loaded_skills.push(skill_info);
    }

    let failed_count = report.errors.len();
    let failure_summary = report.failure_summary();

    JsonRpcResponse::success(
        request.id,
        json!({
            "skills": loaded_skills,
            "count": loaded_skills.len(),
            "failed_count": failed_count,
            "failed": failure_summary,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{install_from_git, is_inside};

    #[test]
    fn a_git_url_cannot_choose_the_clone_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let err = install_from_git("https://example.com/..", dir.path(), false)
            .expect_err("a traversing repository name must be rejected");
        assert!(
            err.contains("Invalid repository name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn containment_is_component_wise() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let sibling = dir.path().join("skills-old");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(is_inside(&skills.join("child"), &skills));
        assert!(!is_inside(&sibling, &skills));
        assert!(!is_inside(dir.path(), &skills));
    }
}
