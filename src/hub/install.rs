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
use crate::hub::types::{ExtensionEntry, InstallSpec, McpTransport};
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
        InstallSpec::McpRemote {
            url,
            transport,
            headers,
        } => {
            // Declared headers are auth material (bearer tokens, API keys). Store
            // the `{{secret:NAME}}` reference — never the plaintext — exactly as
            // the stdio env path does; the actor resolves it per-connect.
            let mut header_map = HashMap::new();
            for h in headers {
                if let Some(secret_name) = secret_refs.get(&h.name) {
                    header_map.insert(h.name.clone(), secret_ref(secret_name));
                } else if let Some(v) = plain_values.get(&h.name) {
                    header_map.insert(h.name.clone(), v.clone());
                }
            }
            let base = match transport {
                McpTransport::Sse => McpManagerConfig::sse(id, name, url),
                // StreamableHttp and (defensively) Stdio-on-a-remote-spec both
                // speak the HTTP transport.
                McpTransport::StreamableHttp | McpTransport::Stdio => {
                    McpManagerConfig::http(id, name, url)
                }
            };
            Ok(base.with_headers(header_map).with_auto_start(true))
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
/// checkout at the pinned `git_ref` (default branch when absent), verify the
/// declared `sha256` against the leaf that is about to be copied, copy the
/// `<subdir>` leaf into `<skills_dir>/<name>`, and stamp it `Github` in the
/// manifest (so official sync never overwrites it). Pure w.r.t. the gateway —
/// takes the resolved skills dir.
///
/// Both pins are enforced *before* any write: an unresolvable `git_ref` or a
/// digest mismatch aborts with nothing installed. The trust disclosure shows the
/// user this `sha256` and the install response echoes it as `pin.sha256`, so it
/// has to mean something.
pub fn install_git_skill(
    entry: &crate::hub::types::ExtensionEntry,
    spec: &InstallSpec,
    skills_dir: &std::path::Path,
) -> Result<String, String> {
    let InstallSpec::GitDir {
        git_url,
        subdir,
        git_ref,
        sha256,
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
    // Restrict accepted git URL schemes: HTTPS (`https://`) and SSH-style scp
    // form (`git@host:…`). Anything else (file://, ssh://, plain /local/path,
    // gopher://) is a foothold into the sandbox and gets rejected at install
    // time rather than at clone time (where the failure mode is opaque).
    if !(git_url.starts_with("https://")
        || (git_url.starts_with("git@") && git_url.contains(':')))
    {
        return Err(format!(
            "git_url must be https:// or git@<host>:..., got '{git_url}'"
        ));
    }
    // Clone into an isolated per-source checkout (never the live skills dir),
    // at the pinned revision when the catalog declares one.
    let checkout = skills_dir.join(".git-cache").join(mcp_server_id(&entry.id));
    // Keep the clone call inside a closure so we can clean up the
    // `.git-cache/<id>` directory on any error path that follows it.
    let clone_result = crate::bundled::clone_or_update_at(git_url, &checkout, git_ref.as_deref());
    if let Err(e) = clone_result {
        let _ = std::fs::remove_dir_all(&checkout);
        return Err(e.to_string());
    }
    let src_leaf = checkout.join(&leaf);
    if !src_leaf.is_dir() {
        let _ = std::fs::remove_dir_all(&checkout);
        return Err(format!("subdir '{leaf}' not found in {git_url}"));
    }
    // Enforce the content pin before the first write.
    if let Err(e) =
        crate::extension::marketplace::installer::verify_plugin_integrity(&src_leaf, sha256.as_deref())
    {
        let _ = std::fs::remove_dir_all(&checkout);
        return Err(e.to_string());
    }
    // Atomic stage-then-rename: copy into a fresh staging directory, then
    // rename onto the target. A mid-copy failure leaves the staging dir as
    // garbage (cleaned by official sync) and the existing target untouched.
    let target = skills_dir.join(&safe_name);
    let staging = skills_dir
        .join(".staging")
        .join(format!("{}-{nonce}", safe_name, nonce = mcp_server_id(&entry.id)));
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(e) = crate::bundled::copy_skill_leaf(&src_leaf, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.to_string());
    }
    if let Err(e) = std::fs::rename(&staging, &target) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.to_string());
    }

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
    // Best-effort: drop the per-entry `.git-cache/<id>` clone after a successful
    // install. Re-installing would just re-clone; the on-disk leak from leaving
    // it forever is the documented pathology (review/hub-statics).
    let _ = std::fs::remove_dir_all(&checkout);
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
    use crate::hub::types::{EnvDecl, HeaderDecl};

    fn remote(transport: McpTransport, headers: Vec<HeaderDecl>) -> InstallSpec {
        InstallSpec::McpRemote {
            url: "https://mcp.example.com/mcp".into(),
            transport,
            headers,
        }
    }

    /// Regression: a declared secret header used to be collected from the user,
    /// written to the vault, and then **dropped** — the server was dialed with no
    /// auth at all while the install reported success.
    #[test]
    fn remote_spec_carries_secret_header_as_a_reference() {
        let spec = remote(
            McpTransport::StreamableHttp,
            vec![
                HeaderDecl {
                    name: "Authorization".into(),
                    secret: true,
                },
                HeaderDecl {
                    name: "X-Region".into(),
                    secret: false,
                },
            ],
        );
        let mut refs = HashMap::new();
        refs.insert(
            "Authorization".to_string(),
            "ext.mcp.x.Authorization".to_string(),
        );
        let mut plain = HashMap::new();
        plain.insert("X-Region".to_string(), "us".to_string());

        let cfg = mcp_config_from_spec("x", "Y", &spec, &refs, &plain).unwrap();
        assert_eq!(
            cfg.headers.get("Authorization").map(String::as_str),
            Some("{{secret:ext.mcp.x.Authorization}}"),
            "the reference is persisted, never the plaintext"
        );
        assert_eq!(cfg.headers.get("X-Region").map(String::as_str), Some("us"));
        assert_eq!(cfg.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert!(cfg.auto_start);
    }

    /// A header the user did not supply is simply absent — never an empty string,
    /// which would look like a real (wrong) credential to the server.
    #[test]
    fn remote_spec_omits_unsupplied_headers() {
        let spec = remote(
            McpTransport::StreamableHttp,
            vec![HeaderDecl {
                name: "Authorization".into(),
                secret: true,
            }],
        );
        let cfg = mcp_config_from_spec("x", "Y", &spec, &Default::default(), &Default::default())
            .unwrap();
        assert!(cfg.headers.is_empty());
    }

    /// An SSE catalog entry must be dialed as SSE. It used to be installed as
    /// HTTP regardless of its declared transport.
    #[test]
    fn remote_sse_spec_keeps_its_transport() {
        use crate::mcp::manager::McpTransportType;
        let cfg = mcp_config_from_spec(
            "x",
            "Y",
            &remote(McpTransport::Sse, vec![]),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        assert!(matches!(cfg.transport, McpTransportType::Sse));

        let cfg = mcp_config_from_spec(
            "x",
            "Y",
            &remote(McpTransport::StreamableHttp, vec![]),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        assert!(matches!(cfg.transport, McpTransportType::Http));
    }

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

    /// Build a source repo containing `my-skill/SKILL.md` on `main`, plus the
    /// skills dir and the entry the install path expects.
    fn git_skill_fixture(
        tmp: &std::path::Path,
    ) -> (
        String,
        std::path::PathBuf,
        crate::hub::types::ExtensionEntry,
    ) {
        use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};
        let src = tmp.join("src");
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

        let skills_dir = tmp.join("skills");
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
        (src.to_string_lossy().to_string(), skills_dir, entry)
    }

    fn git_dir_spec(git_url: &str, sha256: Option<&str>) -> InstallSpec {
        InstallSpec::GitDir {
            git_url: git_url.to_string(),
            subdir: Some("my-skill".into()),
            git_ref: Some("main".into()),
            sha256: sha256.map(str::to_owned),
        }
    }

    #[test]
    fn install_git_skill_clones_subdir_and_stamps_source() {
        let tmp = tempfile::tempdir().unwrap();
        let (url, skills_dir, entry) = git_skill_fixture(tmp.path());
        let path =
            install_git_skill(&entry, &git_dir_spec(&url, None), &skills_dir).expect("install");
        assert!(std::path::Path::new(&path).join("SKILL.md").exists());
        let manifest = crate::bundled::manifest::InstallRegistry::load(&skills_dir).unwrap();
        assert_eq!(
            manifest.skills.get("my-skill").unwrap().source,
            crate::bundled::manifest::SkillOrigin::Github
        );
    }

    /// The `sha256` pin is shown to the user in the trust disclosure and echoed
    /// back as `pin.sha256`, so it has to be enforced — and enforced *before* the
    /// first write, leaving nothing installed on mismatch.
    #[test]
    fn install_git_skill_rejects_a_sha256_mismatch_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let (url, skills_dir, entry) = git_skill_fixture(tmp.path());
        let bad = "0".repeat(64);
        let err = install_git_skill(&entry, &git_dir_spec(&url, Some(&bad)), &skills_dir)
            .expect_err("a mismatched pin must fail the install");
        assert!(err.contains("integrity check failed"), "{err}");
        assert!(
            !skills_dir.join("my-skill").exists(),
            "nothing may be installed when the pin does not match"
        );
    }

    #[test]
    fn install_git_skill_accepts_the_matching_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let (url, skills_dir, entry) = git_skill_fixture(tmp.path());
        // First install with no pin to materialize the checkout, then compute the
        // digest of the exact leaf the install copies from.
        install_git_skill(&entry, &git_dir_spec(&url, None), &skills_dir).expect("seed install");
        let leaf = skills_dir
            .join(".git-cache")
            .join(mcp_server_id(&entry.id))
            .join("my-skill");
        let digest =
            crate::extension::marketplace::installer::directory_digest(&leaf).expect("digest");

        let fresh = tempfile::tempdir().unwrap();
        let (url2, skills_dir2, entry2) = git_skill_fixture(fresh.path());
        let path = install_git_skill(&entry2, &git_dir_spec(&url2, Some(&digest)), &skills_dir2)
            .expect("matching pin must install");
        assert!(std::path::Path::new(&path).join("SKILL.md").exists());
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
