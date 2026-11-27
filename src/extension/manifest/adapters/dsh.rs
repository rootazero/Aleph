//! Import literal DeepSeek Harness bundle contributions.

mod tags;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value as Json};

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ImportDiagnostic, ManifestAdapter};
use crate::extension::manifest::{
    PluginManifest, inputs, parsers, sanitize_plugin_id, validate_plugin_id,
};
use crate::extension::mcp_config;
use crate::extension::types::{PluginKind, PluginOrigin, SkillType};
use crate::yaml::{Mapping, Value};

const MCP_CLIENT: &str = "@deepseek-ai/dsh-mcp-client";

pub struct DshAdapter;

impl ManifestAdapter for DshAdapter {
    fn detect(&self, dir: &Path) -> bool {
        inputs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|source| serde_json::from_str::<Json>(&source).ok())
            .is_some_and(|package| package.pointer("/dsh/bundle/patch").is_some())
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        self.parse_if_detected(dir)?
            .context("no dsh.bundle.patch declaration")
    }

    fn parse_if_detected(&self, dir: &Path) -> Result<Option<AdapterOutput>> {
        let manifest = dir.join("package.json");
        if !inputs::is_file(&manifest) {
            return Ok(None);
        }
        let root = inputs::canonicalize(dir)?;
        super::foreign::checked_path(&root, &manifest)?;
        let source = inputs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let Ok(package) = serde_json::from_str::<Json>(&source) else {
            return Ok(None);
        };
        let Some(patch) = package.pointer("/dsh/bundle/patch") else {
            return Ok(None);
        };
        let patch = patch
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .context("dsh.bundle.patch must be a nonempty package-relative path")?;
        let patch = super::foreign::relative_path(patch)?;
        let patch = super::foreign::checked_path(&root, &root.join(patch))?
            .context("dsh.bundle.patch does not exist")?;
        parse_bundle(&root, &package, &patch).map(Some)
    }

    fn format_name(&self) -> &str {
        "deepseek-harness"
    }

    fn priority(&self) -> i32 {
        61
    }
}

fn parse_bundle(root: &Path, package: &Json, patch: &Path) -> Result<AdapterOutput> {
    let name = package
        .get("name")
        .and_then(Json::as_str)
        .or_else(|| root.file_name().and_then(|name| name.to_str()))
        .context("DSH package has no name")?;
    let id = sanitize_plugin_id(name);
    validate_plugin_id(&id).map_err(anyhow::Error::msg)?;
    let source = inputs::read_to_string(patch)?;
    let rewritten = tags::preserve_explicit_tags(&source)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid DSH patch {}", patch.display()))?;
    let patch_value: Value = crate::yaml::from_str(&rewritten)
        .with_context(|| format!("invalid DSH patch {}", patch.display()))?;
    let steps = patch_value
        .as_sequence()
        .context("DSH patch must be a sequence")?;
    let mut capabilities = Vec::new();
    let mut diagnostics = Vec::new();
    let mut names = HashSet::new();
    for (step_index, step) in steps.iter().enumerate() {
        let location = format!("patch[{}]", step_index + 1);
        let Some(insert) = step
            .as_mapping()
            .filter(|step| step.len() == 1)
            .and_then(|step| step.get("insert"))
            .and_then(Value::as_sequence)
        else {
            diagnose(
                &mut diagnostics,
                patch,
                &location,
                "expected a literal insert operation",
            );
            continue;
        };
        for (index, node) in insert.iter().enumerate() {
            let location = format!("{location}.insert[{}]", index + 1);
            let Some(mapping) = node.as_mapping() else {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    "expected a literal MCP client node",
                );
                continue;
            };
            if mapping.get("name").and_then(Value::as_str) != Some(MCP_CLIENT) {
                diagnose(&mut diagnostics, patch, &location, "requires a Cordis host");
                continue;
            }
            let label = mapping
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>");
            let location = format!("{location} ({label})");
            if let Some(tag) = explicit_tag(node) {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    &format!("YAML tag {tag} requires a Cordis host"),
                );
                continue;
            }
            if mapping
                .keys()
                .any(|key| !matches!(key.as_str(), Some("id" | "name" | "config")))
            {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    "unsupported Cordis node field",
                );
                continue;
            }
            let Some(config) = mapping.get("config").and_then(Value::as_mapping) else {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    "config must be a literal mapping",
                );
                continue;
            };
            let Some(server_name) = config
                .get("serverName")
                .and_then(Value::as_str)
                .filter(|name| valid_server_name(name))
            else {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    "serverName must be a nonempty identifier",
                );
                continue;
            };
            let literal = match mcp_literal(config) {
                Ok(literal) => literal,
                Err(error) => {
                    diagnose(&mut diagnostics, patch, &location, &error.to_string());
                    continue;
                }
            };
            let cwd = match config.get("cwd") {
                Some(value) => {
                    let cwd = value.as_str().context("DSH cwd must be a literal path")?;
                    let cwd = super::foreign::relative_path(cwd)?;
                    let resolved = super::foreign::checked_path(root, &root.join(cwd))?
                        .with_context(|| format!("DSH cwd does not resolve at {location}"))?;
                    ensure!(
                        inputs::is_dir(&resolved),
                        "DSH cwd at {location} must be a directory"
                    );
                    Some(resolved)
                }
                None => None,
            };
            let as_json = Json::Object(Map::from_iter([(
                server_name.to_owned(),
                Json::Object(literal),
            )]));
            let servers = mcp_config::parse_declared_servers(&as_json.to_string(), root, &id, true)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("{} at {location}", patch.display()))?;
            if servers.is_empty() {
                diagnose(
                    &mut diagnostics,
                    patch,
                    &location,
                    "invalid literal MCP server configuration",
                );
                continue;
            }
            ensure!(
                names.insert(server_name.to_owned()),
                "duplicate DSH serverName '{server_name}' in {} at {location}",
                patch.display()
            );
            for mut server in servers {
                server.cwd = cwd.clone();
                capabilities.push(CapabilityDeclaration::McpServer(server));
            }
        }
    }
    for path in super::foreign::directory_skills(root)? {
        let name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("skill");
        match parsers::parse_skill_registration(&path, name, &id, SkillType::Skill) {
            Ok(skill) => capabilities.push(skill),
            Err(error) => diagnose(&mut diagnostics, &path, "SKILL.md", &error.to_string()),
        }
    }
    if capabilities.is_empty() {
        let reason = diagnostics
            .first()
            .map_or("no literal MCP client or skill", |item| {
                item.message.as_str()
            });
        bail!(
            "DSH package has no mountable contributions ({}): {reason}",
            patch.display()
        );
    }
    let version = package
        .get("version")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let description = package
        .get("description")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let has_mcp = capabilities
        .iter()
        .any(|entry| matches!(entry, CapabilityDeclaration::McpServer(_)));
    let kind = if has_mcp {
        PluginKind::Mcp
    } else {
        PluginKind::Static
    };
    let entry = patch.strip_prefix(root)?.to_path_buf();
    let mut manifest = PluginManifest::new(id.clone(), name.to_owned(), kind, entry);
    manifest.root_dir = root.to_path_buf();
    manifest.version = version.clone();
    manifest.description = description.clone();
    ImportDiagnostic::make_package_relative(&mut diagnostics, root);
    Ok(AdapterOutput {
        plugin_id: id.clone(),
        name: Some(name.to_owned()),
        version,
        description,
        capabilities,
        source: CapabilitySource {
            plugin_id: id,
            origin: PluginOrigin::Global,
            format: SourceFormat::DeepSeekHarness,
        },
        permissions: Vec::new(),
        runtime_manifest: Some(manifest),
        diagnostics,
    })
}

