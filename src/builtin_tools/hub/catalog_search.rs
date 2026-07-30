//! `hub_catalog_search` — browse/search the Aleph Hub catalog.
//!
//! This is the entry point to the whole Hub chain. `hub_resolve_spec` and
//! `hub_install_run` both take an `entry_id`, and until this tool existed there
//! was no way for a model to obtain one: `hub_catalog_sync` returns counts, the
//! catalog cache had no reader, and the Panel does its own filtering client-side.
//! "Install the GitHub MCP server for me" was therefore unanswerable even though
//! every downstream piece worked.
//!
//! Results carry the two facts that decide what to do next: `requires_config`
//! (the install will need user-supplied values — get them from
//! `hub_resolve_spec`'s env/header declarations) and `needs_user_consent` (the
//! install cannot complete agent-side and must go through the Panel's trust
//! flow), so the model does not attempt installs that are guaranteed to bounce.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::trust::build_disclosure;
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind};
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AlephTool;

/// Default result cap. Generous enough to browse a category, small enough that a
/// bare search does not flood the context.
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct HubCatalogSearchArgs {
    /// Free text matched against name, description, tags and author.
    /// Omit to browse everything (subject to the other filters).
    #[serde(default)]
    pub query: Option<String>,
    /// Restrict to one kind of extension.
    #[serde(default)]
    pub kind: Option<ExtensionKind>,
    /// Restrict to one functional category.
    #[serde(default)]
    pub category: Option<ExtensionCategory>,
    /// `true` → only already-installed entries; `false` → only not-installed.
    /// Omit for both.
    #[serde(default)]
    pub installed: Option<bool>,
    /// Max results (default 20, capped at 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One catalog result, trimmed to what a decision needs.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogHit {
    /// Pass this verbatim to `hub_resolve_spec` / `hub_install_run`.
    pub id: String,
    pub kind: &'static str,
    pub category: &'static str,
    pub name: String,
    pub description: String,
    pub trust_tier: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    pub installed: bool,
    pub enabled: bool,
    /// The installed copy is older than what the catalog now offers.
    pub update_available: bool,
    /// Install needs user-supplied values (API keys, tokens, endpoints).
    pub requires_config: bool,
    /// Install cannot be completed agent-side; it needs a user gesture in the
    /// Panel (anything that writes executable code, or a risky command spec).
    pub needs_user_consent: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubCatalogSearchOutput {
    pub hits: Vec<CatalogHit>,
    /// How many entries matched before the limit was applied — so a truncated
    /// result never reads as "that is all there is".
    pub total_matched: usize,
    pub truncated: bool,
}

#[derive(Clone)]
pub struct HubCatalogSearchTool {
    pub cache: Arc<CatalogCache>,
    /// Live MCP handle, used only to resolve installed-state. `None` → MCP
    /// entries simply report `installed: false`.
    pub mcp: Option<McpManagerHandle>,
}

fn to_hit(e: &ExtensionEntry) -> CatalogHit {
    // `needs_user_consent` is only knowable with a spec; without one the entry is
    // not installable anyway, so reporting `false` cannot mislead an install.
    let needs_user_consent = e.install_spec.as_ref().is_some_and(|spec| {
        let d = build_disclosure(e, spec);
        crate::builtin_tools::hub::install_run::requires_user_consent(d.ack_required, spec)
    });
    CatalogHit {
        id: e.id.clone(),
        kind: e.kind.as_str(),
        category: e.category.as_str(),
        name: e.name.clone(),
        description: e.description.clone(),
        trust_tier: e.trust_tier.as_str(),
        version: e.version.clone(),
        repo_url: e.repo_url.clone(),
        installed: e.installed,
        enabled: e.enabled,
        update_available: e.update_available,
        requires_config: e.requires_config,
        needs_user_consent,
    }
}

#[async_trait]
impl AlephTool for HubCatalogSearchTool {
    const NAME: &'static str = "hub_catalog_search";
    const DESCRIPTION: &'static str = "Search or browse the Aleph Hub catalog of installable \
         extensions (MCP servers, skills, plugins). This is how you obtain the `entry_id` that \
         `hub_resolve_spec` and `hub_install_run` require. Reads the local cache — works offline; \
         run `hub_catalog_sync` first if results look stale. Each hit reports whether it is \
         already installed, whether a newer version is available, whether installing needs \
         user-supplied config, and whether the install requires a user gesture in the Panel \
         (`needs_user_consent`) rather than completing agent-side.";
    type Args = HubCatalogSearchArgs;
    type Output = HubCatalogSearchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = CatalogFilter {
            kind: args.kind,
            category: args.category,
            query: args.query,
            ..Default::default()
        };
        let mut entries = self
            .cache
            .query(&filter)
            .await
            .map_err(|e| AlephError::other(format!("catalog query failed: {e}")))?;

        // Reconcile against the live backends + the provenance ledger, so
        // `installed` / `update_available` mean the same thing here as on a
        // Panel card — same functions, not a second implementation. Both reads
        // are best-effort by design.
        let installed = crate::hub::reconcile::collect_installed(self.mcp.clone()).await;
        let origins = self.cache.origins().await.unwrap_or_else(|e| {
            tracing::warn!("hub_catalog_search: install origin read failed: {e}");
            Vec::new()
        });
        crate::hub::reconcile::mark_installed_state(&mut entries, &installed, &origins);

        if let Some(want) = args.installed {
            entries.retain(|e| e.installed == want);
        }
        let total_matched = entries.len();
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let truncated = total_matched > limit;
        entries.truncate(limit);

        Ok(HubCatalogSearchOutput {
            hits: entries.iter().map(to_hit).collect(),
            total_matched,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{InstallSpec, TrustTier};

    fn entry(id: &str, name: &str, desc: &str, kind: ExtensionKind) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind,
            category: ExtensionCategory::Developer,
            name: name.into(),
            description: desc.into(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Verified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: Some("Aleph Hub".into()),
            install_spec: Some(InstallSpec::McpStdio {
                command: "npx".into(),
                args: vec![],
                env: vec![],
            }),
        }
    }

    async fn tool_with(entries: Vec<ExtensionEntry>) -> HubCatalogSearchTool {
        let cache = CatalogCache::open_in_memory().unwrap();
        cache.upsert_many(&entries).await.unwrap();
        HubCatalogSearchTool {
            cache: Arc::new(cache),
            mcp: None,
        }
    }

    #[tokio::test]
    async fn search_returns_the_entry_id_install_needs() {
        let tool = tool_with(vec![
            entry(
                "aleph-hub:gh",
                "GitHub",
                "Manage repositories.",
                ExtensionKind::Mcp,
            ),
            entry(
                "aleph-hub:pg",
                "Postgres",
                "Query databases.",
                ExtensionKind::Mcp,
            ),
        ])
        .await;
        let out = tool
            .call(HubCatalogSearchArgs {
                query: Some("repositories".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.total_matched, 1);
        assert_eq!(out.hits[0].id, "aleph-hub:gh");
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn kind_filter_narrows_results() {
        let tool = tool_with(vec![
            entry("aleph-hub:a", "Alpha", "", ExtensionKind::Mcp),
            entry("aleph-hub:b", "Beta", "", ExtensionKind::Skill),
        ])
        .await;
        let out = tool
            .call(HubCatalogSearchArgs {
                kind: Some(ExtensionKind::Skill),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].kind, "skill");
    }

    /// A capped result must say so, or the model reads 20-of-40 as "all of them".
    #[tokio::test]
    async fn limit_is_reported_not_silently_applied() {
        let many: Vec<ExtensionEntry> = (0..5)
            .map(|i| {
                entry(
                    &format!("aleph-hub:{i}"),
                    &format!("E{i}"),
                    "",
                    ExtensionKind::Mcp,
                )
            })
            .collect();
        let tool = tool_with(many).await;
        let out = tool
            .call(HubCatalogSearchArgs {
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.hits.len(), 2);
        assert_eq!(out.total_matched, 5);
        assert!(out.truncated);
    }

    /// A GitDir (skill/plugin) entry writes code to disk, so the model is told up
    /// front that this install needs a user gesture.
    #[tokio::test]
    async fn git_dir_entries_report_needs_user_consent() {
        let mut e = entry("aleph-hub:s", "Skill", "", ExtensionKind::Skill);
        e.install_spec = Some(InstallSpec::GitDir {
            git_url: "https://github.com/a/b".into(),
            subdir: None,
            git_ref: None,
            sha256: None,
        });
        let tool = tool_with(vec![e]).await;
        let out = tool.call(HubCatalogSearchArgs::default()).await.unwrap();
        assert!(out.hits[0].needs_user_consent);
    }

    #[tokio::test]
    async fn limit_zero_is_clamped_to_one_result() {
        let tool = tool_with(vec![entry("aleph-hub:a", "Alpha", "", ExtensionKind::Mcp)]).await;
        let out = tool
            .call(HubCatalogSearchArgs {
                limit: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.hits.len(), 1);
    }
}
