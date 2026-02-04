# Natural Language Command Detection Design

> Date: 2026-01-20
> Status: Draft

## Overview

Implement natural language recognition for slash commands, allowing users to invoke Skills, MCP servers, and Custom commands without the `/` prefix.

**Goals:**
- Explicit mention: "使用 knowledge-graph 分析代码" → invoke `/knowledge-graph`
- Implicit intent: "帮我画个知识图谱" → auto-detect and invoke `/knowledge-graph`

## Architecture

```
User Input
    ↓
┌─────────────────────────────────────────────────────────────┐
│  NaturalLanguageCommandDetector (new module)                │
│  ├─ L1: Explicit command detection (regex "使用/use X")    │
│  │      → Extract command name → Lookup in registry         │
│  │                                                          │
│  └─ L2: Implicit intent detection (keyword matching)        │
│         → Iterate all commands' triggers/description        │
│         → Calculate match score → Sort by priority          │
└─────────────────────────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────────────────────────┐
│  CommandParser (existing, enhanced)                         │
│  ├─ Original: Parse /command format                         │
│  └─ New: Accept NaturalLanguageCommandDetector results      │
└─────────────────────────────────────────────────────────────┘
    ↓
ParsedCommand { source_type, command_name, arguments, ... }
```

## L1: Explicit Command Detection

**Location**: `core/src/command/nl_detector.rs`

### Supported Patterns

**Chinese:**
- `使用/用/调用/执行/运行 X`
- `让/交给 X 来/处理/做`

**English:**
- `use/invoke/call/run/execute X`
- `ask/let X to`
- `with/using X`

### Implementation

```rust
static EXPLICIT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Chinese patterns
        Regex::new(r"(?i)^(使用|用|调用|执行|运行)\s*[「「]?(\S+)[」」]?\s*").unwrap(),
        Regex::new(r"(?i)(让|交给)\s*[「「]?(\S+)[」」]?\s*(来|处理|做)").unwrap(),

        // English patterns
        Regex::new(r"(?i)^(use|invoke|call|run|execute)\s+(\S+)\s+(to\s+)?").unwrap(),
        Regex::new(r"(?i)(ask|let)\s+(\S+)\s+(to\s+)").unwrap(),
        Regex::new(r"(?i)(with|using)\s+(\S+)[,\s]").unwrap(),
    ]
});

pub struct ExplicitMatch {
    pub command_name: String,
    pub source: ToolSource,
    pub remaining_input: Option<String>,
    pub confidence: f64,  // Always 1.0 for explicit
}
```

## L2: Implicit Intent Detection

### Trigger Configuration

**Priority:** Manual triggers > Auto-extracted from description

```rust
pub struct CommandTriggers {
    pub manual: Vec<String>,
    pub auto_extracted: Vec<String>,
}
```

### Skills Frontmatter Extension

```yaml
---
name: knowledge-graph
description: Generate knowledge graphs and analyze dependencies
triggers:
  - 知识图谱
  - 关系图
  - 依赖分析
  - graph
  - dependencies
---
```

### MCP Server Config Extension

```json
{
  "servers": {
    "knowledge-graph": {
      "command": "npx",
      "args": ["@example/kg-server"],
      "triggers": ["知识图谱", "关系图", "graph"]
    }
  }
}
```

### Custom Command Config Extension

```toml
[[routing.rules]]
regex = "^/translate"
hint = "翻译文本"
triggers = ["翻译", "translate", "转换语言"]
system_prompt = "..."
```

### Match Score Calculation

```rust
fn calculate_match_score(&self, input: &str, triggers: &CommandTriggers) -> f64 {
    let mut score = 0.0;

    // Manual triggers: weight 1.0
    for trigger in &triggers.manual {
        if input.contains(&trigger.to_lowercase()) {
            score += 1.0;
        }
    }

    // Auto-extracted: weight 0.6
    for trigger in &triggers.auto_extracted {
        if input.contains(&trigger.to_lowercase()) {
            score += 0.6;
        }
    }

    // Normalize
    let total = triggers.manual.len() + triggers.auto_extracted.len();
    score / total.max(1) as f64
}
```

