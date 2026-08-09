//! Template loader: user-directory overrides over built-in TOML files baked
//! into the binary via `include_str!`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::types::{TeamTemplate, TeamTemplateError};

// --------------------------------------------------------------------------
// Built-in template registry
// --------------------------------------------------------------------------

/// Embedded built-in templates. Each entry is `(name, toml_source)`.
/// Adding a builtin is a single line here plus a sibling `.toml` file.
const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    ("software-dev", include_str!("builtin/software-dev.toml")),
    (
        "research-paper",
        include_str!("builtin/research-paper.toml"),
    ),
    ("code-review", include_str!("builtin/code-review.toml")),
    ("strategy-room", include_str!("builtin/strategy-room.toml")),
];

// --------------------------------------------------------------------------
// Public entry points
// --------------------------------------------------------------------------

/// Default user-templates directory under the conventional Aleph home.
/// Honours `$ALEPH_HOME` for parity with the `project_root` + dotfiles code.
pub fn default_user_dir() -> PathBuf {
    // Single source: `get_config_dir` already IS the `ALEPH_HOME` -> `~/.aleph`
    // rule (see `canvas_io::aleph_home` for why the copies were collapsed).
    let base = crate::utils::paths::get_config_dir().unwrap_or_else(|_| PathBuf::from("."));
    base.join("teams").join("templates")
}

/// Parse a single TOML file into a validated [`TeamTemplate`].
fn parse_template(name: &str, source: &str) -> Result<TeamTemplate, TeamTemplateError> {
    let mut tpl: TeamTemplate =
        toml::from_str(source).map_err(|e| TeamTemplateError::Parse(e.to_string()))?;
    // Force template.name to match the registered key (file stem or builtin id)
    // so users can't ship two files claiming the same name.
    tpl.name = name.to_string();
    tpl.validate()?;
    Ok(tpl)
}

/// Load a template by `name`. Resolution order: user dir → builtin.
pub fn load_template(name: &str) -> Result<TeamTemplate, TeamTemplateError> {
    let registry = TemplateRegistry::discover(&default_user_dir());
    registry.get(name).cloned().ok_or_else(|| {
        TeamTemplateError::NotFound(format!(
            "no template named `{name}` (try: {})",
            registry.names().collect::<Vec<_>>().join(", ")
        ))
    })
}

// --------------------------------------------------------------------------
// TemplateRegistry
// --------------------------------------------------------------------------

/// All templates discoverable on this host. Built once per RPC / tool call —
/// templates are small and reads are infrequent, so we don't bother caching.
#[derive(Debug, Default)]
pub struct TemplateRegistry {
    /// Sorted by name for deterministic listing output.
    entries: BTreeMap<String, TeamTemplate>,
}

impl TemplateRegistry {
    /// Discover all templates: built-in first, user-dir overrides on top.
    pub fn discover(user_dir: &Path) -> Self {
        let mut entries: BTreeMap<String, TeamTemplate> = BTreeMap::new();

        // 1. Built-ins.
        for (name, source) in BUILTIN_TEMPLATES {
            match parse_template(name, source) {
                Ok(tpl) => {
                    entries.insert((*name).to_string(), tpl);
                }
                Err(e) => {
                    tracing::error!(
                        template = %name,
                        error = %e,
                        "templates: built-in template failed to parse (this is a compile-time bug)"
                    );
                }
            }
        }

        // 2. User overrides. Failures are logged but never abort discovery —
        //    one bad file shouldn't kneecap the picker.
        if user_dir.exists() {
            match std::fs::read_dir(user_dir) {
                Ok(rd) => {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                            continue;
                        }
                        let name = match path.file_stem().and_then(|s| s.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        match std::fs::read_to_string(&path) {
                            Ok(source) => match parse_template(&name, &source) {
                                Ok(tpl) => {
                                    entries.insert(name, tpl);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        path = %path.display(),
                                        error = %e,
                                        "templates: user template failed to parse; skipping"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::warn!(
                                    path = %path.display(),
                                    error = %e,
                                    "templates: user template read failed; skipping"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        dir = %user_dir.display(),
                        error = %e,
                        "templates: user template directory read failed"
                    );
                }
            }
        }

        Self { entries }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TeamTemplate> {
        self.entries.get(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &TeamTemplate> {
        self.entries.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_all_builtins() {
        let r = TemplateRegistry::discover(std::path::Path::new("/nonexistent-dir-xyz"));
        // 4 built-ins must always be discoverable.
        assert_eq!(r.names().count(), 4);
        assert!(r.get("software-dev").is_some());
        assert!(r.get("research-paper").is_some());
        assert!(r.get("code-review").is_some());
        assert!(r.get("strategy-room").is_some());
    }

    #[test]
    fn user_dir_overrides_builtin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("software-dev.toml");
        let custom = r#"
description = "Custom override"
[leader]
id = "lead"
[[members]]
id = "worker"
"#;
        std::fs::write(&path, custom).expect("write");

        let r = TemplateRegistry::discover(tmp.path());
        let tpl = r.get("software-dev").expect("present");
        assert_eq!(tpl.description, "Custom override");
        // Note: name is always forced to the registry key, so the user can't
        // smuggle a different name via the TOML body.
        assert_eq!(tpl.name, "software-dev");
    }

    #[test]
    fn malformed_user_template_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("broken.toml"), "this is not valid toml [[[")
            .expect("write");

        let r = TemplateRegistry::discover(tmp.path());
        // Builtins still load.
        assert_eq!(r.names().count(), 4);
        // Broken file is silently dropped.
        assert!(r.get("broken").is_none());
    }

    #[test]
    fn load_template_returns_not_found_with_hint() {
        let err = load_template("ghost-template");
        match err {
            Err(TeamTemplateError::NotFound(msg)) => {
                assert!(msg.contains("software-dev"), "hint should list builtins");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
