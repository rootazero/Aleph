//! File-system persistence for workflow templates.
//!
//! Lives under `$ALEPH_HOME/workflows/*.json`. Pure file-system layer — no
//! JSON-RPC, no tool registration (R4). Mirrors [`crate::json_canvas_io`]: atomic
//! writes via temp-file + rename so readers never see a torn write, and
//! reuses its `sanitise_name` for path-traversal-safe filenames.
//!
//! The persisted document is the [`WorkflowManifest`] superset — the single
//! source of truth (see [`super::interop::manifest`]). Storing the manifest
//! rather than the lean [`WorkflowDef`](crate::workflow::def::WorkflowDef)
//! keeps the `.workflow.js`-compatible metadata (`whenToUse`, `phases` with
//! per-phase `model`, per-step `label`/`model`/`phase`/`schema`/`isolation`/
//! `agentType`) durable across
//! `import → save → export`, so an exported template faithfully reproduces the
//! engineering format. Execution still consumes only the projected core via
//! [`WorkflowManifest::to_def`] (R10 — the executor never sees the extra
//! metadata). Legacy `snake_case` `WorkflowDef.json` files load unchanged via a
//! serde alias on the manifest step.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{AlephError, Result};
use crate::json_canvas_io::sanitise_name;
use crate::workflow::interop::manifest::WorkflowManifest;

/// File extension for stored workflow templates. Private: the two readers
/// (`resolve_path_at`, `list_at`) are both in this file, and the store's public
/// face speaks in names, never extensions.
const WORKFLOW_EXT: &str = "json";

/// Monotonic suffix source: two concurrent writers of the SAME final path must
/// not share a temp file, or one writer's `rename` would publish the other
/// writer's half-written bytes (and the loser's `rename` would spuriously
/// fail). Combined with the pid this is unique within and across processes.
/// Static `AtomicU64` deliberately uses `std::sync::atomic` (loom's `new` is
/// not `const`) — the documented sync-primitives exception.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A temp sibling of `final_path`, unique per writer: `{final}.{pid}.{seq}.tmp`.
/// The `.tmp` suffix keeps it out of [`list_at`] (which matches only `.json`).
fn unique_tmp_path(final_path: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tmp = final_path.as_os_str().to_os_string();
    tmp.push(format!(".{}.{seq}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

/// `$ALEPH_HOME/workflows/`. Falls back to `~/.aleph/workflows/`, then
/// `./workflows/`.
#[must_use]
pub fn workflow_dir() -> PathBuf {
    aleph_home().join("workflows")
}

/// Aleph home, or the CWD when it cannot be resolved at all.
/// Single source: [`crate::utils::paths::get_config_dir`] (see `json_canvas_io`).
fn aleph_home() -> PathBuf {
    crate::utils::paths::get_config_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Listed entry for a workflow file on disk.
///
/// Carries the fields a caller needs to *choose* a workflow, not just name one.
/// `list` used to return a bare stem, so answering "which of these should I
/// run?" meant one `describe` per template — and `when_to_use`, the field the
/// `.workflow.js` format exists to put in front of that decision, had no
/// runtime reader at all (neither `list` nor `describe` surfaced it; only
/// `export` did).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMeta {
    /// Storage key — the sanitised file stem, which is what `describe` / `run`
    /// / `delete` resolve against.
    pub name: String,
    /// The manifest's own `description`.
    pub description: String,
    /// The manifest's `whenToUse` — selection guidance for the caller.
    pub when_to_use: String,
    /// Number of steps, so a caller can tell a two-step template from a
    /// twenty-step one without loading it.
    pub steps: usize,
}

/// The result of walking the workflow directory: the entries that parsed, and
/// the ones that did not.
///
/// Problems are **returned, not swallowed**. A `continue` past an unreadable
/// file is right for the listing (one bad template must not fail the whole
/// list) and a lie for the caller, who otherwise cannot distinguish "you have
/// no workflow called that" from "the file is there and corrupt". Each caller
/// decides what to do with `problems`; the walk does not decide for them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowListing {
    pub entries: Vec<WorkflowMeta>,
    /// One human-readable line per file that could not be read or parsed,
    /// naming the file.
    pub problems: Vec<String>,
}
/// Resolve a logical name within `dir`: `{dir}/{sanitised}.json`. The returned
/// path is guaranteed to be a direct child of `dir` (no traversal).
#[must_use]
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

/// Persist `manifest` under its own name into `dir`. Validates before writing —
/// an invalid template never reaches disk.
pub fn save_at(dir: &Path, manifest: &WorkflowManifest) -> Result<PathBuf> {
    manifest.validate()?;
    ensure_dir_at(dir)?;
    let final_path = resolve_path_at(dir, &manifest.name);
    let body = serde_json::to_string_pretty(manifest)
        .map_err(|e| AlephError::config(format!("workflow serialise failed: {e}")))?;

    let tmp_path = unique_tmp_path(&final_path);
    if let Err(e) = fs::write(&tmp_path, body) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AlephError::config(format!(
            "workflow write {} failed: {e}",
            tmp_path.display()
        )));
    }
    if let Err(e) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AlephError::config(format!(
            "workflow rename {} → {} failed: {e}",
            tmp_path.display(),
            final_path.display()
        )));
    }
    Ok(final_path)
}

