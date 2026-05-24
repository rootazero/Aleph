//! On-disk project catalogue. See [`crate::projects`] for the module-level
//! contract.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

/// Maximum number of remembered projects. Older entries are evicted by
/// `last_used_at` once we exceed this bound — Claude Code keeps every
/// project forever and the directory grows unbounded; we cap to keep the
/// Panel picker scannable.
pub const RECENT_PROJECTS_CAP: usize = 64;

const STORE_VERSION: u32 = 1;

/// One remembered project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Stable identifier derived from the canonicalised path
    /// (`sha256(path)[..16]` lowercase hex). Two registrations of the same
    /// folder collapse onto the same project — never collide.
    pub id: String,
    /// Display name shown in the Panel picker. Defaults to the folder's
    /// basename; the user may rename later via `projects.rename`.
    pub name: String,
    /// Absolute path on disk. Always canonicalised at insert time so that
    /// `~/foo` and `/Users/me/foo` resolve to the same entry.
    pub path: PathBuf,
    /// Unix-seconds creation time.
    pub created_at: i64,
    /// Unix-seconds last activation time. Bumped by `touch()` whenever a
    /// run begins in this project so the picker can sort by recency.
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    projects: Vec<Project>,
}

impl Default for StoreFile {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            projects: Vec::new(),
        }
    }
}

/// Typed errors surfaced to RPC handlers.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("path not absolute: {0}")]
    NotAbsolute(PathBuf),
    #[error("path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("project not found: {0}")]
    NotFound(String),
    #[error("project already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("invalid project name: {0}")]
    InvalidName(String),
}

/// Default catalogue path: `~/.aleph/projects.json`.
///
/// Falls back to `ALEPH_HOME` when set, otherwise `dirs::home_dir()`. We do
/// **not** silently drop back to `/tmp` — that path is wiped on reboot and
/// would surface as "the project picker forgot everything overnight" with
/// no diagnostic. Callers who hit the panic path can set `ALEPH_HOME` to
/// rescue the daemon.
pub fn default_projects_path() -> PathBuf {
    aleph_home().join("projects.json")
}

/// Resolve the catalogue's parent directory. Public so callers can locate
/// it without recomputing the fallback rules.
pub(crate) fn aleph_home() -> PathBuf {
    if let Ok(p) = std::env::var("ALEPH_HOME") {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            return pb;
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| {
        panic!(
            "projects: $HOME unavailable and $ALEPH_HOME unset; refusing to fall back to a \
             volatile path that would lose registered projects on reboot"
        )
    });
    home.join(".aleph")
}

