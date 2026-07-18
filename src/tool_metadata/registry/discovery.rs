//! Smart Tool Discovery Methods
//!
//! Methods for generating tool indices and smart prompts.

use crate::sync_primitives::Arc;

use super::super::types::{ToolIndex, ToolIndexCategory, ToolIndexEntry, UnifiedTool};
use super::health::{HealthSnapshot, ToolHealthCache};
use super::types::ToolStorage;

/// Smart discovery functionality for `ToolCatalog`
pub struct ToolDiscovery {
    tools: ToolStorage,
}

impl ToolDiscovery {
    /// Create a new discovery handler with the given storage
    pub const fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// Generate tool list for LLM prompt
    ///
    /// Returns a markdown-formatted list of all active tools
    /// suitable for injection into L3 router system prompt.
    pub async fn to_prompt_block(&self, health: &HealthSnapshot) -> String {
        let tools = self.tools.read().await;
        let mut lines: Vec<String> = tools
            .values()
            .filter(|t| t.is_active)
            .filter(|t| health.is_healthy(&t.name))
            .map(|t| t.to_prompt_line())
            .collect();

        lines.sort(); // Alphabetical order
        lines.join("\n")
    }

    /// Generate lightweight tool index for smart discovery
    ///
    /// Creates a `ToolIndex` containing minimal metadata for all tools.
    /// This is used for token-efficient LLM prompt injection.
    ///
    /// # Arguments
    ///
    /// * `core_tools` - List of tool names that should be marked as core
    /// * `health` - snapshot of the runtime health cache; unhealthy tools
    ///   are skipped so the LLM never sees them
    pub async fn generate_tool_index(
        &self,
        core_tools: &[&str],
        health: &HealthSnapshot,
    ) -> ToolIndex {
        let tools = self.tools.read().await;
        let mut index = ToolIndex::new();

        for tool in tools
            .values()
            .filter(|t| t.is_active)
            .filter(|t| health.is_healthy(&t.name))
        {
            let entry = tool.to_index_entry(core_tools);
            index.add(entry);
        }

        index
    }

    /// Generate smart prompt with tool index + filtered full schemas
    ///
    /// This is the main entry point for smart tool discovery.
    /// Returns a prompt that contains:
    /// 1. Full schemas for core tools
    /// 2. Full schemas for filtered tools (if any)
    /// 3. Index-only entries for remaining tools
    ///
    /// # Arguments
    ///
    /// * `core_tools` - Tools that always have full schema
    /// * `filtered_tools` - Additional tools to include with full schema
    ///
    /// # Returns
    ///
    /// Tuple of (`tool_definitions`, `tool_index_prompt`)
    /// - `tool_definitions`: Vec of tools with full schema for function calling
    /// - `tool_index_prompt`: Markdown prompt for index-only tools
    pub async fn generate_smart_prompt(
        &self,
        core_tools: &[&str],
        filtered_tools: &[&str],
        health: &HealthSnapshot,
    ) -> (Vec<UnifiedTool>, String) {
        let tools = self.tools.read().await;

        let mut full_schema_tools = Vec::new();
        let mut index = ToolIndex::new();

        for tool in tools
            .values()
            .filter(|t| t.is_active)
            .filter(|t| health.is_healthy(&t.name))
        {
            let is_core = core_tools.contains(&tool.name.as_str());
            let is_filtered = filtered_tools.contains(&tool.name.as_str());

            if is_core || is_filtered {
                // Include with full schema
                full_schema_tools.push(tool.clone());
            } else {
                // Index only
                let entry = tool.to_index_entry(core_tools);
                index.add(entry);
            }
        }

        // Sort full schema tools by priority
        full_schema_tools.sort_by(|a, b| {
            let priority_a = a.source.priority();
            let priority_b = b.source.priority();
            priority_b.cmp(&priority_a).then(a.name.cmp(&b.name))
        });

        (full_schema_tools, index.to_prompt())
    }

    /// Trigger detached background refreshes for every **registered probe**
    /// whose cache entry is missing or expired. Callers typically run this
    /// just before a prompt assembly so the *next* turn sees fresh
    /// results; the current turn still uses whatever the snapshot held.
    ///
    /// Iterates the cache's registered probes rather than the catalog's tool
    /// list: a probe is the only thing that produces a gating decision, so
    /// every registered probe must be evaluated regardless of whether a
    /// same-named tool happens to live in *this* catalog. This matters when a
    /// probe is keyed by a tool name that is not an entry in *this* catalog
    /// (e.g. a probe registered on a shared cache consulted by a subagent
    /// catalog) — the previous catalog-driven scan silently skipped those
    /// probes and they never gated.
    ///
    /// This is intentionally fire-and-forget: a slow probe never blocks
    /// prompt construction. The `tokio::spawn` is detached because the
    /// cache itself owns the result via `ArcSwap`.
    pub async fn trigger_health_refresh(&self, cache: &Arc<ToolHealthCache>) {
        for name in cache.probe_names() {
            if cache.needs_refresh(&name) {
                let cache = Arc::clone(cache);
                tokio::spawn(async move {
                    cache.refresh(&name).await;
                });
            }
        }
    }

    /// Get full tool definition by name
    ///
    /// Used by the `get_tool_schema` meta tool to provide on-demand schema.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool command name
    ///
    /// # Returns
    ///
    /// Full `UnifiedTool` if found, None otherwise. When several tools match
    /// (e.g. one whose `name` equals the query and another whose `id` ends
    /// with `:{name}`), selection is deterministic: an exact name match wins,
    /// then highest source priority, then the lexicographically smallest id.
    /// Without this ordering `HashMap` iteration order would make the meta
    /// tool return an arbitrary (possibly wrong) tool's schema.
    pub async fn get_tool_definition(&self, name: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.name == name || t.id.rsplit_once(':').is_some_and(|(_, n)| n == name))
            .max_by(|a, b| {
                let a_exact = a.name == name;
                let b_exact = b.name == name;
                a_exact
                    .cmp(&b_exact)
                    .then_with(|| a.source.priority().cmp(&b.source.priority()))
                    .then_with(|| b.id.cmp(&a.id)) // smaller id wins on tie
            })
            .cloned()
    }

    /// List tools by category for the `list_tools` meta tool
    ///
    /// # Arguments
    ///
    /// * `category` - Optional category filter (core, builtin, mcp, skill, custom)
    ///
    /// # Returns
    ///
    /// Vector of tool index entries matching the category
    pub async fn list_tools_by_category(
        &self,
        category: Option<&str>,
        health: &HealthSnapshot,
    ) -> Vec<ToolIndexEntry> {
        let tools = self.tools.read().await;
        let core_tools: Vec<&str> = vec![]; // Empty for listing

        tools
            .values()
            .filter(|t| t.is_active)
            .filter(|t| health.is_healthy(&t.name))
            .filter(|t| {
                if let Some(cat) = category {
                    let tool_cat = ToolIndexCategory::from(&t.source);
                    tool_cat.display_name().to_lowercase() == cat.to_lowercase()
                } else {
                    true
                }
            })
            .map(|t| t.to_index_entry(&core_tools))
            .collect()
    }
}
