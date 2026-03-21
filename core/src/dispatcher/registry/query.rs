//! Query Methods for ToolRegistry
//!
//! Methods for querying and searching tools in the registry.

use crate::config::RoutingRuleConfig;

use super::super::types::{ChannelType, ToolSource, UnifiedTool};
use super::types::ToolStorage;

/// Query functionality for ToolRegistry
pub struct ToolQuery {
    tools: ToolStorage,
}

impl ToolQuery {
    /// Create a new query handler with the given storage
    pub fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// List all active tools
    ///
    /// Returns all tools where `is_active == true`.
    pub async fn list_all(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.values().filter(|t| t.is_active).cloned().collect()
    }

    /// List builtin tools only
    ///
    /// Returns the system builtin commands sorted by sort_order.
    pub async fn list_builtin_tools(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut builtins: Vec<_> = tools
            .values()
            .filter(|t| t.is_builtin && t.is_active)
            .cloned()
            .collect();
        builtins.sort_by_key(|t| t.sort_order);
        builtins
    }

    /// List preset tools for Settings UI (Flat Namespace Mode)
    ///
    /// Returns all non-Custom tools: Builtin + MCP + Skill + Native
    /// These are the "preset" tools that users can't delete, sorted by priority.
    pub async fn list_preset_tools(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut presets: Vec<_> = tools
            .values()
            .filter(|t| t.is_active && !matches!(t.source, ToolSource::Custom { .. }))
            .cloned()
            .collect();

        // Sort by source priority: Builtin > Native > MCP > Skill
        presets.sort_by(|a, b| {
            let priority_a = match &a.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Mcp { .. } => 2,
                ToolSource::Plugin { .. } => 3,
                ToolSource::Skill { .. } => 4,
                ToolSource::Custom { .. } => 5,
            };
            let priority_b = match &b.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Mcp { .. } => 2,
                ToolSource::Plugin { .. } => 3,
                ToolSource::Skill { .. } => 4,
                ToolSource::Custom { .. } => 5,
            };
            priority_a
                .cmp(&priority_b)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.name.cmp(&b.name))
        });
        presets
    }

    /// Generate routing rules from builtin tools
    ///
    /// This is the SINGLE SOURCE OF TRUTH for builtin command routing configuration.
    /// Config module should call this instead of maintaining separate hardcoded rules.
    ///
    /// Returns RoutingRuleConfig for each builtin tool that has routing_regex set.
    pub async fn get_builtin_routing_rules(&self) -> Vec<RoutingRuleConfig> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.is_builtin && t.routing_regex.is_some())
            .map(|t| RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: t.routing_regex.clone().unwrap_or_default(),
                provider: Some("openai".to_string()), // Will be overridden by default_provider
                system_prompt: t.routing_system_prompt.clone(),
                strip_prefix: Some(t.routing_strip_prefix),
                capabilities: if t.routing_capabilities.is_empty() {
                    None
                } else {
                    Some(t.routing_capabilities.clone())
                },
                intent_type: t.routing_intent_type.clone(),
                preferred_model: None,
                context_format: t.routing_context_format.clone(),
                icon: t.icon.clone(),
            })
            .collect()
    }

    /// List all tools for UI display (sorted by sort_order, then name)
    ///
    /// Returns all active tools suitable for Settings UI display.
    pub async fn list_all_for_ui(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools.values().filter(|t| t.is_active).cloned().collect();
        result.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));
        result
    }

    /// List active tools visible to a specific channel
    ///
    /// Tools with empty `visible_channels` are visible to all channels.
    /// Tools with non-empty `visible_channels` are only visible to listed channels.
    pub async fn list_for_channel(&self, channel: ChannelType) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active
                    && (t.visible_channels.is_empty()
                        || t.visible_channels.contains(&channel))
            })
            .cloned()
            .collect();
        result.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));
        result
    }

    /// Resolve a slash command input to a registered tool
    ///
    /// Supports hierarchical command resolution:
    /// - `/session new my-topic` → tool `session.new`, args `"my-topic"`
    /// - `/search weather` → tool `search`, args `"weather"`
    /// - `/plugin marketplace install x` → tool `plugin.marketplace.install`, args `"x"`
    ///
    /// Algorithm:
    /// 1. Strip `/` prefix, split into words
    /// 2. Strip `@botname` from first word (Telegram group commands)
    /// 3. Greedy longest-match: join words with `.`, try progressively shorter prefixes
    /// 4. Max depth: 3 levels
    pub async fn resolve_command(&self, input: &str) -> Option<super::types::ResolvedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let without_slash = &trimmed[1..];
        if without_slash.is_empty() {
            return None;
        }

        // Split into words
        let all_words: Vec<&str> = without_slash.split_whitespace().collect();
        if all_words.is_empty() {
            return None;
        }

        // Strip @botname suffix from first word (Telegram: "/gen@mybot" → "gen")
        let first_word = match all_words[0].split_once('@') {
            Some((name, _)) => name.to_lowercase(),
            None => all_words[0].to_lowercase(),
        };

        // Build candidate words: first_word (with @botname stripped) + rest
        let mut words: Vec<String> = vec![first_word];
        for w in &all_words[1..] {
            words.push(w.to_lowercase());
        }

        let tools = self.tools.read().await;

        // Max depth 3: try at most 3 words as the command path
        let max_depth = words.len().min(3);

        // Greedy longest-match: try joining progressively fewer words with "."
        for n in (1..=max_depth).rev() {
            let candidate = words[..n].join(".");
            if let Some(tool) = Self::find_best_match(&tools, &candidate) {
                let remaining: Vec<&str> = all_words[n..].iter().map(|s| s.as_ref()).collect();
                let arguments = if remaining.is_empty() {
                    None
                } else {
                    Some(remaining.join(" "))
                };
                return Some(super::types::ResolvedCommand {
                    tool,
                    arguments,
                    raw_input: input.to_string(),
                });
            }
        }

        None
    }

    /// Find the best matching active tool by name (case-insensitive).
    /// When multiple tools share the same name, picks highest source priority.
    fn find_best_match(
        tools: &std::collections::HashMap<String, UnifiedTool>,
        name: &str,
    ) -> Option<UnifiedTool> {
        tools
            .values()
            .filter(|t| t.is_active && t.name.to_lowercase() == name)
            .max_by(|a, b| {
                a.source
                    .priority()
                    .cmp(&b.source.priority())
                    .then_with(|| b.id.cmp(&a.id))
            })
            .cloned()
    }

    /// Check if a name is a namespace (has active tools with that prefix)
    ///
    /// e.g. `is_namespace("session")` returns true if tools like `session.new` exist.
    pub async fn is_namespace(&self, name: &str) -> bool {
        let prefix = format!("{}.", name.to_lowercase());
        let tools = self.tools.read().await;
        tools
            .values()
            .any(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix))
    }

    /// List direct children of a namespace
    ///
    /// e.g. `list_namespace_children("session")` returns tools named `session.new`,
    /// `session.list`, etc., but NOT `session.sub.deep`.
    pub async fn list_namespace_children(&self, namespace: &str) -> Vec<UnifiedTool> {
        let prefix = format!("{}.", namespace.to_lowercase());
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| {
                if !t.is_active {
                    return false;
                }
                let name_lower = t.name.to_lowercase();
                if !name_lower.starts_with(&prefix) {
                    return false;
                }
                // Only direct children: no further dots after prefix
                let suffix = &name_lower[prefix.len()..];
                !suffix.contains('.')
            })
            .cloned()
            .collect()
    }

    /// List root-level commands for UI (Flat Namespace Mode)
    ///
    /// Returns all active tools from all sources for command completion.
    /// This is the primary method for UI command completion display.
    ///
    /// Includes: Builtin + Native + Custom + MCP + Skill
    ///
    /// Source priority order for display:
    /// 1. Builtin (system commands)
    /// 2. Native (system capabilities)
    /// 3. Custom (user-defined rules)
    /// 4. MCP (external tools)
    /// 5. Skill (Claude Agent skills)
    pub async fn list_root_commands(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools.values().filter(|t| t.is_active).cloned().collect();

        // Sort by source priority, then sort_order, then name
        result.sort_by(|a, b| {
            // Sort order: Builtin > Native > Custom > MCP > Plugin > Skill
            let priority_a = match &a.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Custom { .. } => 2,
                ToolSource::Mcp { .. } => 3,
                ToolSource::Plugin { .. } => 4,
                ToolSource::Skill { .. } => 5,
            };
            let priority_b = match &b.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Custom { .. } => 2,
                ToolSource::Mcp { .. } => 3,
                ToolSource::Plugin { .. } => 4,
                ToolSource::Skill { .. } => 5,
            };

            priority_a
                .cmp(&priority_b)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.name.cmp(&b.name))
        });
        result
    }

    /// List all tools including inactive ones
    pub async fn list_all_with_inactive(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.values().cloned().collect()
    }

    /// List tools by source type
    ///
    /// # Arguments
    ///
    /// * `source_type` - One of "Native", "Mcp", "Skill", "Custom"
    pub async fn list_by_source_type(&self, source_type: &str) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.is_active && t.source.label() == source_type)
            .cloned()
            .collect()
    }

    /// List tools by MCP server name
    pub async fn list_by_mcp_server(&self, server: &str) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| {
                t.is_active && matches!(&t.source, ToolSource::Mcp { server: s } if s == server)
            })
            .cloned()
            .collect()
    }

    /// Get tool by ID
    ///
    /// # Arguments
    ///
    /// * `id` - Full tool ID (e.g., "native:search", "mcp:fs:read_file")
    pub async fn get_by_id(&self, id: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.get(id).cloned()
    }

    /// Get tool by name
    ///
    /// Searches for a tool by its command name (not full ID).
    /// When multiple tools match, returns the one with the highest source
    /// priority (deterministic: breaks ties by lexicographically smallest ID).
    pub async fn get_by_name(&self, name: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.name == name || t.id.ends_with(&format!(":{}", name)))
            .max_by(|a, b| {
                a.source
                    .priority()
                    .cmp(&b.source.priority())
                    .then_with(|| b.id.cmp(&a.id)) // smaller ID wins on tie
            })
            .cloned()
    }

    /// Fuzzy search tools by name or description
    ///
    /// Returns tools where name or description contains the query string.
    /// Results are ordered by relevance (name match first, then description).
    pub async fn search(&self, query: &str) -> Vec<UnifiedTool> {
        let query_lower = query.to_lowercase();
        let tools = self.tools.read().await;

        let mut results: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active
                    && (t.name.to_lowercase().contains(&query_lower)
                        || t.description.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect();

        // Sort by relevance: name matches first
        results.sort_by(|a, b| {
            let a_name_match = a.name.to_lowercase().contains(&query_lower);
            let b_name_match = b.name.to_lowercase().contains(&query_lower);
            match (a_name_match, b_name_match) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        results
    }

    /// Filter active tools by name prefix (case-insensitive)
    pub async fn filter_by_prefix(&self, prefix: &str) -> Vec<UnifiedTool> {
        if prefix.is_empty() {
            return self.list_all().await;
        }
        let prefix_lower = prefix.to_lowercase();
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix_lower))
            .cloned()
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Get total tool count
    pub async fn count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.len()
    }

    /// Get active tool count
    pub async fn active_count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.values().filter(|t| t.is_active).count()
    }
}
