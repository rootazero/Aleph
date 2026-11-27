//! Import the static resources of a Pi package.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use glob::{MatchOptions, Pattern};
use serde_json::Value;

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ImportDiagnostic, ManifestAdapter};
use crate::extension::manifest::{inputs, parsers, sanitize_plugin_id, validate_plugin_id};
use crate::extension::types::{PluginOrigin, SkillType};

pub struct PiAdapter;

impl ManifestAdapter for PiAdapter {
    fn detect(&self, dir: &Path) -> bool {
        let manifest = dir.join("package.json");
        inputs::is_dir(dir.join("prompts"))
            || inputs::read_to_string(manifest)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|package| package.get("pi").is_some())
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        self.parse_if_detected(dir)?
            .context("no Pi package manifest or prompt directory")
    }

    fn parse_if_detected(&self, dir: &Path) -> Result<Option<AdapterOutput>> {
        let manifest = dir.join("package.json");
        let conventional = inputs::is_dir(dir.join("prompts"));
        let Some(package) = read_package(dir, &manifest, conventional)? else {
            return Ok(None);
        };
        parse_pi(dir, &manifest, &package).map(Some)
    }

    fn format_name(&self) -> &str {
        "pi"
    }

    fn priority(&self) -> i32 {
        60
    }
}

/// The scanner admits a package file cheaply. The adapter turns it into a row
/// only when Pi metadata or its conventional prompt directory is present.
pub(crate) fn candidate_marker(dir: &Path) -> bool {
    dir.join("package.json").is_file() || dir.join("prompts").is_dir()
}

fn read_package(dir: &Path, manifest: &Path, conventional: bool) -> Result<Option<Value>> {
    if !inputs::is_file(manifest) {
        return Ok(conventional.then(|| Value::Object(Default::default())));
    }
    let root = inputs::canonicalize(dir)?;
    checked_path(&root, manifest)?;
    let text = inputs::read_to_string(manifest)?;
    match serde_json::from_str::<Value>(&text) {
        Ok(package) if conventional || package.get("pi").is_some() => Ok(Some(package)),
        Ok(_) => Ok(None),
        Err(error) if conventional => {
            Err(error).with_context(|| format!("invalid {}", manifest.display()))
        }
        Err(_) => Ok(None),
    }
}

fn parse_pi(dir: &Path, manifest: &Path, package: &Value) -> Result<AdapterOutput> {
    let root = inputs::canonicalize(dir)?;
    let raw_name = package
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| dir.file_name().and_then(|name| name.to_str()))
        .context("Pi package has no name")?;
    let id = sanitize_plugin_id(raw_name);
    validate_plugin_id(&id).map_err(anyhow::Error::msg)?;
    let config = package
        .get("pi")
        .map(|value| {
            value
                .as_object()
                .context("package.json pi must be an object")
        })
        .transpose()?;
    let explicit = config.is_some_and(|config| {
        ["skills", "prompts", "extensions", "themes"]
            .iter()
            .any(|kind| config.contains_key(*kind))
    });
    let mut diagnostics = Vec::new();
    for kind in ["extensions", "themes"] {
        if config
            .and_then(|config| config.get(kind))
            .is_some_and(|value| value.as_array().is_none_or(|entries| !entries.is_empty()))
        {
            diagnostic(
                &mut diagnostics,
                manifest,
                format!("pi.{kind} requires a Pi host"),
            );
        }
    }
    let mut capabilities = Vec::new();
    for (kind, skill_type) in [
        ("skills", SkillType::Skill),
        ("prompts", SkillType::Command),
    ] {
        let entries = config.and_then(|config| config.get(kind));
        let files = if explicit {
            select_files(&root, manifest, entries, kind, &mut diagnostics)?
        } else {
            let mut paths = BTreeSet::new();
            collect_files(
                &root,
                &root.join(kind),
                kind,
                true,
                &mut paths,
                &mut Vec::new(),
            )?;
            paths
        };
        for path in files {
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let name = if kind == "skills" && name == "SKILL" {
                path.parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or(name)
            } else {
                name
            };
            match parsers::parse_skill_registration(&path, name, &id, skill_type) {
                Ok(CapabilityDeclaration::Skill(mut skill)) => {
                    if kind == "prompts" {
                        skill.name = name.to_owned();
                        if skill.description.is_empty() {
                            if let Some(line) =
                                skill.content.lines().find(|line| !line.trim().is_empty())
                            {
                                skill.description = line.chars().take(60).collect();
                                if line.chars().count() > 60 {
                                    skill.description.push_str("...");
                                }
                            }
                        }
                        if let Some(token) =
                            crate::extension::template::unsupported_pi_argument(&skill.content)
                        {
                            diagnostic(
                                &mut diagnostics,
                                &path,
                                format!("unsupported Pi argument: {token}"),
                            );
                            continue;
                        }
                        skill.template_dialect = crate::extension::template::PromptDialect::Pi;
                    }
                    capabilities.push(CapabilityDeclaration::Skill(skill));
                }
                Ok(_) => unreachable!("Markdown skill reader returns a skill"),
                Err(error) => diagnostic(&mut diagnostics, &path, error.to_string()),
            }
        }
    }
    if capabilities.is_empty() {
        let reason = diagnostics
            .first()
            .map(|item| item.message.as_str())
            .unwrap_or("empty skills and prompts");
        bail!(
            "Pi package {} has no mountable contributions: {reason}",
            manifest.display()
        );
    }
    Ok(AdapterOutput {
        plugin_id: id.clone(),
        name: Some(raw_name.to_owned()),
        version: package
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        description: package
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        capabilities,
        source: CapabilitySource {
            plugin_id: id,
            origin: PluginOrigin::Global,
            format: SourceFormat::Pi,
        },
        permissions: Vec::new(),
        runtime_manifest: None,
        diagnostics,
    })
}

