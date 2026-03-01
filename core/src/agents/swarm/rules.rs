//! Rule Engine for fast event aggregation
//!
//! Pattern-matching based aggregation for 90% of high-frequency events.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use super::aggregator::SlidingWindow;
use super::events::{FileOperation, ImportantEvent, InfoEvent};

/// Aggregation rule
pub struct AggregationRule {
    /// Pattern to match
    pub pattern: EventPattern,
    /// Time window in milliseconds
    pub window_ms: u64,
    /// Minimum number of matching events
    pub threshold: usize,
    /// Function to create aggregated event
    pub output: fn(Vec<InfoEvent>) -> ImportantEvent,
}

/// Event pattern for matching
#[derive(Debug, Clone)]
pub enum EventPattern {
    /// File access with optional path prefix
    FileAccess {
        path_prefix: Option<String>,
        operation: Option<FileOperation>,
    },
    /// Symbol search
    SymbolSearch {
        symbol: Option<String>,
    },
    /// Tool execution
    ToolExecution {
        tool: Option<String>,
    },
}

impl EventPattern {
    /// Check if an event matches this pattern
    pub fn matches(&self, event: &InfoEvent) -> bool {
        match (self, event) {
            (
                EventPattern::FileAccess { path_prefix, operation },
                InfoEvent::FileAccessed { path, operation: op, .. },
            ) => {
                let path_match = path_prefix.as_ref()
                    .map(|prefix| path.starts_with(prefix))
                    .unwrap_or(true);

                let op_match = operation.as_ref()
                    .map(|expected| expected == op)
                    .unwrap_or(true);

                path_match && op_match
            }
            (
                EventPattern::SymbolSearch { symbol },
                InfoEvent::SymbolSearched { symbol: s, .. },
            ) => {
                symbol.as_ref()
                    .map(|expected| expected == s)
                    .unwrap_or(true)
            }
            (
                EventPattern::ToolExecution { tool },
                InfoEvent::ToolExecuted { tool: t, .. },
            ) => {
                tool.as_ref()
                    .map(|expected| expected == t)
                    .unwrap_or(true)
            }
            _ => false,
        }
    }
}

/// Rule Engine
pub struct RuleEngine {
    rules: Vec<AggregationRule>,
    /// Track recent matches for deduplication
    recent_matches: Arc<RwLock<HashMap<String, u64>>>,
}

