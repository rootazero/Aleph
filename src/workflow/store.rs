//! File-system persistence for workflow templates.
//!
//! Lives under `$ALEPH_HOME/workflows/*.json`. Pure file-system layer — no
//! JSON-RPC, no tool registration (R4). Mirrors [`crate::canvas_io`]: atomic
//! writes via temp-file + rename so readers never see a torn write, and
//! reuses its `sanitise_name` for path-traversal-safe filenames.

use std::fs;
use std::path::{Path, PathBuf};

use crate::canvas_io::sanitise_name;
use crate::error::{AlephError, Result};
use crate::workflow::def::WorkflowDef;

/// File extension for stored workflow templates.
pub const WORKFLOW_EXT: &str = "json";

/// `$ALEPH_HOME/workflows/`. Falls back to `~/.aleph/workflows/`, then
/// `./workflows/`.
pub fn workflow_dir() -> PathBuf {
    aleph_home().join("workflows")
}

fn aleph_home() -> PathBuf {
    std::env::var_os("ALEPH_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Listed entry for a workflow file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMeta {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Resolve a logical name within `dir`: `{dir}/{sanitised}.json`. The returned
/// path is guaranteed to be a direct child of `dir` (no traversal).
pub fn resolve_path_at(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.{WORKFLOW_EXT}", sanitise_name(name)))
}

fn ensure_dir_at(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| {
        AlephError::config(format!(
            "failed to create workflows directory {}: {e}",
            dir.display()
        ))
    })
}

/// Persist `def` under its own name into `dir`. Validates before writing —
/// an invalid template never reaches disk.
pub fn save_at(dir: &Path, def: &WorkflowDef) -> Result<PathBuf> {
    def.validate()?;
    ensure_dir_at(dir)?;
    let final_path = resolve_path_at(dir, &def.name);
    let body = serde_json::to_string_pretty(def)
        .map_err(|e| AlephError::config(format!("workflow serialise failed: {e}")))?;

    let tmp_path = final_path.with_extension(format!("{WORKFLOW_EXT}.tmp"));
    fs::write(&tmp_path, body)
        .map_err(|e| AlephError::config(format!("workflow write {} failed: {e}", tmp_path.display())))?;
    fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AlephError::config(format!(
            "workflow rename {} → {} failed: {e}",
            tmp_path.display(),
            final_path.display()
        ))
    })?;
    Ok(final_path)
}

/// Convenience: [`save_at`] anchored to [`workflow_dir`].
pub fn save(def: &WorkflowDef) -> Result<PathBuf> {
    save_at(&workflow_dir(), def)
}

/// Load a workflow by `name` from `dir`. Errors if missing or parse fails.
pub fn load_at(dir: &Path, name: &str) -> Result<WorkflowDef> {
    let path = resolve_path_at(dir, name);
    let body = fs::read_to_string(&path)
        .map_err(|e| AlephError::config(format!("workflow read {} failed: {e}", path.display())))?;
    serde_json::from_str(&body)
        .map_err(|e| AlephError::config(format!("workflow parse {} failed: {e}", path.display())))
}

/// Convenience: [`load_at`] anchored to [`workflow_dir`].
pub fn load(name: &str) -> Result<WorkflowDef> {
    load_at(&workflow_dir(), name)
}

/// List workflows under `dir`. A missing directory yields an empty list (the
/// caller wants "what's there", not "did the dir exist").
pub fn list_at(dir: &Path) -> Result<Vec<WorkflowMeta>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| AlephError::config(format!("workflow listing {} failed: {e}", dir.display())))?;

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(WORKFLOW_EXT) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(WorkflowMeta {
            name: stem.to_string(),
            path: path.clone(),
            size_bytes,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Convenience: [`list_at`] anchored to [`workflow_dir`].
pub fn list() -> Result<Vec<WorkflowMeta>> {
    list_at(&workflow_dir())
}

/// Delete a workflow by `name` from `dir`. Returns `true` if a file was
/// removed, `false` if it did not exist (idempotent delete).
pub fn delete_at(dir: &Path, name: &str) -> Result<bool> {
    let path = resolve_path_at(dir, name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AlephError::config(format!(
            "workflow delete {} failed: {e}",
            path.display()
        ))),
    }
}

/// Convenience: [`delete_at`] anchored to [`workflow_dir`].
pub fn delete(name: &str) -> Result<bool> {
    delete_at(&workflow_dir(), name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::def::WorkflowStepDef;
    use tempfile::TempDir;

    fn sample(name: &str) -> WorkflowDef {
        WorkflowDef {
            name: name.into(),
            description: "demo".into(),
            steps: vec![
                WorkflowStepDef {
                    id: "gather".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                },
                WorkflowStepDef {
                    id: "write".into(),
                    agent: "writer".into(),
                    prompt: "write it up".into(),
                    depends_on: vec!["gather".into()],
                },
            ],
        }
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let d = sample("report");
        save_at(tmp.path(), &d).unwrap();
        let back = load_at(tmp.path(), "report").unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn save_rejects_invalid_def() {
        let tmp = TempDir::new().unwrap();
        let mut d = sample("bad");
        d.steps.clear();
        assert!(save_at(tmp.path(), &d).is_err());
        // Nothing written.
        assert!(list_at(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn list_is_sorted_and_skips_non_json() {
        let tmp = TempDir::new().unwrap();
        save_at(tmp.path(), &sample("zebra")).unwrap();
        save_at(tmp.path(), &sample("alpha")).unwrap();
        fs::write(tmp.path().join("notes.txt"), "ignore me").unwrap();
        let names: Vec<String> = list_at(tmp.path()).unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["alpha".to_string(), "zebra".to_string()]);
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope");
        assert!(list_at(&missing).unwrap().is_empty());
    }

    #[test]
    fn delete_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        save_at(tmp.path(), &sample("temp")).unwrap();
        assert!(delete_at(tmp.path(), "temp").unwrap());
        assert!(!delete_at(tmp.path(), "temp").unwrap());
        assert!(load_at(tmp.path(), "temp").is_err());
    }

    #[test]
    fn name_is_sanitised_against_traversal() {
        let tmp = TempDir::new().unwrap();
        let mut d = sample("../escape");
        d.name = "../escape".into();
        let path = save_at(tmp.path(), &d).unwrap();
        // Stored file stays a direct child of tmp dir.
        assert_eq!(path.parent().unwrap(), tmp.path());
    }
}
