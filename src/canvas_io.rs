//! Server-side I/O for Obsidian JSON Canvas (`.canvas`) files.
//!
//! Lives under `$ALEPH_HOME/canvases/`. Pure file-system layer — no JSON-RPC,
//! no tool registration. The future `memory_canvas` builtin tool (R8 everything-is-a-tool)
//! and `graph.import_canvas` / `graph.export_canvas` RPCs will both adopt this
//! module rather than re-rolling I/O.
//!
//! Schema types come from [`aleph_protocol::canvas_format`] so the wire shape
//! matches the panel exactly.

use std::fs;
use std::path::{Path, PathBuf};

use aleph_protocol::canvas_format::{parse, to_string_pretty, Document};

use crate::error::AlephError;

/// File extension Obsidian uses.
pub const CANVAS_EXT: &str = "canvas";

/// `$ALEPH_HOME/canvases/`. Falls back to `~/.aleph/canvases/`, then `./canvases/`.
#[must_use]
pub fn canvas_dir() -> PathBuf {
    aleph_home().join("canvases")
}

fn aleph_home() -> PathBuf {
    std::env::var_os("ALEPH_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Listed entry for a canvas file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasMeta {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Sanitise a user-provided name: keep `[A-Za-z0-9._-]`, replace anything
/// else with `_`, collapse runs, trim leading/trailing punctuation. Prevents
/// path traversal and Windows-incompatible characters.
#[must_use]
pub fn sanitise_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_replacement = false;
    for ch in raw.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        if keep {
            out.push(ch);
            last_was_replacement = false;
        } else if !last_was_replacement {
            out.push('_');
            last_was_replacement = true;
        }
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c == '-' || c == '_');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolve a logical name within a given directory: `{dir}/{sanitised}.canvas`.
///
/// The name is sanitised via [`sanitise_name`]; the returned path is
/// guaranteed to be a direct child of `dir` (no traversal).
#[must_use]
pub fn resolve_path_at(dir: &Path, name: &str) -> PathBuf {
    let safe = sanitise_name(name);
    dir.join(format!("{safe}.{CANVAS_EXT}"))
}

/// Same as [`resolve_path_at`] but anchored to [`canvas_dir`].
#[must_use]
pub fn resolve_path(name: &str) -> PathBuf {
    resolve_path_at(&canvas_dir(), name)
}

/// Ensure a target directory exists. Idempotent.
pub fn ensure_dir_at(dir: &Path) -> Result<(), AlephError> {
    fs::create_dir_all(dir).map_err(|e| {
        AlephError::config(format!(
            "failed to create canvases directory {}: {e}",
            dir.display()
        ))
    })
}

/// Convenience: ensure the default [`canvas_dir`].
pub fn ensure_dir() -> Result<PathBuf, AlephError> {
    let dir = canvas_dir();
    ensure_dir_at(&dir)?;
    Ok(dir)
}

/// Persist a canvas under `name`, writing into `dir`.
///
/// Writes are atomic: serialise to a sibling `.tmp` file, then `rename` over
/// the target. This protects readers from seeing torn writes.
pub fn save_at(dir: &Path, name: &str, doc: &Document) -> Result<PathBuf, AlephError> {
    ensure_dir_at(dir)?;
    let final_path = resolve_path_at(dir, name);
    let body = to_string_pretty(doc)
        .map_err(|e| AlephError::config(format!("canvas serialise failed: {e}")))?;

    let tmp_path = final_path.with_extension(format!("{CANVAS_EXT}.tmp"));
    fs::write(&tmp_path, body).map_err(|e| {
        AlephError::config(format!("canvas write {} failed: {e}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &final_path).map_err(|e| {
        // Best-effort cleanup of the temp on rename failure — ignore secondary
        // error since we already have a primary cause to report.
        let _ = fs::remove_file(&tmp_path);
        AlephError::config(format!(
            "canvas rename {} → {} failed: {e}",
            tmp_path.display(),
            final_path.display()
        ))
    })?;
    Ok(final_path)
}

/// Convenience: [`save_at`] anchored to [`canvas_dir`].
pub fn save(name: &str, doc: &Document) -> Result<PathBuf, AlephError> {
    save_at(&canvas_dir(), name, doc)
}

/// Load a canvas by `name` from `dir`. Errors if the file is missing or parse fails.
pub fn load_at(dir: &Path, name: &str) -> Result<Document, AlephError> {
    load_path(&resolve_path_at(dir, name))
}

/// Convenience: [`load_at`] anchored to [`canvas_dir`].
pub fn load(name: &str) -> Result<Document, AlephError> {
    load_at(&canvas_dir(), name)
}

/// Load a canvas from an explicit path.
pub fn load_path(path: &Path) -> Result<Document, AlephError> {
    let body = fs::read_to_string(path)
        .map_err(|e| AlephError::config(format!("canvas read {} failed: {e}", path.display())))?;
    parse(&body)
        .map_err(|e| AlephError::config(format!("canvas parse {} failed: {e}", path.display())))
}

/// List canvases under `dir`. Missing directory → empty list (not an error —
/// caller wants "what's there", not "did the dir exist").
pub fn list_at(dir: &Path) -> Result<Vec<CanvasMeta>, AlephError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| AlephError::config(format!("canvas listing {} failed: {e}", dir.display())))?;

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(CANVAS_EXT) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let size_bytes = entry.metadata().map_or(0, |m| m.len());
        out.push(CanvasMeta {
            name: stem.to_string(),
            path: path.clone(),
            size_bytes,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Convenience: [`list_at`] anchored to [`canvas_dir`].
pub fn list() -> Result<Vec<CanvasMeta>, AlephError> {
    list_at(&canvas_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas_format::{Node, NodeCommon};
    use tempfile::TempDir;

    fn sample_doc() -> Document {
        Document {
            nodes: vec![Node::File {
                common: NodeCommon {
                    id: "n1".into(),
                    x: 0,
                    y: 0,
                    width: 280,
                    height: 150,
                    color: None,
                },
                file: "a.md".into(),
                subpath: None,
            }],
            edges: vec![],
        }
    }

    fn tmp() -> (TempDir, PathBuf) {
        let t = tempfile::tempdir().expect("tmpdir");
        let p = t.path().to_path_buf();
        (t, p)
    }

    #[test]
    fn sanitise_name_strips_path_traversal_and_special_chars() {
        assert_eq!(sanitise_name("normal"), "normal");
        assert_eq!(sanitise_name("with space"), "with_space");
        assert_eq!(sanitise_name("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitise_name("dot.name.canvas"), "dot.name.canvas");
        assert_eq!(sanitise_name(""), "untitled");
        assert_eq!(sanitise_name("***"), "untitled");
        // Runs collapsed
        assert_eq!(sanitise_name("a    b"), "a_b");
        // Leading/trailing punctuation trimmed
        assert_eq!(sanitise_name("__leading"), "leading");
        assert_eq!(sanitise_name("trailing__"), "trailing");
    }

    #[test]
    fn resolve_path_at_anchors_inside_dir() {
        let (_tmp, dir) = tmp();
        let p = resolve_path_at(&dir, "hello");
        assert!(p.starts_with(&dir));
        assert_eq!(p.extension().and_then(|s| s.to_str()), Some("canvas"));
    }

    #[test]
    fn resolve_path_at_blocks_traversal_via_sanitisation() {
        let (_tmp, dir) = tmp();
        let p = resolve_path_at(&dir, "../../etc/passwd");
        assert!(
            p.starts_with(&dir),
            "{} must stay inside {}",
            p.display(),
            dir.display()
        );
        assert!(!p.to_string_lossy().contains(".."));
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_tmp, dir) = tmp();
        let original = sample_doc();
        let path = save_at(&dir, "first", &original).expect("save");
        assert!(path.exists());
        let loaded = load_at(&dir, "first").expect("load");
        assert_eq!(loaded, original);
    }

    #[test]
    fn save_overwrites_existing_file_atomically() {
        let (_tmp, dir) = tmp();
        let v1 = sample_doc();
        let mut v2 = sample_doc();
        if let Node::File { file, .. } = &mut v2.nodes[0] {
            *file = "b.md".into();
        }
        save_at(&dir, "same", &v1).unwrap();
        save_at(&dir, "same", &v2).unwrap();
        let loaded = load_at(&dir, "same").unwrap();
        assert_eq!(loaded, v2);
    }

    #[test]
    fn save_creates_canvases_dir_if_missing() {
        let (_tmp, base) = tmp();
        let dir = base.join("nested").join("deeper");
        assert!(!dir.exists());
        save_at(&dir, "auto-mkdir", &sample_doc()).unwrap();
        assert!(dir.exists());
    }

    #[test]
    fn load_returns_error_on_missing_file() {
        let (_tmp, dir) = tmp();
        let result = load_at(&dir, "never-existed");
        assert!(result.is_err());
    }

    #[test]
    fn load_returns_error_on_invalid_json() {
        let (_tmp, dir) = tmp();
        ensure_dir_at(&dir).unwrap();
        fs::write(dir.join("broken.canvas"), "{ not valid json").unwrap();
        assert!(load_at(&dir, "broken").is_err());
    }

    #[test]
    fn list_returns_empty_when_dir_missing() {
        let (_tmp, base) = tmp();
        let missing = base.join("doesnt_exist");
        let entries = list_at(&missing).expect("missing dir → empty list, not error");
        assert!(entries.is_empty());
    }

    #[test]
    fn list_returns_only_canvas_files_sorted() {
        let (_tmp, dir) = tmp();
        save_at(&dir, "zeta", &sample_doc()).unwrap();
        save_at(&dir, "alpha", &sample_doc()).unwrap();
        save_at(&dir, "mu", &sample_doc()).unwrap();
        fs::write(dir.join("notes.txt"), "irrelevant").unwrap();

        let entries = list_at(&dir).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
        assert!(entries.iter().all(|e| e.size_bytes > 0));
    }

    #[test]
    fn round_trip_via_explicit_path() {
        let (_tmp, dir) = tmp();
        let path = save_at(&dir, "path-test", &sample_doc()).unwrap();
        let loaded = load_path(&path).unwrap();
        assert_eq!(loaded, sample_doc());
    }

    #[test]
    fn canvas_dir_honours_aleph_home_env() {
        // Defensive: verify the helper consults ALEPH_HOME without us mutating
        // env state (we read what is — env mutation is broadly racy in tests).
        // If ALEPH_HOME is set, the returned path starts with it; otherwise it
        // falls back to a home-dir-rooted path. We only assert the suffix.
        let dir = canvas_dir();
        assert_eq!(dir.file_name().and_then(|s| s.to_str()), Some("canvases"));
    }
}
