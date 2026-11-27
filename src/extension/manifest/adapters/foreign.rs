//! Filesystem containment shared by imported plugin formats.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::extension::manifest::inputs;

pub(super) fn checked_path(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    match inputs::canonicalize(path) {
        Ok(resolved) if resolved.starts_with(root) => Ok(Some(resolved)),
        Ok(_) => bail!("Resource escapes package: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
                bail!(
                    "Package resource symlink cannot be resolved: {}",
                    path.display()
                );
            }
            if let Some(parent) = path.parent().filter(|parent| parent.starts_with(root)) {
                checked_path(root, parent)?;
            }
            Ok(None)
        }
        Err(error) => Err(error).with_context(|| format!("cannot resolve {}", path.display())),
    }
}

pub(super) fn relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("Package resource path must stay within the package: {value}");
    }
    Ok(path
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .collect())
}

/// Collect conventional directory skills without following an escaping path.
pub(super) fn directory_skills(root: &Path) -> Result<Vec<PathBuf>> {
    let mut skills = Vec::new();
    let mut pending = vec![root.join("skills")];
    let mut visited = std::collections::HashSet::new();
    while let Some(path) = pending.pop() {
        let Some(resolved) = checked_path(root, &path)? else {
            continue;
        };
        if !inputs::is_dir(&path) || !visited.insert(resolved) {
            continue;
        }
        let skill = path.join("SKILL.md");
        if checked_path(root, &skill)?.is_some() && inputs::is_file(&skill) {
            skills.push(skill);
        }
        for child in inputs::read_dir(path)? {
            let child = child?;
            if !child.file_name().to_string_lossy().starts_with('.')
                && child.file_name() != "node_modules"
                && inputs::is_dir(child.path())
            {
                pending.push(child.path());
            }
        }
    }
    skills.sort();
    Ok(skills)
}
