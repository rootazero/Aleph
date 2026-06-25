//! Install routing over `InstallSpec`.
//!
//! `mcp_config_from_spec` (pure) builds the MCP server config, writing
//! `{{secret:NAME}}` references for secret-bearing env fields — never
//! plaintext; the reference resolves per-server at spawn. `run_install` routes
//! a resolved spec to the correct backend (MCP add / marketplace plugin copy /
//! OCI-unsupported).

use std::collections::HashMap;

use crate::extension::marketplace::{MarketplaceManager, BUILTIN_MARKETPLACE_NAME};
use crate::extension::PluginScope;
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::secrets::secret_ref;
use crate::hub::types::{ExtensionEntry, InstallSpec};
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle};

/// Build an `McpManagerConfig` from an install spec.
///
/// `secret_refs` maps an env var name to its stored vault secret name (from
/// `crate::hub::secrets::field_key`); `plain_values` maps a non-secret env var
/// name to the user-submitted value. Per field, precedence is: secret reference
/// (`{{secret:NAME}}`) → submitted plain value → declared `default`. Plaintext
/// secrets never enter the config.
pub fn mcp_config_from_spec(
    id: &str,
    name: &str,
    spec: &InstallSpec,
    secret_refs: &HashMap<String, String>,
    plain_values: &HashMap<String, String>,
) -> Result<McpManagerConfig, String> {
    match spec {
        InstallSpec::McpStdio { command, args, env } => {
            let mut env_map = HashMap::new();
            for e in env {
                if let Some(secret_name) = secret_refs.get(&e.name) {
                    env_map.insert(e.name.clone(), secret_ref(secret_name));
                } else if let Some(v) = plain_values.get(&e.name) {
                    env_map.insert(e.name.clone(), v.clone());
                } else if let Some(def) = &e.default {
                    env_map.insert(e.name.clone(), def.clone());
                }
            }
            Ok(McpManagerConfig::stdio(id, name, command)
                .with_args(args.clone())
                .with_env(env_map)
                .with_auto_start(true))
        }
        InstallSpec::McpRemote { url, .. } => {
            // Header-secret injection for remote MCP is a follow-up; build the
            // base config so the server is reachable.
            Ok(McpManagerConfig::http(id, name, url).with_auto_start(true))
        }
        InstallSpec::OciImage { .. } => {
            Err("OCI/Docker MCP containers are not installable in this version".into())
        }
        InstallSpec::GitDir { .. } => Err("GitDir installs via the plugin path, not MCP".into()),
    }
}

/// Outcome of a successful install.
#[derive(Debug, Clone)]
pub enum InstallOutcome {
    Mcp { id: String },
    Plugin { path: String },
    Skill { path: String },
}

/// Inputs the install router needs from the handler layer.
pub struct InstallContext<'a> {
    pub entry: &'a ExtensionEntry,
    pub mcp: Option<&'a McpManagerHandle>,
    pub marketplace: Option<&'a MarketplaceManager>,
    /// env/header field name -> stored vault secret name.
    pub secret_refs: HashMap<String, String>,
    /// non-secret env field name -> user-submitted plain value.
    pub plain_values: HashMap<String, String>,
}

/// Deterministic MCP server id derived from the hub entry id.
pub(crate) fn mcp_server_id(entry_id: &str) -> String {
    entry_id.replace([':', '/'], "_")
}

/// True if `command` resolves on PATH (PATHEXT-aware via `which`). Used to
/// fail an install fast rather than persist a server that can't spawn.
fn command_available(command: &str) -> bool {
    which::which(command).is_ok()
}