fn diagnostic(diagnostics: &mut Vec<ImportDiagnostic>, path: &Path, message: String) {
    diagnostics.push(ImportDiagnostic {
        path: path.to_path_buf(),
        message,
    });
}

fn checked_path(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    match inputs::canonicalize(path) {
        Ok(resolved) if resolved.starts_with(root) => Ok(Some(resolved)),
        Ok(_) => bail!("Pi resource escapes package: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
                bail!("Pi resource symlink cannot be resolved: {}", path.display());
            }
            if let Some(parent) = path.parent().filter(|parent| parent.starts_with(root)) {
                checked_path(root, parent)?;
            }
            Ok(None)
        }
        Err(error) => Err(error).with_context(|| format!("cannot resolve {}", path.display())),
    }
}

fn relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("Pi resource path must stay within the package: {value}");
    }
    Ok(path
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .collect())
}

fn collect_files(
    root: &Path,
    path: &Path,
    kind: &str,
    allow_flat_skill: bool,
    files: &mut BTreeSet<PathBuf>,
    ancestors: &mut Vec<PathBuf>,
) -> Result<()> {
    let Some(resolved) = checked_path(root, path)? else {
        return Ok(());
    };
    if inputs::is_dir(path) {
        if ancestors.contains(&resolved) {
            bail!(
                "Pi resource directory contains a symlink cycle: {}",
                path.display()
            );
        }
        let skill_marker = path.join("SKILL.md");
        if kind == "skills"
            && checked_path(root, &skill_marker)?.is_some()
            && inputs::is_file(&skill_marker)
        {
            files.insert(skill_marker);
            return Ok(());
        }
        ancestors.push(resolved);
        for entry in inputs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') || name == "node_modules" {
                continue;
            }
            let child = entry.path();
            let flat_skill = kind == "skills" && allow_flat_skill && !inputs::is_dir(&child);
            collect_files(root, &child, kind, flat_skill, files, ancestors)?;
        }
        ancestors.pop();
    } else if inputs::is_file(path)
        && path.extension().is_some_and(|ext| ext == "md")
        && (kind == "prompts"
            || allow_flat_skill
            || path.file_name().is_some_and(|name| name == "SKILL.md"))
    {
        files.insert(path.to_path_buf());
    }
    Ok(())
}

