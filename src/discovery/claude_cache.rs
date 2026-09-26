//! Read-only discovery of the plugins Claude Code has installed.
//!
//! `~/.claude/plugins/installed_plugins.json` (schema `version: 2`) names each
//! install as `<name>@<marketplace>` with a `scope` and a `version`; the
//! plugin tree lives at `plugins/cache/<marketplace>/<name>/<version>/`. That
//! path is DERIVED from the key and the version — the file's `installPath` is
//! an absolute path on whatever machine wrote it and is never followed.
//! Aleph reads this tree and never writes under it; the enable bit lives in
//! Aleph's own `plugins.toml`, default off (`PluginOrigin::enabled_by_default`).
//!
//! Only a `scope: "user"` install is discovered. A `project` or `local`
//! install belongs to one project, and this reader has no project to bind it
//! to: as `Global` it would be visible in every project. It is skipped with a
//! `warn!`, as is an entry that names no scope.
//!
//! A file this reader does not understand is "unknown", not "no plugins": one
//! `warn!`, nothing discovered from it that load, and nothing downstream
//! forgets anything because of it — no writer drops a preference for a
//! plugin that was not discovered.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{debug, warn};

use super::paths::{
    validate_path_component, PLUGINS_DIR, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE,
};
use super::scanner::is_existing_dir_no_follow;
use super::types::{DiscoveredPath, GlobalRoot};

pub(crate) const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";
/// Below `AlephGlobal` (10): on a same-id contest an Aleph install wins
/// (`discover_and_mount` walks highest priority first and the first
/// registration keeps the id).
pub(crate) const CLAUDE_CACHE_PRIORITY: u32 = 5;
const SUPPORTED_SCHEMA_VERSION: u64 = 2;
/// The one install scope that means "this user, in every project".
const USER_SCOPE: &str = "user";
const CACHE_DIR: &str = "cache";

/// One `user`-scope install named by `installed_plugins.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedPlugin {
    /// `<name>@<marketplace>`, as the file keys it.
    pub key: String,
    pub marketplace: String,
    pub name: String,
    /// An opaque directory name — semver or a git sha.
    pub version: String,
}

impl CachedPlugin {
    /// `<claude_home>/plugins/cache/<marketplace>/<name>/<version>`.
    pub(crate) fn cache_dir(&self, claude_home: &Path) -> PathBuf {
        claude_home
            .join(PLUGINS_DIR)
            .join(CACHE_DIR)
            .join(&self.marketplace)
            .join(&self.name)
            .join(&self.version)
    }
}

#[derive(serde::Deserialize)]
struct InstalledPlugins {
    version: u64,
    /// Required: a file without it is a shape this reader does not know,
    /// not an install with no plugins. Entries stay untyped so one entry of
    /// an unexpected shape costs that entry, not the whole file.
    plugins: BTreeMap<String, Vec<Value>>,
}

