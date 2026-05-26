//! Canonicalized session pool key + path-normalization helpers.

use std::path::PathBuf;

use tracing::debug;

/// Canonicalized session pool key — prevents duplicate sessions for equivalent paths.
///
/// Includes an optional `name` so callers can run multiple parallel
/// sessions in the same repo (mirrors acpx's `-s backend` / `-s frontend`).
/// The empty string is the canonical "default" name and is the legacy
/// shape — old callers via `SessionKey::new` see no behavioral change.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SessionKey {
    pub(super) harness_id: String,
    pub(super) cwd: PathBuf,
    pub(super) name: String,
}

impl SessionKey {
    /// Default (unnamed) session for the given harness + cwd.
    pub fn new(harness_id: &str, cwd: &str) -> Self {
        Self::with_name(harness_id, cwd, None)
    }

    /// Named session — `None` is equivalent to `new()` (empty name).
    pub fn with_name(harness_id: &str, cwd: &str, name: Option<&str>) -> Self {
        Self {
            harness_id: harness_id.to_string(),
            cwd: canonicalize_cwd(cwd),
            name: name.unwrap_or("").to_string(),
        }
    }

    pub fn harness_id(&self) -> &str {
        &self.harness_id
    }

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// Empty string for the default/unnamed session.
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub(super) fn canonicalize_cwd(cwd: &str) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| {
        let path = PathBuf::from(cwd);
        let normalized = normalize_path(&path);
        let final_path = if normalized.is_absolute() {
            normalized
        } else {
            std::env::current_dir()
                .map(|cd| cd.join(&normalized))
                .unwrap_or(normalized)
        };
        debug!(
            cwd,
            ?final_path,
            "SessionKey: canonicalize failed, using normalized path"
        );
        final_path
    })
}

pub(super) fn normalize_path(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => result.push(prefix.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if result.parent().is_some() {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
            Component::Normal(name) => result.push(name),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}