fn diagnose(diagnostics: &mut Vec<ImportDiagnostic>, path: &Path, location: &str, message: &str) {
    diagnostics.push(ImportDiagnostic {
        path: path.to_path_buf(),
        message: format!("{location}: {message}"),
    });
}

fn explicit_tag(value: &Value) -> Option<String> {
    match value {
        Value::Tagged(tagged) => {
            let label = tagged.tag.to_string();
            let original = label
                .strip_prefix("!aleph-dsh-explicit-tag-")
                .and_then(|encoded| hex::decode(encoded).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok());
            Some(original.unwrap_or(label))
        }
        Value::Sequence(values) => values.iter().find_map(explicit_tag),
        Value::Mapping(values) => values
            .iter()
            .find_map(|(key, value)| explicit_tag(key).or_else(|| explicit_tag(value))),
        _ => None,
    }
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn mcp_literal(config: &Mapping) -> Result<Map<String, Json>> {
    let transport = config
        .get("transport")
        .and_then(Value::as_str)
        .context("missing literal transport")?;
    let allowed = match transport {
        "stdio" => ["command", "args", "cwd", "env"].as_slice(),
        "streamable-http" => ["url", "headers"].as_slice(),
        other => bail!("unsupported transport '{other}'"),
    };
    let mut literal = Map::new();
    literal.insert(
        "type".into(),
        Json::String(
            if transport == "stdio" {
                "stdio"
            } else {
                "http"
            }
            .into(),
        ),
    );
    for (key, value) in config {
        let key = key
            .as_str()
            .context("expected a string configuration field")?;
        if matches!(key, "serverName" | "transport") {
            continue;
        }
        ensure!(
            allowed.contains(&key),
            "unsupported {transport} configuration field '{key}'"
        );
        if key == "cwd" {
            ensure!(value.as_str().is_some(), "cwd requires a literal string");
            continue;
        }
        let value = match key {
            "args" => Json::Array(
                value
                    .as_sequence()
                    .context("args must be a literal sequence")?
                    .iter()
                    .map(|arg| {
                        arg.as_str()
                            .map(|arg| Json::String(arg.to_owned()))
                            .context("args must contain only strings")
                    })
                    .collect::<Result<_>>()?,
            ),
            "env" | "headers" => Json::Object(
                value
                    .as_mapping()
                    .context("expected a literal mapping")?
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            key.as_str().context("expected string key")?.to_owned(),
                            Json::String(
                                value.as_str().context("expected string value")?.to_owned(),
                            ),
                        ))
                    })
                    .collect::<Result<_>>()?,
            ),
            _ => Json::String(
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .context("expected a nonempty literal string")?
                    .to_owned(),
            ),
        };
        literal.insert(key.to_owned(), value);
    }
    let required = if transport == "stdio" {
        "command"
    } else {
        "url"
    };
    ensure!(
        literal.contains_key(required),
        "{transport} requires '{required}'"
    );
    Ok(literal)
}