/// Parse the file's text. `Err` = not a shape this reader understands (the
/// caller skips the whole source with one warn). A key that is not
/// `<name>@<marketplace>`, an install that is not `user`-scoped, a key with
/// more than one `user` install, and a segment that is not exactly one path
/// component are each skipped on their own.
pub(crate) fn parse_installed_plugins(json: &str) -> Result<Vec<CachedPlugin>, String> {
    let doc: InstalledPlugins = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if doc.version != SUPPORTED_SCHEMA_VERSION {
        return Err(format!(
            "installed_plugins.json schema version {} (this reader knows {SUPPORTED_SCHEMA_VERSION})",
            doc.version
        ));
    }
    let mut out = Vec::new();
    for (key, entries) in &doc.plugins {
        let Some((name, marketplace)) = split_key(key) else {
            debug!(
                key,
                "installed_plugins.json key is not <name>@<marketplace>; skipped"
            );
            continue;
        };
        let users: Vec<&Value> = entries.iter().filter(|e| is_user_install(key, e)).collect();
        let [entry] = users.as_slice() else {
            if !users.is_empty() {
                // Nothing says which of them Claude Code runs.
                warn!(
                    key,
                    installs = users.len(),
                    "Claude Code lists more than one user install; skipped"
                );
            }
            continue;
        };
        let Some(version) = entry
            .get("version")
            .and_then(Value::as_str)
            .filter(|v| validate_path_component(v).is_ok())
        else {
            warn!(key, "Claude Code install has no usable version; skipped");
            continue;
        };
        out.push(CachedPlugin {
            key: key.clone(),
            marketplace: marketplace.to_string(),
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    Ok(out)
}

/// `<name>@<marketplace>`, each half exactly one path component. A second
/// `@` makes the split a guess, so it is refused.
fn split_key(key: &str) -> Option<(&str, &str)> {
    let (name, marketplace) = key.split_once('@')?;
    let one_component = |s: &str| validate_path_component(s).is_ok();
    (!marketplace.contains('@') && one_component(name) && one_component(marketplace))
        .then_some((name, marketplace))
}

/// Controller ruling 1: `user` only; every other scope — and none — is
/// skipped loudly, never mapped to `Global`.
fn is_user_install(key: &str, entry: &Value) -> bool {
    let scope = entry.get("scope").and_then(Value::as_str);
    if scope == Some(USER_SCOPE) {
        return true;
    }
    warn!(
        key,
        scope = scope.unwrap_or("<none>"),
        "Claude Code install is not user-scoped; not discovered (it belongs to one project)"
    );
    false
}

/// Every cache dir named by the file that exists (not through a symlink) and
/// carries `.claude-plugin/plugin.json`. Missing file → nothing; unreadable
/// or foreign file → one `warn!` and nothing.
///
/// The manifest is required, not merely a component dir: a plugin without
/// one takes its name from its marketplace entry, and Aleph's no-manifest
/// fallback names a plugin after its directory — here the VERSION, which
/// many plugins share.
pub(crate) fn discover_claude_cache(claude_home: &Path) -> Vec<DiscoveredPath> {
    let file = claude_home.join(PLUGINS_DIR).join(INSTALLED_PLUGINS_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!(path = %file.display(), error = %e, "cannot read Claude Code's installed_plugins.json; source skipped");
            return Vec::new();
        }
    };
    let cached = match parse_installed_plugins(&text) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %file.display(), error = %e, "installed_plugins.json is not a shape this build reads; source skipped");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for plugin in cached {
        let dir = plugin.cache_dir(claude_home);
        if !is_existing_dir_no_follow(&dir) {
            debug!(key = %plugin.key, dir = %dir.display(), "cache dir absent or a symlink; skipped");
            continue;
        }
        if !dir
            .join(PLUGIN_MANIFEST_DIR)
            .join(PLUGIN_MANIFEST_FILE)
            .is_file()
        {
            debug!(key = %plugin.key, "no .claude-plugin/plugin.json (marketplace-inline plugin, e.g. LSP-only); skipped");
            continue;
        }
        // Scope = a global root of its own: per-user, no project, and
        // distinguishable from `~/.claude` proper.
        out.push(DiscoveredPath::global(
            dir,
            GlobalRoot::ClaudeCache,
            CLAUDE_CACHE_PRIORITY,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::types::{DiscoveryScope, DiscoverySource, GlobalRoot};
    use std::path::{Path, PathBuf};

    const REAL_SHAPE: &str = r#"{"version": 2, "plugins": {
      "clangd-lsp@claude-plugins-official": [{"scope": "user", "installPath": "/Users/someone/.claude/plugins/cache/claude-plugins-official/clangd-lsp/1.0.0", "version": "1.0.0", "installedAt": "2025-12-23T09:04:40.454Z", "lastUpdated": "2025-12-23T09:04:40.454Z"}],
      "superpowers@claude-plugins-official": [{"scope": "user", "installPath": "/Users/someone/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0", "version": "6.3.0", "installedAt": "2026-01-01T00:00:00.000Z", "lastUpdated": "2026-09-20T00:00:00.000Z", "gitCommitSha": "0123456789abcdef0123456789abcdef01234567"}],
      "plugin-dev@claude-plugins-official": [{"scope": "user", "installPath": "/x", "version": "c447c3207a42", "installedAt": "2026-09-20T00:00:00.000Z", "lastUpdated": "2026-09-20T00:00:00.000Z"}]
    }}"#;

    fn plant_manifest(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.json"),
            format!(r#"{{"name":"{name}"}}"#),
        )
        .unwrap();
    }

    fn write_index(home: &Path, json: &str) -> PathBuf {
        let plugins = home.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join(INSTALLED_PLUGINS_FILE), json).unwrap();
        plugins
    }

    #[test]
    fn parses_the_real_shape_and_derives_the_cache_dir_from_key_and_version() {
        let cached = parse_installed_plugins(REAL_SHAPE).unwrap();
        assert_eq!(cached.len(), 3);
        let sp = cached.iter().find(|c| c.name == "superpowers").unwrap();
        assert_eq!(sp.marketplace, "claude-plugins-official");
        assert_eq!(sp.version, "6.3.0");
        assert_eq!(
            sp.cache_dir(Path::new("/home/u/.claude")),
            PathBuf::from(
                "/home/u/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0"
            )
        );
        // A git-sha pseudo-version is an opaque directory name, not semver.
        let pd = cached.iter().find(|c| c.name == "plugin-dev").unwrap();
        assert!(pd
            .cache_dir(Path::new("/h"))
            .ends_with("plugin-dev/c447c3207a42"));
        // `installPath` is NOT what is followed.
        assert!(
            !format!("{cached:?}").contains("/Users/someone"),
            "installPath must not leak into the derived path"
        );
    }

    #[test]
    fn a_malformed_or_foreign_version_file_discovers_nothing() {
        assert!(parse_installed_plugins("{not json").is_err());
        assert!(
            parse_installed_plugins(r#"{"version": 3, "plugins": {}}"#).is_err(),
            "an unknown schema version is not ours to guess"
        );
        assert!(
            parse_installed_plugins(r#"{"version": 2}"#).is_err(),
            "a file with no `plugins` map is an unknown shape, not an empty install"
        );
        assert!(parse_installed_plugins(
            r#"{"version": 2, "plugins": {"bad-key-no-at": [{"scope": "user", "version": "1"}]}}"#
        )
        .unwrap()
        .is_empty());
    }

    /// Every segment of the derived path comes from the file, so every one of
    /// them goes through the one validator (`paths::validate_path_component`).
    /// Checking only `version` would let `../../x@m` walk out of the cache.
    #[test]
    fn a_key_or_version_that_is_not_one_path_component_is_skipped() {
        let json = r#"{"version": 2, "plugins": {
          "../../escape@m": [{"scope": "user", "version": "1.0.0"}],
          "ok@../../escape": [{"scope": "user", "version": "1.0.0"}],
          "a\\b@m": [{"scope": "user", "version": "1.0.0"}],
          "ver@m": [{"scope": "user", "version": ".."}],
          "slash@m": [{"scope": "user", "version": "1/2"}],
          "two@at@m": [{"scope": "user", "version": "1.0.0"}],
          "@m": [{"scope": "user", "version": "1.0.0"}],
          "n@": [{"scope": "user", "version": "1.0.0"}],
          "fine@m": [{"scope": "user", "version": "1.0.0"}]
        }}"#;
        let keys: Vec<String> = parse_installed_plugins(json)
            .unwrap()
            .into_iter()
            .map(|c| c.key)
            .collect();
        assert_eq!(keys, vec!["fine@m".to_string()]);
    }

    /// Controller ruling 1: Claude Code also installs at project and local
    /// scope. Such an install belongs to ONE project; mapping it to `Global`
    /// would make it visible in every project. Only `scope: "user"` is
    /// discovered; any other value — and a missing one — is skipped.
    #[test]
    fn only_a_user_scope_install_is_discovered() {
        let home = tempfile::tempdir().unwrap();
        let plugins = write_index(
            home.path(),
            r#"{"version": 2, "plugins": {
              "mine@m": [{"scope": "user", "version": "1.0.0"}],
              "theirs@m": [{"scope": "project", "projectPath": "/some/repo", "version": "1.0.0"}],
              "local@m": [{"scope": "local", "projectPath": "/some/repo", "version": "1.0.0"}],
              "unscoped@m": [{"version": "1.0.0"}]
            }}"#,
        );
        for name in ["mine", "theirs", "local", "unscoped"] {
            plant_manifest(&plugins.join(format!("cache/m/{name}/1.0.0")), name);
        }
        let found = discover_claude_cache(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("cache/m/mine/1.0.0"));
    }

    /// Two `user` installs under one key name two versions and nothing says
    /// which one Claude Code runs. Unknown is skipped, never guessed.
    #[test]
    fn a_key_with_two_user_installs_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let plugins = write_index(
            home.path(),
            r#"{"version": 2, "plugins": {
              "twice@m": [{"scope": "user", "version": "1.0.0"}, {"scope": "user", "version": "2.0.0"}],
              "once@m": [{"scope": "user", "version": "1.0.0"}]
            }}"#,
        );
        plant_manifest(&plugins.join("cache/m/twice/1.0.0"), "twice");
        plant_manifest(&plugins.join("cache/m/twice/2.0.0"), "twice");
        plant_manifest(&plugins.join("cache/m/once/1.0.0"), "once");
        let found = discover_claude_cache(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("cache/m/once/1.0.0"));
    }

    #[test]
    fn discovery_lists_only_cache_dirs_that_carry_a_manifest() {
        let home = tempfile::tempdir().unwrap();
        let plugins = write_index(home.path(), REAL_SHAPE);
        // superpowers: a real manifest. clangd-lsp: dir exists, no manifest
        // (LSP-only). plugin-dev: dir absent.
        plant_manifest(
            &plugins.join("cache/claude-plugins-official/superpowers/6.3.0"),
            "superpowers",
        );
        std::fs::create_dir_all(plugins.join("cache/claude-plugins-official/clangd-lsp/1.0.0"))
            .unwrap();
        let found = discover_claude_cache(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].scope,
            DiscoveryScope::Global(GlobalRoot::ClaudeCache)
        );
        assert_eq!(
            found[0].source(),
            DiscoverySource::ClaudeCache,
            "the derived label classify() reads"
        );
        assert_eq!(found[0].priority, CLAUDE_CACHE_PRIORITY);
        assert!(found[0].path.ends_with("superpowers/6.3.0"));
    }

    /// A cache dir with components but no `.claude-plugin/plugin.json` is a
    /// marketplace-inline plugin: its name lives in the marketplace entry,
    /// not in the tree. Aleph's no-manifest fallback names a plugin after its
    /// directory — here the VERSION (`1.0.0`), shared by many plugins — so it
    /// is skipped rather than listed under a name that is not its own.
    #[test]
    fn a_cache_dir_without_a_plugin_json_is_skipped_even_with_components() {
        let home = tempfile::tempdir().unwrap();
        let plugins = write_index(
            home.path(),
            r#"{"version": 2, "plugins": {"inline@m": [{"scope": "user", "version": "1.0.0"}]}}"#,
        );
        std::fs::create_dir_all(plugins.join("cache/m/inline/1.0.0/commands")).unwrap();
        assert!(discover_claude_cache(home.path()).is_empty());
    }

    /// A cache dir that is a symlink is not enumerated as if it lived in the
    /// cache — the same rule `scan_plugin_parent` applies.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_cache_dir_is_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        plant_manifest(elsewhere.path(), "linked");
        let plugins = write_index(
            home.path(),
            r#"{"version": 2, "plugins": {"linked@m": [{"scope": "user", "version": "1.0.0"}]}}"#,
        );
        std::fs::create_dir_all(plugins.join("cache/m/linked")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), plugins.join("cache/m/linked/1.0.0")).unwrap();
        assert!(discover_claude_cache(home.path()).is_empty());
    }

    #[test]
    fn a_missing_file_is_a_silent_no_op() {
        let home = tempfile::tempdir().unwrap();
        assert!(discover_claude_cache(home.path()).is_empty());
    }
}