/// Convenience: [`save_at`] anchored to [`workflow_dir`].
pub fn save(manifest: &WorkflowManifest) -> Result<PathBuf> {
    save_at(&workflow_dir(), manifest)
}

/// Write rendered text (e.g. an exported `.mjs` workflow) into `dir` under
/// `{sanitised name}.{ext}`, atomically (temp + rename). Returns the path.
///
/// Private: the only caller is [`write_text`] (plus this file's tests). The
/// `_at` pair exists so tests can target a tmpdir, and that is a within-file
/// concern.
fn write_text_at(dir: &Path, name: &str, ext: &str, body: &str) -> Result<PathBuf> {
    ensure_dir_at(dir)?;
    let final_path = dir.join(format!("{}.{ext}", sanitise_name(name)));
    let tmp_path = unique_tmp_path(&final_path);
    if let Err(e) = fs::write(&tmp_path, body) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AlephError::config(format!(
            "write {} failed: {e}",
            tmp_path.display()
        )));
    }
    if let Err(e) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AlephError::config(format!(
            "rename {} → {} failed: {e}",
            tmp_path.display(),
            final_path.display()
        )));
    }
    Ok(final_path)
}

/// Convenience: [`write_text_at`] anchored to [`workflow_dir`].
pub fn write_text(name: &str, ext: &str, body: &str) -> Result<PathBuf> {
    write_text_at(&workflow_dir(), name, ext, body)
}

/// Load a workflow by `name` from `dir`. Errors if missing or parse fails.
/// Legacy `snake_case` `WorkflowDef.json` files deserialise transparently via the
/// `depends_on` serde alias on [`WorkflowManifest`]'s step.
pub fn load_at(dir: &Path, name: &str) -> Result<WorkflowManifest> {
    let path = resolve_path_at(dir, name);
    let body = fs::read_to_string(&path)
        .map_err(|e| AlephError::config(format!("workflow read {} failed: {e}", path.display())))?;
    serde_json::from_str(&body)
        .map_err(|e| AlephError::config(format!("workflow parse {} failed: {e}", path.display())))
}

/// Convenience: [`load_at`] anchored to [`workflow_dir`].
pub fn load(name: &str) -> Result<WorkflowManifest> {
    load_at(&workflow_dir(), name)
}