impl RuleEngine {
    /// Create a new rule engine with custom rules
    pub fn new(rules: Vec<AggregationRule>) -> Self {
        Self {
            rules,
            recent_matches: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create rule engine with default rules
    pub fn with_default_rules() -> Self {
        Self::new(default_rules())
    }

    /// Try to aggregate an event using rules
    pub async fn try_aggregate(
        &self,
        event: &InfoEvent,
        window: &Arc<RwLock<SlidingWindow>>,
    ) -> Option<ImportantEvent> {
        let now = current_timestamp();

        for rule in &self.rules {
            if !rule.pattern.matches(event) {
                continue;
            }

            // Get matching events from window
            let matching_events = {
                let window_guard = window.read().await;
                let recent = window_guard.get_recent(100);

                recent
                    .into_iter()
                    .filter(|e| {
                        rule.pattern.matches(e) &&
                        (now - e.timestamp()) * 1000 <= rule.window_ms
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if matching_events.len() >= rule.threshold {
                // Check if we recently fired this rule
                let rule_key = format!("{:?}_{}", rule.pattern, rule.threshold);
                let mut matches = self.recent_matches.write().await;

                if let Some(&last_fire) = matches.get(&rule_key) {
                    if (now - last_fire) * 1000 < rule.window_ms {
                        // Too soon to fire again
                        continue;
                    }
                }

                // Fire the rule
                matches.insert(rule_key, now);
                return Some((rule.output)(matching_events));
            }
        }

        None
    }
}

/// Default aggregation rules
fn default_rules() -> Vec<AggregationRule> {
    vec![
        // Rule 1: Multiple agents accessing same directory -> Hotspot
        AggregationRule {
            pattern: EventPattern::FileAccess {
                path_prefix: None,
                operation: Some(FileOperation::Read),
            },
            window_ms: 1000,
            threshold: 3,
            output: |events| {
                // Extract common path prefix
                let paths: Vec<&str> = events.iter()
                    .filter_map(|e| match e {
                        InfoEvent::FileAccessed { path, .. } => Some(path.as_str()),
                        _ => None,
                    })
                    .collect();

                let area = find_common_prefix(&paths).unwrap_or("/");

                // Count unique agents
                let mut agents = std::collections::HashSet::new();
                for event in &events {
                    if let InfoEvent::FileAccessed { agent_id, .. } = event {
                        agents.insert(agent_id.clone());
                    }
                }

                ImportantEvent::Hotspot {
                    area: area.to_string(),
                    agent_count: agents.len(),
                    activity: "file_analysis".to_string(),
                    timestamp: current_timestamp(),
                }
            },
        },

        // Rule 2: Multiple symbol searches -> Confirmed Insight
        AggregationRule {
            pattern: EventPattern::SymbolSearch { symbol: None },
            window_ms: 2000,
            threshold: 2,
            output: |events| {
                // Find most searched symbol
                let mut symbol_counts: HashMap<String, usize> = HashMap::new();
                let mut sources = Vec::new();

                for event in &events {
                    if let InfoEvent::SymbolSearched { symbol, agent_id, .. } = event {
                        *symbol_counts.entry(symbol.clone()).or_insert(0) += 1;
                        sources.push(agent_id.clone());
                    }
                }

                let (symbol, count) = symbol_counts.iter()
                    .max_by_key(|(_, &count)| count)
                    .map(|(s, &c)| (s.clone(), c))
                    .unwrap_or_else(|| ("unknown".to_string(), 0));

                ImportantEvent::ConfirmedInsight {
                    symbol,
                    confidence: (count as f32 / events.len() as f32).min(1.0),
                    sources,
                    timestamp: current_timestamp(),
                }
            },
        },
    ]
}

/// Find common prefix among paths
fn find_common_prefix<'a>(paths: &'a [&'a str]) -> Option<&'a str> {
    if paths.is_empty() {
        return None;
    }

    let first = paths[0];
    let mut prefix_byte_len = first.len();

    for path in &paths[1..] {
        let common_bytes = first.as_bytes().iter()
            .zip(path.as_bytes().iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix_byte_len = prefix_byte_len.min(common_bytes);
    }

    // Ensure we don't split a multi-byte character
    while prefix_byte_len > 0 && !first.is_char_boundary(prefix_byte_len) {
        prefix_byte_len -= 1;
    }

    // Find last '/' before prefix_byte_len
    if let Some(pos) = first[..prefix_byte_len].rfind('/') {
        Some(&first[..=pos])
    } else {
        Some("/")
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_access_pattern_matching() {
        let pattern = EventPattern::FileAccess {
            path_prefix: Some("auth/".to_string()),
            operation: Some(FileOperation::Read),
        };

        let matching = InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "auth/login.rs".into(),
            operation: FileOperation::Read,
            timestamp: 0,
        };

        let non_matching = InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "core/main.rs".into(),
            operation: FileOperation::Read,
            timestamp: 0,
        };

        assert!(pattern.matches(&matching));
        assert!(!pattern.matches(&non_matching));
    }

    #[test]
    fn test_symbol_search_pattern_matching() {
        let pattern = EventPattern::SymbolSearch {
            symbol: Some("AuthService".to_string()),
        };

        let matching = InfoEvent::SymbolSearched {
            agent_id: "agent_1".into(),
            symbol: "AuthService".into(),
            context: None,
            timestamp: 0,
        };

        let non_matching = InfoEvent::SymbolSearched {
            agent_id: "agent_1".into(),
            symbol: "UserService".into(),
            context: None,
            timestamp: 0,
        };

        assert!(pattern.matches(&matching));
        assert!(!pattern.matches(&non_matching));
    }

    #[test]
    fn test_find_common_prefix() {
        let paths = vec![
            "/src/auth/login.rs",
            "/src/auth/logout.rs",
            "/src/auth/session.rs",
        ];

        let prefix = find_common_prefix(&paths);
        assert_eq!(prefix, Some("/src/auth/"));
    }

    #[test]
    fn test_find_common_prefix_no_common() {
        let paths = vec![
            "/src/auth/login.rs",
            "/core/main.rs",
        ];

        let prefix = find_common_prefix(&paths);
        assert_eq!(prefix, Some("/"));
    }

    #[test]
    fn test_default_rules_creation() {
        let rules = default_rules();
        assert_eq!(rules.len(), 2);
    }
}