### Auto-extraction from Description

```rust
fn extract_keywords_from_description(description: &str) -> Vec<String> {
    let stop_words = ["the", "a", "an", "is", "are", "to", "for",
                      "的", "是", "和", "与", "用", "来", "可以"];

    description
        .split(|c: char| c.is_whitespace() || c == ',' || c == '，')
        .filter(|w| w.len() >= 2)
        .filter(|w| !stop_words.contains(&w.to_lowercase().as_str()))
        .map(|w| w.to_lowercase())
        .collect()
}
```

## Priority Strategy

**Mixed strategy:** Fixed priority between types, score-based within same type.

```rust
candidates.sort_by(|a, b| {
    let type_order_a = a.source.type_priority();  // Builtin=0, Skills=1, MCP=2, Custom=3
    let type_order_b = b.source.type_priority();

    type_order_a.cmp(&type_order_b)
        .then(b.score.partial_cmp(&a.score).unwrap())
});
```

## Integration with CommandParser

```rust
impl CommandParser {
    pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
        // 1. Original: check / prefix
        if input.trim_start().starts_with('/') {
            return self.parse_slash_command(input);
        }

        // 2. New: natural language detection
        if let Some(ref detector) = self.nl_detector {
            // L1: Explicit detection
            if let Some(explicit) = detector.detect_explicit(input) {
                return Some(self.create_command_from_match(/* ... */));
            }

            // L2: Implicit detection (confidence >= 0.5)
            if let Some(implicit) = detector.detect_implicit(input) {
                if implicit.confidence >= 0.5 {
                    return Some(self.create_command_from_match(/* ... */));
                }
            }
        }

        None
    }
}
```

## Unified Command Index

**Location:** `core/src/command/unified_index.rs`

```rust
pub struct UnifiedCommandIndex {
    entries: HashMap<String, Vec<IndexEntry>>,
}

pub struct IndexEntry {
    pub source_type: ToolSourceType,
    pub command_name: String,
    pub weight: f64,
}

impl UnifiedCommandIndex {
    pub fn build(
        skills_registry: &SkillsRegistry,
        mcp_configs: &HashMap<String, McpServerConfig>,
        routing_rules: &[RoutingRuleConfig],
    ) -> Self { /* ... */ }

    pub fn rebuild(&mut self, /* ... */) { /* ... */ }

    pub fn find_matches(&self, input: &str) -> Vec<ScoredMatch> { /* ... */ }
}
```

## File Structure

### New Files

```
core/src/command/
├── nl_detector.rs            # Natural language command detector (~300 lines)
└── unified_index.rs          # Unified command index (~250 lines)
```

### Modified Files

```
core/src/command/
├── mod.rs                    # Export new modules
├── parser.rs                 # Integrate NL detector
└── types.rs                  # Add triggers-related types

core/src/config/types/
├── mcp.rs                    # McpServerConfig.triggers
└── routing.rs                # RoutingRuleConfig.triggers

core/src/skills/
├── mod.rs                    # SkillFrontmatter.triggers
└── registry.rs               # Adapt to new triggers field

core/src/aleph.udl           # FFI interface updates (if needed)
```

## Dependencies

```
NaturalLanguageCommandDetector
         │
         ├─→ UnifiedCommandIndex
         │         │
         │         ├─→ SkillsRegistry
         │         ├─→ McpServerConfig
         │         └─→ RoutingRuleConfig
         │
         └─→ CommandParser
```

## Estimated Code

- `nl_detector.rs`: ~300 lines
- `unified_index.rs`: ~250 lines
- Existing file modifications: ~150 lines
- **Total**: ~700 lines