/// List workflows under `dir`. A missing directory yields an empty listing (the
/// caller wants "what's there", not "did the dir exist").
///
/// Each `.json` is parsed so the entry can carry `description` / `whenToUse` /
/// step count. A file that will not read or parse becomes a line in
/// [`WorkflowListing::problems`] rather than a silent omission — otherwise a
/// corrupt template is indistinguishable from a template that was never saved,
/// on every surface at once.
pub fn list_at(dir: &Path) -> Result<WorkflowListing> {
    if !dir.exists() {
        return Ok(WorkflowListing::default());
    }
    let entries = fs::read_dir(dir).map_err(|e| {
        AlephError::config(format!("workflow listing {} failed: {e}", dir.display()))
    })?;

    let mut listing = WorkflowListing::default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(WORKFLOW_EXT) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let name = stem.to_string();
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|body| {
                serde_json::from_str::<WorkflowManifest>(&body).map_err(|e| e.to_string())
            }) {
            Ok(manifest) => listing.entries.push(WorkflowMeta {
                name,
                description: manifest.description,
                when_to_use: manifest.when_to_use,
                steps: manifest.steps.len(),
            }),
            Err(e) => {
                // Named, so the caller can act on it. Still listed by name:
                // `delete` works on an unparseable file, and hiding it would
                // make the only remedy undiscoverable.
                listing
                    .problems
                    .push(format!("{}: unreadable workflow ({e})", path.display()));
                listing.entries.push(WorkflowMeta {
                    name,
                    description: String::new(),
                    when_to_use: String::new(),
                    steps: 0,
                });
            }
        }
    }
    listing.entries.sort_by(|a, b| a.name.cmp(&b.name));
    listing.problems.sort();
    Ok(listing)
}