/// Install a single skill from a `GitDir` spec: clone the repo into an isolated
/// checkout, copy the `<subdir>` leaf into `<skills_dir>/<name>`, and stamp
/// it `Github` in the manifest (so official sync never overwrites it). Pure
/// w.r.t. the gateway — takes the resolved skills dir.
pub fn install_git_skill(
    entry: &crate::hub::types::ExtensionEntry,
    spec: &InstallSpec,
    skills_dir: &std::path::Path,
) -> Result<String, String> {
    let InstallSpec::GitDir {
        git_url, subdir, ..
    } = spec
    else {
        return Err("install_git_skill requires a GitDir spec".into());
    };
    let leaf = subdir.clone().unwrap_or_else(|| entry.name.clone());
    // Reject traversal in the SOURCE path too (not just the destination name):
    // `leaf` is joined onto the checkout to pick the copy source, so a `..`
    // segment from a crafted catalog entry could read outside the clone.
    if leaf
        .split(['/', '\\'])
        .any(|seg| seg == ".." || seg.is_empty())
    {
        return Err(format!("unsafe skill subdir '{leaf}'"));
    }
    // Last path segment is the on-disk skill name; the guard above guarantees it
    // is non-empty and free of `..`.
    let safe_name = leaf.rsplit(['/', '\\']).next().unwrap_or(&leaf).to_string();
    // Clone into an isolated per-source checkout (never the live skills dir).
    let checkout = skills_dir.join(".git-cache").join(mcp_server_id(&entry.id));
    crate::bundled::clone_or_update(git_url, &checkout)?;
    let src_leaf = checkout.join(&leaf);
    if !src_leaf.is_dir() {
        return Err(format!("subdir '{leaf}' not found in {git_url}"));
    }
    let target = skills_dir.join(&safe_name);
    crate::bundled::copy_skill_leaf(&src_leaf, &target).map_err(|e| e.to_string())?;

    // Stamp manifest as Github so official sync skips it.
    let mut manifest = crate::bundled::manifest::InstallRegistry::load(skills_dir)
        .unwrap_or_else(|| crate::bundled::manifest::InstallRegistry::new(""));
    manifest.skills.insert(
        safe_name.clone(),
        crate::bundled::manifest::SkillEntry {
            source: crate::bundled::manifest::SkillOrigin::Github,
            version: entry.version.clone(),
            url: Some(git_url.clone()),
            installed_at: None,
        },
    );
    let _ = manifest.save(skills_dir);
    Ok(target.display().to_string())
}

/// Resolve which marketplace an install entry's plugin lives in.
///
/// Hub-official plugin entries are primed with `source_id == ALEPH_HUB_ID`, but
/// the slot key is not a marketplace name — these plugins are bundled into the
/// builtin `aleph-official` marketplace, so they install from it. `"local"` means
/// "search all marketplaces by name"; any other source id is a registered peer
/// marketplace, taken verbatim.
fn plugin_marketplace_name(source_id: &str) -> Option<&str> {
    match source_id {
        ALEPH_HUB_ID => Some(BUILTIN_MARKETPLACE_NAME),
        "local" => None,
        other => Some(other),
    }
}