fn select_files(
    root: &Path,
    manifest: &Path,
    entries: Option<&Value>,
    kind: &str,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<BTreeSet<PathBuf>> {
    let Some(entries) = entries else {
        return Ok(BTreeSet::new());
    };
    let entries = entries
        .as_array()
        .with_context(|| format!("pi.{kind} must be an array"))?;
    let mut files = BTreeSet::new();
    let mut excludes = Vec::new();
    let mut restore = BTreeSet::new();
    let mut remove = BTreeSet::new();
    for entry in entries {
        let Some(text) = entry.as_str() else {
            diagnostic(
                diagnostics,
                manifest,
                format!("pi.{kind} resource must be a string"),
            );
            continue;
        };
        let (sign, value) = match text.as_bytes().first() {
            Some(b'!' | b'+' | b'-') => (text.as_bytes()[0], &text[1..]),
            _ => (b' ', text),
        };
        let relative = relative_path(value)?;
        let normalized = relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let has_glob = normalized.contains(['*', '?']);
        if normalized.contains(['[', ']', '{', '}']) || (matches!(sign, b'+' | b'-') && has_glob) {
            diagnostic(
                diagnostics,
                manifest,
                format!("unsupported pi.{kind} pattern: {text}"),
            );
            continue;
        }
        let prefix: PathBuf = relative
            .components()
            .take_while(|part| !part.as_os_str().to_string_lossy().contains(['*', '?']))
            .collect();
        checked_path(root, &root.join(&prefix))?;
        let pattern = Pattern::new(&normalized);
        match (sign, pattern) {
            (b'+', _) => {
                restore.insert(root.join(relative));
            }
            (b'-', _) => {
                remove.insert(root.join(relative));
            }
            (b'!', Ok(pattern)) => excludes.push(pattern),
            (_, Ok(pattern)) if has_glob => {
                collect_matches(
                    root,
                    &root.join(prefix),
                    kind,
                    &pattern,
                    &mut files,
                    &mut Vec::new(),
                )?;
            }
            (_, Ok(_)) => collect_files(
                root,
                &root.join(relative),
                kind,
                true,
                &mut files,
                &mut Vec::new(),
            )?,
            (_, Err(error)) => diagnostic(
                diagnostics,
                manifest,
                format!("unsupported pi.{kind} pattern {text}: {error}"),
            ),
        }
    }
    files.retain(|path| {
        let skill_parent = path
            .file_name()
            .is_some_and(|name| kind == "skills" && name == "SKILL.md")
            .then(|| path.parent())
            .flatten();
        let exact = |set: &BTreeSet<PathBuf>| {
            set.contains(path) || skill_parent.is_some_and(|parent| set.contains(parent))
        };
        let excluded = excludes.iter().any(|pattern| {
            let matches_resource = |candidate: &Path| {
                matches(root, pattern, candidate)
                    || candidate
                        .file_name()
                        .is_some_and(|name| pattern.matches_path(Path::new(name)))
            };
            matches_resource(path) || skill_parent.is_some_and(matches_resource)
        });
        !exact(&remove) && (exact(&restore) || !excluded)
    });
    Ok(files)
}

fn collect_matches(
    root: &Path,
    path: &Path,
    kind: &str,
    pattern: &Pattern,
    files: &mut BTreeSet<PathBuf>,
    ancestors: &mut Vec<PathBuf>,
) -> Result<()> {
    let Some(resolved) = checked_path(root, path)? else {
        return Ok(());
    };
    if matches(root, pattern, path) {
        collect_files(root, path, kind, true, files, ancestors)?;
    }
    if inputs::is_dir(path) {
        if ancestors.contains(&resolved) {
            bail!(
                "Pi resource directory contains a symlink cycle: {}",
                path.display()
            );
        }
        ancestors.push(resolved);
        for entry in inputs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with('.') && name != "node_modules" {
                collect_matches(root, &entry.path(), kind, pattern, files, ancestors)?;
            }
        }
        ancestors.pop();
    }
    Ok(())
}

fn matches(root: &Path, pattern: &Pattern, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        pattern.matches_path_with(
            relative,
            MatchOptions {
                case_sensitive: true,
                require_literal_separator: true,
                require_literal_leading_dot: true,
            },
        )
    })
}

#[cfg(test)]
mod tests;