/// Convenience: [`list_at`] anchored to [`workflow_dir`].
pub fn list() -> Result<WorkflowListing> {
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
    use crate::workflow::interop::manifest::{WorkflowManifestStep, WorkflowPhase};
    use tempfile::TempDir;

    fn sample(name: &str) -> WorkflowManifest {
        WorkflowManifest {
            name: name.into(),
            description: "demo".into(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![
                WorkflowManifestStep {
                    id: "gather".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
                },
                WorkflowManifestStep {
                    id: "write".into(),
                    agent: "writer".into(),
                    prompt: "write it up".into(),
                    depends_on: vec!["gather".into()],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
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
    fn rich_manifest_metadata_survives_disk_roundtrip() {
        // The whole point of persisting the manifest: per-step model/schema/phase
        // and meta whenToUse/phases must come back byte-identical so a later
        // `export` reproduces the engineering format faithfully.
        let tmp = TempDir::new().unwrap();
        let mut m = sample("rich");
        m.when_to_use = "use for reports".into();
        m.phases = vec![WorkflowPhase {
            title: "Gather".into(),
            detail: "collect".into(),
            model: Some("opus".into()),
        }];
        m.steps[0].model = Some("haiku".into());
        m.steps[0].phase = Some("Gather".into());
        m.steps[0].label = Some("audit:gather".into());
        m.steps[0].schema = Some(serde_json::json!({"type": "object"}));
        m.steps[0].isolation = Some("worktree".into());
        m.steps[0].agent_type = Some("Explore".into());
        save_at(tmp.path(), &m).unwrap();
        let back = load_at(tmp.path(), "rich").unwrap();
        assert_eq!(m, back, "rich metadata is durable across save/load");
    }

    #[test]
    fn legacy_workflow_def_json_loads_via_alias() {
        // A file written before the manifest migration uses snake_case
        // `depends_on` and carries none of the extra fields. It must still load.
        let tmp = TempDir::new().unwrap();
        ensure_dir_at(tmp.path()).unwrap();
        let legacy = r#"{
            "name": "legacy",
            "description": "old",
            "steps": [
                {"id": "a", "agent": "w", "prompt": "do a"},
                {"id": "b", "agent": "w", "prompt": "do b", "depends_on": ["a"]}
            ]
        }"#;
        fs::write(resolve_path_at(tmp.path(), "legacy"), legacy).unwrap();
        let back = load_at(tmp.path(), "legacy").unwrap();
        assert_eq!(back.name, "legacy");
        assert_eq!(back.steps.len(), 2);
        assert_eq!(back.steps[1].depends_on, vec!["a".to_string()]);
        assert!(back.when_to_use.is_empty() && back.phases.is_empty());
    }

    #[test]
    fn save_rejects_invalid_def() {
        let tmp = TempDir::new().unwrap();
        let mut d = sample("bad");
        d.steps.clear();
        assert!(save_at(tmp.path(), &d).is_err());
        // Nothing written.
        assert!(list_at(tmp.path()).unwrap().entries.is_empty());
    }

    #[test]
    fn list_is_sorted_and_skips_non_json() {
        let tmp = TempDir::new().unwrap();
        save_at(tmp.path(), &sample("zebra")).unwrap();
        save_at(tmp.path(), &sample("alpha")).unwrap();
        fs::write(tmp.path().join("notes.txt"), "ignore me").unwrap();
        let names: Vec<String> = list_at(tmp.path())
            .unwrap()
            .entries
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "zebra".to_string()]);
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope");
        assert!(list_at(&missing).unwrap().entries.is_empty());
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

    #[test]
    fn write_text_at_writes_body_to_expected_path() {
        let tmp = TempDir::new().unwrap();
        let path = write_text_at(tmp.path(), "report", "workflow.js", "// body").unwrap();
        assert_eq!(path, tmp.path().join("report.workflow.js"));
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "// body");
        // No stray temp file left behind.
        assert!(!tmp.path().join("report.workflow.js.tmp").exists());
    }

    #[test]
    fn write_text_at_sanitises_name_against_traversal() {
        let tmp = TempDir::new().unwrap();
        let path = write_text_at(tmp.path(), "../escape", "workflow.js", "x").unwrap();
        // Stored file stays a direct child of tmp dir.
        assert_eq!(path.parent().unwrap(), tmp.path());
    }

    #[cfg(unix)]
    #[test]
    fn save_at_cleans_tmp_when_write_fails() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let mut perms = fs::metadata(dir).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(dir, perms).unwrap();
        let result = save_at(dir, &sample("blocked"));
        let mut restore = fs::metadata(dir).unwrap().permissions();
        restore.set_mode(0o755);
        fs::set_permissions(dir, restore).unwrap();
        assert!(result.is_err(), "read-only dir must fail");
        let entries: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.iter().all(|n| !n.ends_with(".tmp")),
            "no .tmp left behind on write failure, got: {entries:?}"
        );
    }

    #[test]
    fn list_entries_carry_selection_fields() {
        // `list` is the choosing surface: description / whenToUse / step count
        // ride on each row so a caller does not need one `describe` per
        // candidate — and `whenToUse` finally has a runtime reader at all.
        let tmp = TempDir::new().unwrap();
        let mut m = sample("chooser");
        m.when_to_use = "when the repo needs a two-step research report".into();
        save_at(tmp.path(), &m).unwrap();
        let listing = list_at(tmp.path()).unwrap();
        assert!(listing.problems.is_empty());
        assert_eq!(listing.entries.len(), 1);
        let entry = &listing.entries[0];
        assert_eq!(entry.name, "chooser");
        assert_eq!(entry.description, "demo");
        assert_eq!(
            entry.when_to_use,
            "when the repo needs a two-step research report"
        );
        assert_eq!(entry.steps, 2);
    }

    #[test]
    fn list_names_a_corrupt_file_instead_of_hiding_it() {
        // A `continue` past an unreadable file is right for the walk and a lie
        // for the caller: a corrupt template would read as "never saved" on
        // every surface at once. The listing must (a) say which file is broken
        // and (b) still list it by name, because `delete` works on it and
        // hiding it makes the only remedy undiscoverable.
        let tmp = TempDir::new().unwrap();
        save_at(tmp.path(), &sample("good")).unwrap();
        fs::write(tmp.path().join("broken.json"), "{ not json").unwrap();
        let listing = list_at(tmp.path()).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["broken", "good"], "broken is still addressable");
        assert_eq!(listing.problems.len(), 1);
        assert!(
            listing.problems[0].contains("broken.json"),
            "problem names the file: {}",
            listing.problems[0]
        );
    }
}