/// Route a resolved install spec to its backend and perform the install.
pub async fn run_install(
    spec: &InstallSpec,
    ctx: &InstallContext<'_>,
) -> Result<InstallOutcome, String> {
    match spec {
        InstallSpec::McpStdio { .. } | InstallSpec::McpRemote { .. } => {
            if let InstallSpec::McpStdio { command, .. } = spec {
                if !command_available(command) {
                    return Err(format!(
                        "required command '{command}' not found on PATH — install its runtime (e.g. node/python) and retry"
                    ));
                }
            }
            let mcp = ctx.mcp.ok_or("MCP manager unavailable")?;
            let id = mcp_server_id(&ctx.entry.id);
            let cfg = mcp_config_from_spec(
                &id,
                &ctx.entry.name,
                spec,
                &ctx.secret_refs,
                &ctx.plain_values,
            )?;
            mcp.add_server(cfg).await.map_err(|e| e.to_string())?;
            Ok(InstallOutcome::Mcp { id })
        }
        InstallSpec::OciImage { .. } => {
            Err("OCI/Docker MCP containers are not installable in this version".into())
        }
        InstallSpec::GitDir { .. } => {
            if ctx.entry.kind == crate::hub::types::ExtensionKind::Skill {
                let skills_dir = crate::utils::paths::get_config_dir()
                    .map_err(|e| e.to_string())?
                    .join("skills");
                let path = install_git_skill(ctx.entry, spec, &skills_dir)?;
                Ok(InstallOutcome::Skill { path })
            } else {
                // Plugin install via the marketplace path (SHA-256 + atomic copy).
                let marketplace = ctx.marketplace.ok_or("marketplace unavailable")?;
                let marketplace_name = plugin_marketplace_name(&ctx.entry.source_id);
                let path = marketplace.install_to_scope(
                    &ctx.entry.name,
                    marketplace_name,
                    PluginScope::User,
                    None,
                )?;
                Ok(InstallOutcome::Plugin {
                    path: path.display().to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::EnvDecl;

    #[test]
    fn stdio_spec_builds_config_with_secret_refs() {
        let spec = InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@x/y".into()],
            env: vec![
                EnvDecl {
                    name: "TOKEN".into(),
                    required: true,
                    secret: true,
                    ..Default::default()
                },
                EnvDecl {
                    name: "REGION".into(),
                    default: Some("us".into()),
                    ..Default::default()
                },
                // required, non-secret, NO default — must take the submitted value
                EnvDecl {
                    name: "ACCOUNT".into(),
                    required: true,
                    secret: false,
                    ..Default::default()
                },
            ],
        };
        let mut refs = HashMap::new();
        refs.insert("TOKEN".to_string(), "ext.mcp.x.TOKEN".to_string());
        let mut plain = HashMap::new();
        plain.insert("ACCOUNT".to_string(), "acct-123".to_string());
        let cfg = mcp_config_from_spec("x", "Y", &spec, &refs, &plain).unwrap();
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y".to_string(), "@x/y".to_string()]);
        assert_eq!(
            cfg.env.get("TOKEN").map(String::as_str),
            Some("{{secret:ext.mcp.x.TOKEN}}")
        );
        // non-secret field falls back to its declared default
        assert_eq!(cfg.env.get("REGION").map(String::as_str), Some("us"));
        // required non-secret field with no default takes the submitted value
        assert_eq!(cfg.env.get("ACCOUNT").map(String::as_str), Some("acct-123"));
        assert!(cfg.auto_start);
    }

    #[test]
    fn oci_spec_is_unsupported() {
        let spec = InstallSpec::OciImage {
            image: "mcp/y@sha256:abc".into(),
        };
        let err = mcp_config_from_spec("x", "Y", &spec, &Default::default(), &Default::default())
            .unwrap_err();
        assert!(err.contains("not installable"));
    }

    #[test]
    fn install_git_skill_clones_subdir_and_stamps_source() {
        use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};
        let tmp = tempfile::tempdir().unwrap();
        // Source repo with a `my-skill/SKILL.md` leaf.
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("my-skill")).unwrap();
        std::fs::write(src.join("my-skill").join("SKILL.md"), b"# hi").unwrap();
        let repo = git2::Repository::init(&src).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // Point HEAD at main so the clone checks out our commit (libgit2 inits
        // HEAD to an unborn `master`; without this the clone's working tree is empty).
        repo.set_head("refs/heads/main").unwrap();

        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let entry = ExtensionEntry {
            id: "aleph-hub:x/my-skill".into(),
            kind: ExtensionKind::Skill,
            category: ExtensionCategory::Developer,
            name: "my-skill".into(),
            description: "d".into(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Community,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: None,
            install_spec: None,
        };
        let spec = InstallSpec::GitDir {
            git_url: src.to_string_lossy().to_string(),
            subdir: Some("my-skill".into()),
            git_ref: Some("main".into()),
            sha256: None,
        };
        let path = install_git_skill(&entry, &spec, &skills_dir).expect("install");
        assert!(std::path::Path::new(&path).join("SKILL.md").exists());
        let manifest = crate::bundled::manifest::InstallRegistry::load(&skills_dir).unwrap();
        assert_eq!(
            manifest.skills.get("my-skill").unwrap().source,
            crate::bundled::manifest::SkillOrigin::Github
        );
    }

    #[test]
    fn absent_command_is_unavailable() {
        assert!(!command_available("definitely-not-a-real-command-xyz-123"));
    }

    #[test]
    fn plugin_marketplace_name_maps_hub_to_builtin() {
        // Hub-official plugin entries (source_id == ALEPH_HUB_ID) install from the
        // builtin "aleph-official" marketplace — NOT a marketplace literally named
        // "aleph-hub" (which does not exist). "local" searches all marketplaces;
        // any other id is a registered peer marketplace, taken verbatim.
        assert_eq!(
            plugin_marketplace_name(ALEPH_HUB_ID),
            Some(BUILTIN_MARKETPLACE_NAME)
        );
        assert_eq!(plugin_marketplace_name("aleph-hub"), Some("aleph-official"));
        assert_eq!(plugin_marketplace_name("local"), None);
        assert_eq!(plugin_marketplace_name("peer-market"), Some("peer-market"));
    }
}
