//! Query Methods for `ToolCatalog`
//!
//! Methods for querying and searching tools in the registry.

use super::super::types::{ChannelType, ToolSource, UnifiedTool};
use super::types::ToolStorage;

/// Query functionality for `ToolCatalog`
pub struct ToolQuery {
    tools: ToolStorage,
}

impl ToolQuery {
    /// Create a new query handler with the given storage
    pub const fn new(tools: ToolStorage) -> Self {
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
    /// Returns the system builtin commands sorted by `sort_order`.
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

    /// List all tools for UI display (sorted by `sort_order`, then name)
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
                    && (t.visible_channels.is_empty() || t.visible_channels.contains(&channel))
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

        // Greedy longest-match: try joining progressively fewer words with "_"
        for n in (1..=max_depth).rev() {
            let candidate = words[..n].join("_");
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
    ///
    /// Resolution precedence:
    /// 1. **Canonical name** — a tool whose `name` equals `name`.
    /// 2. **Alias** — a tool that lists `name` in its `aliases`.
    /// 3. **Separator-tolerant** — the input's `.` normalized to `_` matches a
    ///    canonical name or alias (mirrors the execution layer's leniency).
    ///
    /// A canonical-name hit always wins over an alias hit (so a real command
    /// can never be shadowed by another command's alias), and an exact hit in
    /// either tier always wins over the separator-tolerant fallback. Within each
    /// tier, ties break by source priority, then id, mirroring the original
    /// ordering.
    fn find_best_match(
        tools: &std::collections::HashMap<String, UnifiedTool>,
        name: &str,
    ) -> Option<UnifiedTool> {
        // Tier 3 normalization, computed once outside the loop. The execution
        // layer (`LoopToolRegistry::resolve`) already swaps `.`↔`_`, so a user
        // or bot that types `/session.new` must resolve the same command it
        // would execute — otherwise the slash command silently falls through
        // to plain chat. Canonical names and aliases never carry dots, so we
        // only normalize the *input's* `.`→`_` and re-compare. Guarded on
        // `name.contains('.')` and ranked below the exact tiers, this is
        // strictly additive: it only adds matches the exact tiers missed and
        // can never shadow a real command.
        let norm = if name.contains('.') {
            Some(name.replace('.', "_"))
        } else {
            None
        };

        // Single pass with per-tier best tracking. Replaces the original
        // three full scans (`pick` chained via `or_else`) with identical
        // semantics: tier order is canonical name > alias > normalized, and
        // within a tier the comparator is the same one `max_by` used —
        // higher `source.priority()` wins, ties go to the lexicographically
        // larger `id`, and a full tie keeps the *later* element (max_by
        // returns the last of equal maxima), which `replace` reproduces by
        // swapping in the candidate whenever the incumbent is not strictly
        // greater.
        let replace = |incumbent: &UnifiedTool, candidate: &UnifiedTool| {
            incumbent
                .source
                .priority()
                .cmp(&candidate.source.priority())
                .then_with(|| candidate.id.cmp(&incumbent.id))
                != std::cmp::Ordering::Greater
        };

        let mut best: [Option<&UnifiedTool>; 3] = [None, None, None];

        for tool in tools.values() {
            if !tool.is_active {
                continue;
            }
            // Lowercase the name once and each alias once.
            let name_lower = tool.name.to_lowercase();
            let mut alias_hit_name = false;
            let mut alias_hit_norm = false;
            for alias in &tool.aliases {
                let alias_lower = alias.to_lowercase();
                alias_hit_name = alias_hit_name || alias_lower == name;
                if let Some(norm) = &norm {
                    alias_hit_norm = alias_hit_norm || alias_lower == *norm;
                }
            }

            let tier = if name_lower == name {
                // Tier 0: canonical name.
                Some(0)
            } else if alias_hit_name {
                // Tier 1: alias fallback.
                Some(1)
            } else if let Some(norm) = &norm {
                // Tier 2: separator-tolerant fallback.
                if name_lower == *norm || alias_hit_norm {
                    Some(2)
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(tier) = tier {
                if best[tier].is_none_or(|incumbent| replace(incumbent, tool)) {
                    best[tier] = Some(tool);
                }
            }
        }

        // Cross-tier precedence mirrors the original `.or_else` chain:
        // tier 0 > tier 1 > tier 2.
        best[0].or(best[1]).or(best[2]).cloned()
    }

    /// Suggest up to `max` active command names closest to `needle` — an
    /// unknown command name (with or without a leading `/`) — by edit distance.
    ///
    /// Both the canonical `name` and every alias are scored; the tool's best
    /// (lowest) distance wins. Returns canonical names (no leading `/`),
    /// nearest first. Powers the "did you mean?" reply on both the channel
    /// router (`try_send_unknown_command_help`) and the panel `command.execute`
    /// RPC path, replacing per-call-site inline scoring.
    pub async fn suggest_commands(&self, needle: &str, max: usize) -> Vec<String> {
        use crate::builtin_tools::meta_tools::levenshtein_distance;

        let needle = needle.trim().trim_start_matches('/').to_lowercase();
        if needle.is_empty() || max == 0 {
            return Vec::new();
        }

        // Snapshot under the read lock only what scoring needs (lowercased
        // canonical name + aliases of active tools, plus the original-case
        // name for the result); Levenshtein scoring then runs after the
        // guard drops instead of holding the lock across O(tools×aliases)
        // edit-distance work.
        let snapshot: Vec<(String, String, Vec<String>)> = {
            let tools = self.tools.read().await;
            tools
                .values()
                .filter(|t| t.is_active)
                .map(|t| {
                    (
                        t.name.clone(),
                        t.name.to_lowercase(),
                        t.aliases.iter().map(|a| a.to_lowercase()).collect(),
                    )
                })
                .collect()
        };

        let mut scored: Vec<(usize, String)> = Vec::new();

        'tool: for (name, name_lower, aliases_lower) in &snapshot {
            // Best (lowest) effective distance across canonical name + aliases.
            // Threshold is tuned for short identifiers: at most 2 edits for
            // ≤6-char candidates, 3 otherwise, plus a substring fast-path.
            let mut best: Option<usize> = None;
            for cand in std::iter::once(name_lower).chain(aliases_lower.iter()) {
                if *cand == needle {
                    // Exact hit — the caller should have resolved this already;
                    // skip the whole tool so we never suggest what matches.
                    continue 'tool;
                }
                let dist = levenshtein_distance(cand, &needle);
                let threshold = if cand.len().max(needle.len()) <= 6 {
                    2
                } else {
                    3
                };
                let substring_hit = cand.contains(&needle) || needle.contains(cand.as_str());
                if dist <= threshold || substring_hit {
                    let effective = if substring_hit { dist.min(2) } else { dist };
                    best = Some(best.map_or(effective, |b| b.min(effective)));
                }
            }
            if let Some(d) = best {
                scored.push((d, name.clone()));
            }
        }

        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(max);
        scored.into_iter().map(|(_, n)| n).collect()
    }

    /// Check if a name is a namespace (has active tools with that prefix)
    ///
    /// e.g. `is_namespace("session")` returns true if tools like `session_new` exist.
    pub async fn is_namespace(&self, name: &str) -> bool {
        let prefix = format!("{}_", name.to_lowercase());
        let tools = self.tools.read().await;
        tools
            .values()
            .any(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix))
    }

    /// List direct children of a namespace
    ///
    /// e.g. `list_namespace_children("session")` returns tools named `session_new`,
    /// `session_list`, etc.
    pub async fn list_namespace_children(&self, namespace: &str) -> Vec<UnifiedTool> {
        let prefix = format!("{}_", namespace.to_lowercase());
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
                // Only direct children: no further underscores after prefix
                let suffix = &name_lower[prefix.len()..];
                !suffix.contains('_')
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
            a.source
                .display_priority()
                .cmp(&b.source.display_priority())
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.name.cmp(&b.name))
        });
        result
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
        results.sort_by_cached_key(|t| {
            let name_lower = t.name.to_lowercase();
            let name_match = name_lower.contains(&query_lower);
            (!name_match, name_lower)
        });

        results
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