/// Stable ID for a given path. Public so callers can pre-compute the ID
/// without going through the store (e.g. UI cache lookups).
pub fn project_id_for_path(path: &Path) -> String {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    hex_lower(&digest[..8])
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// File-backed catalogue. Cheap to construct — all state lives on disk and
/// is re-read on every operation under an advisory lock so concurrent
/// processes (Panel + CLI) stay consistent.
#[derive(Debug, Clone)]
pub struct ProjectStore {
    path: PathBuf,
}

impl ProjectStore {
    /// Open the catalogue at the default location (`~/.aleph/projects.json`).
    pub fn new() -> Self {
        Self::with_path(default_projects_path())
    }

    /// Open the catalogue at an explicit path. Used by tests.
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    fn lock_path(&self) -> PathBuf {
        let mut p = self.path.clone();
        let new_name = match p.file_name() {
            Some(name) => format!("{}.lock", name.to_string_lossy()),
            None => "projects.json.lock".to_string(),
        };
        p.set_file_name(new_name);
        p
    }

    fn ensure_parent(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn read_unlocked(&self) -> Result<StoreFile, ProjectError> {
        match std::fs::read(&self.path) {
            Ok(bytes) if !bytes.is_empty() => {
                let parsed: StoreFile = serde_json::from_slice(&bytes)?;
                Ok(parsed)
            }
            Ok(_) => Ok(StoreFile::default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(e) => Err(ProjectError::Io(e)),
        }
    }

    fn write_unlocked(&self, file: &StoreFile) -> Result<(), ProjectError> {
        let bytes = serde_json::to_vec_pretty(file)?;
        write_atomic(&self.path, &bytes)?;
        Ok(())
    }

    fn with_locked_file<T, F>(&self, f: F) -> Result<T, ProjectError>
    where
        F: FnOnce(&mut StoreFile) -> Result<T, ProjectError>,
    {
        self.ensure_parent()?;
        let lock_path = self.lock_path();
        let result: Result<T, ProjectError> =
            with_file_lock(&lock_path, |_| {
                let mut file = self
                    .read_unlocked()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
                match f(&mut file) {
                    Ok(value) => {
                        self.write_unlocked(&file).map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                        })?;
                        Ok(Ok(value))
                    }
                    Err(e) => Ok(Err(e)),
                }
            })?;
        result
    }

    /// Return projects ordered by `last_used_at` descending.
    pub fn list(&self) -> Result<Vec<Project>, ProjectError> {
        let file = self.with_locked_file(|f| Ok(f.clone()))?;
        let mut projects = file.projects;
        projects.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        Ok(projects)
    }

    /// Insert (or refresh) a project entry for `path`. If an entry with
    /// the same path already exists, only `last_used_at` is bumped and the
    /// existing entry is returned.
    pub fn add(&self, path: &Path, name: Option<String>) -> Result<Project, ProjectError> {
        let absolute = canonical_dir(path)?;
        self.with_locked_file(|file| {
            let id = project_id_for_path(&absolute);
            if let Some(existing) = file.projects.iter_mut().find(|p| p.id == id) {
                existing.last_used_at = now_secs();
                if let Some(new_name) = name.clone() {
                    if !new_name.trim().is_empty() {
                        existing.name = new_name.trim().to_string();
                    }
                }
                return Ok(existing.clone());
            }
            let display_name = resolve_display_name(&absolute, name)?;
            let now = now_secs();
            let project = Project {
                id: id.clone(),
                name: display_name,
                path: absolute.clone(),
                created_at: now,
                last_used_at: now,
            };
            file.projects.push(project.clone());
            evict_overflow(&mut file.projects);
            Ok(project)
        })
    }

    /// Materialise a fresh empty directory at `<parent>/<name>` then
    /// register it. Fails if the directory already exists — callers must
    /// route the user through `add` for existing folders.
    pub fn create_blank(&self, parent: &Path, name: &str) -> Result<Project, ProjectError> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.contains(std::path::MAIN_SEPARATOR) {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        let parent_abs = canonical_dir(parent)?;
        let target = parent_abs.join(trimmed);
        if target.exists() {
            return Err(ProjectError::AlreadyExists(target));
        }
        std::fs::create_dir_all(&target)?;
        self.add(&target, Some(trimmed.to_string()))
    }

    /// Bump `last_used_at` for an existing project; no-op if absent.
    pub fn touch(&self, id: &str) -> Result<(), ProjectError> {
        self.with_locked_file(|file| {
            if let Some(p) = file.projects.iter_mut().find(|p| p.id == id) {
                p.last_used_at = now_secs();
            }
            Ok(())
        })
    }

    /// Drop a project entry. The on-disk folder is left untouched —
    /// removal here means "forget about this project", not "delete files".
    pub fn remove(&self, id: &str) -> Result<(), ProjectError> {
        self.with_locked_file(|file| {
            let before = file.projects.len();
            file.projects.retain(|p| p.id != id);
            if file.projects.len() == before {
                Err(ProjectError::NotFound(id.to_string()))
            } else {
                Ok(())
            }
        })
    }

    /// Look up a project by ID.
    pub fn get(&self, id: &str) -> Result<Option<Project>, ProjectError> {
        let file = self.with_locked_file(|f| Ok(f.clone()))?;
        Ok(file.projects.into_iter().find(|p| p.id == id))
    }

    /// Look up a project by its canonical absolute path.
    pub fn find_by_path(&self, path: &Path) -> Result<Option<Project>, ProjectError> {
        let canonical = canonical_dir(path)?;
        let id = project_id_for_path(&canonical);
        self.get(&id)
    }
}

impl Default for ProjectStore {
    fn default() -> Self {
        Self::new()
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf, ProjectError> {
    if !path.is_absolute() {
        return Err(ProjectError::NotAbsolute(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(ProjectError::NotDirectory(path.to_path_buf()));
    }
    let canonical = std::fs::canonicalize(path)?;
    Ok(canonical)
}

fn resolve_display_name(path: &Path, override_name: Option<String>) -> Result<String, ProjectError> {
    if let Some(name) = override_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    Ok(basename.to_string())
}

fn evict_overflow(projects: &mut Vec<Project>) {
    if projects.len() <= RECENT_PROJECTS_CAP {
        return;
    }
    projects.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    projects.truncate(RECENT_PROJECTS_CAP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_store(dir: &Path) -> ProjectStore {
        ProjectStore::with_path(dir.join("projects.json"))
    }

    #[test]
    fn list_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn add_persists_and_dedupes() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let proj_dir = dir.path().join("alpha");
        std::fs::create_dir_all(&proj_dir).unwrap();

        let first = store.add(&proj_dir, None).unwrap();
        assert_eq!(first.name, "alpha");
        assert_eq!(first.path, std::fs::canonicalize(&proj_dir).unwrap());

        let second = store.add(&proj_dir, Some("Renamed".to_string())).unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.name, "Renamed");

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn add_rejects_relative_and_missing() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let relative = PathBuf::from("./not-absolute");
        let err = store.add(&relative, None).unwrap_err();
        assert!(matches!(err, ProjectError::NotAbsolute(_)));

        let missing = dir.path().join("ghost");
        let err = store.add(&missing, None).unwrap_err();
        assert!(matches!(err, ProjectError::NotDirectory(_)));
    }

    #[test]
    fn create_blank_makes_dir_and_registers() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let project = store.create_blank(dir.path(), "new-app").unwrap();
        assert!(project.path.exists());
        assert!(project.path.is_dir());
        assert_eq!(project.name, "new-app");
    }

    #[test]
    fn create_blank_refuses_existing_dir() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let existing = dir.path().join("preexisting");
        std::fs::create_dir_all(&existing).unwrap();
        let err = store.create_blank(dir.path(), "preexisting").unwrap_err();
        assert!(matches!(err, ProjectError::AlreadyExists(_)));
    }

    #[test]
    fn create_blank_rejects_separator_in_name() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let err = store.create_blank(dir.path(), "nested/bad").unwrap_err();
        assert!(matches!(err, ProjectError::InvalidName(_)));
    }

    #[test]
    fn touch_updates_last_used() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let p = dir.path().join("touchy");
        std::fs::create_dir_all(&p).unwrap();
        let project = store.add(&p, None).unwrap();
        let before = project.last_used_at;
        std::thread::sleep(std::time::Duration::from_millis(1100));
        store.touch(&project.id).unwrap();
        let after = store.get(&project.id).unwrap().unwrap();
        assert!(after.last_used_at >= before + 1);
    }

    #[test]
    fn remove_drops_entry() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let p = dir.path().join("rm");
        std::fs::create_dir_all(&p).unwrap();
        let project = store.add(&p, None).unwrap();
        store.remove(&project.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        let err = store.remove(&project.id).unwrap_err();
        assert!(matches!(err, ProjectError::NotFound(_)));
    }

    #[test]
    fn list_sorted_by_recency() {
        let dir = tempdir().unwrap();
        let store = fresh_store(dir.path());
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let pa = store.add(&a, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let pb = store.add(&b, None).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed[0].id, pb.id);
        assert_eq!(listed[1].id, pa.id);
    }

    #[test]
    fn project_id_is_stable_for_canonical_path() {
        let dir = tempdir().unwrap();
        let id1 = project_id_for_path(dir.path());
        let id2 = project_id_for_path(dir.path());
        assert_eq!(id1, id2);
    }

    /// `aleph_home()` honours `$ALEPH_HOME` when set so the daemon can be
    /// pointed at an alternate data root (containers, alternate user). This
    /// is also our escape hatch if `dirs::home_dir()` panics on weird boxes.
    #[test]
    fn aleph_home_respects_env_override() {
        let dir = tempdir().unwrap();
        let prev = std::env::var("ALEPH_HOME").ok();
        // SAFETY: this single-threaded test mutates a process env var; the
        // Rust 2024 unsafe-block rule forbids env writes without acknowledgement.
        unsafe {
            std::env::set_var("ALEPH_HOME", dir.path());
        }
        let resolved = aleph_home();
        assert_eq!(resolved, dir.path());
        match prev {
            Some(v) => unsafe { std::env::set_var("ALEPH_HOME", v) },
            None => unsafe { std::env::remove_var("ALEPH_HOME") },
        }
    }
}
