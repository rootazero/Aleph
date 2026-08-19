//! The four documented plugin manifest variables, expanded in one place.
//!
//! `${CLAUDE_PLUGIN_ROOT}` / `${ALEPH_PLUGIN_ROOT}` and their `_DATA` twins are
//! documented in `PLUGIN_SYSTEM.md` as working across "skill / agent / hook /
//! MCP config". Before 2026-08-19 that table described four surfaces and the
//! code covered one and a half:
//!
//! | surface              | `_ROOT`        | `_DATA`   |
//! |----------------------|----------------|-----------|
//! | `.mcp.json` (runtime)| yes            | yes       |
//! | hook command + env   | yes            | **no**    |
//! | skill / command body | **no**         | **no**    |
//! | agent body           | **no**         | **no**    |
//!
//! The prose gap was the expensive one. `Run ${CLAUDE_PLUGIN_ROOT}/scripts/x.py`
//! is the single most common idiom in a Claude Code `SKILL.md`, and it reached
//! the model as a literal — so the model confidently issued a `bash` call
//! against a path containing `${CLAUDE_PLUGIN_ROOT}`.
//!
//! The hook `_DATA` gap had its own shape: `${CLAUDE_PLUGIN_ROOT}` is destroyed
//! by `plugin update` (stage → backup → swap), so a hook that wanted durable
//! state had no addressable path at all — which is the exact failure
//! `plugin_data_dir`'s doc comment describes having already been fixed once,
//! for `.mcp.json`.
//!
//! There were three separate expanders: this one's ancestor in
//! `mcp_config.rs` (four variables), one in `manifest/parsers.rs` (two), and
//! one inside `hooks::substitute_variables` (two, mixed in with the runtime
//! `$TOOL_NAME` family). Splitting the `_ROOT` and `_DATA` pairs across layers
//! is what let the `_DATA` half go unimplemented while its documentation said
//! otherwise.

use std::path::{Path, PathBuf};

/// The plugin-scoped paths behind the four manifest variables.
#[derive(Debug, Clone)]
pub struct PluginVars {
    root: PathBuf,
    data: PathBuf,
}

impl PluginVars {
    /// Build the variable set for a plugin.
    ///
    /// `root` is the plugin's install directory; `data` comes from
    /// [`crate::extension::plugin_data_dir`], which lives outside the install
    /// tree precisely so it survives `plugin update`.
    #[must_use]
    pub fn new(plugin_id: &str, root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            data: super::plugin_data_dir(plugin_id),
        }
    }

    /// The plugin's persistent data directory.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The plugin's install directory.
    #[must_use]
    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    /// Expand all four variables in `value`.
    #[must_use]
    pub fn expand(&self, value: &str) -> String {
        // Cheap guard: the overwhelming majority of strings passing through
        // here (every line of every skill body) contain no variable at all.
        if !value.contains("${") {
            return value.to_string();
        }
        let root = self.root.to_string_lossy();
        let data = self.data.to_string_lossy();
        value
            .replace("${CLAUDE_PLUGIN_ROOT}", &root)
            .replace("${ALEPH_PLUGIN_ROOT}", &root)
            .replace("${CLAUDE_PLUGIN_DATA}", &data)
            .replace("${ALEPH_PLUGIN_DATA}", &data)
    }

    /// Create the data directory, but only when something asks for it.
    ///
    /// Creating it unconditionally would litter `<plugins_root>/data/` with an
    /// empty directory per installed plugin; creating it only when a plugin
    /// names it keeps the tree meaningful. Failure is a warning, not an error:
    /// a plugin whose data directory cannot be made is still worth loading,
    /// and it will fail loudly at the moment it writes.
    pub fn ensure_data_dir_if_referenced(&self, content: &str) {
        if !Self::references_data(content) {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.data) {
            tracing::warn!(
                path = %self.data.display(),
                error = %e,
                "failed to create plugin data directory"
            );
        }
    }

    /// Whether `content` asks for the data directory.
    #[must_use]
    pub fn references_data(content: &str) -> bool {
        content.contains("${CLAUDE_PLUGIN_DATA}") || content.contains("${ALEPH_PLUGIN_DATA}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_spellings_expand() {
        let vars = PluginVars::new("p", Path::new("/install/p"));
        let out = vars.expand(
            "${CLAUDE_PLUGIN_ROOT}|${ALEPH_PLUGIN_ROOT}|${CLAUDE_PLUGIN_DATA}|${ALEPH_PLUGIN_DATA}",
        );
        let parts: Vec<&str> = out.split('|').collect();
        assert_eq!(parts[0], "/install/p");
        assert_eq!(parts[1], "/install/p");
        assert_eq!(parts[2], parts[3], "both _DATA spellings are the same path");
        assert_ne!(
            parts[2], parts[0],
            "the data dir must live outside the install tree — `plugin update` swaps the latter"
        );
    }

    #[test]
    fn text_without_variables_is_returned_unchanged() {
        let vars = PluginVars::new("p", Path::new("/install/p"));
        assert_eq!(vars.expand("no variables here"), "no variables here");
    }

    #[test]
    fn references_data_only_fires_for_the_data_pair() {
        assert!(!PluginVars::references_data("${CLAUDE_PLUGIN_ROOT}/x"));
        assert!(PluginVars::references_data("${CLAUDE_PLUGIN_DATA}/x"));
        assert!(PluginVars::references_data("${ALEPH_PLUGIN_DATA}/x"));
    }
}
