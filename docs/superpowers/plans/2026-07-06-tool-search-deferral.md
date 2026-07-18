# Tool System C1 — `tool_search` Deferred Exposure Tier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third "deferred" tool-exposure tier — deferred tools (MCP tools, opt-in) are dropped from the model's initial tool list entirely and discovered via a new ranked `tool_search` meta-tool that returns matches with their full schemas, ready to call.

**Architecture:** Extends Aleph's existing two-piece progressive disclosure (`ProgressiveDisclosureRewriter` collapses schemas; `SchemaLookupTool`/`get_tool_schema` loads them) with a third tier. The deferred set is computed at the per-request registration seam (`run_loop/inner.rs`, where MCP tools are cleanly identified) and threaded as a name-set to `ScopedToolService`, which drops those names from `list()`/`metadata_schema()` (but NOT `describe()`, so a searched tool stays callable). A self-contained BM25 ranker over an identifier-aware tokenization powers `tool_search`. Default OFF (escape-hatch: byte-identical tool surface when disabled). Presentation-layer only — zero `src/harness/` growth.

**Tech Stack:** Rust, tokio, async-trait, serde_json. No new dependencies. Sibling spec: `docs/superpowers/specs/2026-07-06-tools-3.2-3.5-toolsearch-design.md`. Prerequisite plan: `2026-07-06-tools-3.2-3.5-wiring-entropy.md` (buckets A+B) — this plan is independent of it but assumes the same worktree/branch.

## Global Constraints

- **R10 zero harness growth:** no new/edited files under `src/harness/`. All work in `src/tools/`, `src/config/types/`, `src/gateway/execution_engine/`.
- **R3 no new dependencies.** BM25 is hand-rolled (~90 lines). The memory FTS5 `ContentIndex` was considered and rejected (per-request in-memory SQLite build + tools→context coupling; disproportionate for a tiny per-request corpus — P1 low coupling).
- **R7/P8 clean:** ranking is a mechanical lexical BM25 that the MODEL initiates by calling `tool_search`. It is NOT intent classification or message-driven tool filtering. This is exactly the R10「第2不」pre-authorized `tool_search` 元工具 pattern.
- **Non-破坏性 default OFF:** the deferred tier is gated by a new `[tools] defer_mcp_tools` config, default `false`. When off, no tool is deferred, `tool_search` is not registered, and the tool surface is byte-identical to today (prompt-cache continuity). Mirrors the existing `[tools] core` escape-hatch discipline.
- **Invariant — deferred ≠ unreachable:** a deferred tool is dropped only from the *presentation* `Vec<ToolDefinition>`. It stays registered in `loop_registry` (executable via `execute()`, which resolves against the full inner registry) and present in the `tool_search` corpus. Do NOT add the deferred filter to `ScopedToolService::describe()`.
- **cargo economy:** `cargo check -p alephcore --lib` + targeted `cargo test -p alephcore --lib <filter>`. No full suite. Windows excludes desktop macos/linux crates.
- **Branch:** fresh worktree branch, never `main`. Commit style `<scope>: <description>`.

---

### Task 1: Add `defer_mcp_tools` config (default off) and thread it to the engine

**Files:**
- Modify: `src/config/types/tools.rs` (add field to `ToolsConfig` + its `Default`)
- Modify: `src/gateway/execution_engine/mod.rs` (add field to `ExecutionEngineConfig` + its default)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:733` (copy config → engine)
- Test: `src/config/types/tools.rs` inline

**Interfaces:**
- Produces: `ToolsConfig.defer_mcp_tools: bool` (serde default false) and `ExecutionEngineConfig.defer_mcp_tools: bool`, readable at the registration seam as `self.config.defer_mcp_tools`.

- [ ] **Step 1: Write the failing test**

Add to `src/config/types/tools.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn defer_mcp_tools_defaults_off() {
        let cfg = ToolsConfig::default();
        assert!(!cfg.defer_mcp_tools, "deferred tier must default OFF (escape-hatch)");
    }

    #[test]
    fn defer_mcp_tools_deserializes() {
        let cfg: ToolsConfig = toml::from_str("defer_mcp_tools = true").unwrap();
        assert!(cfg.defer_mcp_tools);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib config::types::tools::tests::defer_mcp_tools_defaults_off`
Expected: FAIL to compile (`no field defer_mcp_tools`).

- [ ] **Step 3: Add the field to `ToolsConfig`**

In `src/config/types/tools.rs`, after the `truncate_tool_descriptions` field (line ~141), add:

```rust
    /// When true, tools from connected MCP servers are DEFERRED: dropped from
    /// the model's initial tool list and discoverable only via the
    /// `tool_search` meta-tool (which returns matches with their schemas).
    /// Default false = byte-identical tool surface (escape hatch). Independent
    /// of `core` / schema collapsing; a deferred tool is neither listed nor
    /// collapsed until `tool_search` surfaces it.
    #[serde(default)]
    pub defer_mcp_tools: bool,
```

Then in `impl Default for ToolsConfig` (line ~179), add `defer_mcp_tools: false,` alongside `truncate_tool_descriptions: false,`.

- [ ] **Step 4: Add the mirror field to `ExecutionEngineConfig`**

In `src/gateway/execution_engine/mod.rs`, after the `truncate_tool_descriptions` field (line ~98), add:

```rust
    /// Mirror of `[tools] defer_mcp_tools`. Gates the deferred exposure tier +
    /// `tool_search` registration at the per-request seam.
    pub defer_mcp_tools: bool,
```

In the same file's `Default` impl for `ExecutionEngineConfig` (near `core_tools: ...default_core_tools()` at line ~110), add `defer_mcp_tools: false,`.

- [ ] **Step 5: Thread config → engine at agent init**

In `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, at the `ExecutionEngineConfig` construction (line 733 sets `core_tools: app_config.tools.core.clone()`), add adjacent:

```rust
            defer_mcp_tools: app_config.tools.defer_mcp_tools,
```

- [ ] **Step 6: Run test + compile**

Run: `cargo test -p alephcore --lib config::types::tools::tests::defer_mcp_tools_defaults_off config::types::tools::tests::defer_mcp_tools_deserializes`
Expected: PASS.

Run: `cargo check -p alephcore --lib`
Expected: compiles clean (both engine construction sites carry the new field).

- [ ] **Step 7: Commit**

```bash
git add src/config/types/tools.rs src/gateway/execution_engine/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "config: add [tools] defer_mcp_tools flag (default off) threaded to ExecutionEngineConfig"
```

---

### Task 2: Create the `tool_search` meta-tool (BM25 ranker + LoopTool)

**Files:**
- Create: `src/tools/tool_search.rs`
- Modify: `src/tools/mod.rs:63` (add `pub mod tool_search;`)
- Test: `src/tools/tool_search.rs` inline

**Interfaces:**
- Produces: `pub struct ToolDoc { pub name: String, pub description: String, pub schema: serde_json::Value }`; `pub struct ToolSearchTool` with `pub const NAME: &'static str = "tool_search"`, `pub fn new(docs: Vec<ToolDoc>) -> Self`, `pub fn is_empty(&self) -> bool`, and `impl LoopTool`. `execute` returns `{ query, count, results: [{ name, description, parameters, score }] }`.

- [ ] **Step 1: Write the module with the ranker + tool**

Create `src/tools/tool_search.rs`:

```rust
//! `tool_search` — ranked discovery meta-tool for the "deferred" exposure
//! tier. Tools deferred out of the model's initial list (MCP tools when
//! `[tools] defer_mcp_tools` is on) stay searchable here: the model queries by
//! capability and gets the top matches WITH their full input schema, so it can
//! call them directly. Registered per-request alongside `get_tool_schema`
//! (see `gateway/execution_engine/run_loop/inner.rs`), closing over a snapshot
//! of every tool's name + description + schema.
//!
//! Ranking is a self-contained BM25 over an identifier-aware tokenization of
//! (name + description) — no new dependency, no coupling to the memory FTS5
//! index. Mechanical lexical rank that the MODEL initiates → R7-clean (not
//! intent classification), R10 presentation layer (zero harness growth).

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, ToolResult};

/// One searchable tool: display name, description, full input schema.
#[derive(Clone)]
pub struct ToolDoc {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Identifier-aware tokenizer: lowercases, splits on every non-alphanumeric
/// boundary AND camelCase humps, so `browser_navigate`, `mcp:slack:post`, and
/// `getUserById` all yield their component words. Tokens under 2 bytes drop.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.len() >= 2 {
            out.push(cur.to_lowercase());
        }
        cur.clear();
    };
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower_or_digit {
                flush(&mut cur, &mut out); // camelCase boundary
            }
            cur.push(ch);
            prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        } else {
            flush(&mut cur, &mut out);
            prev_lower_or_digit = false;
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// In-memory BM25 index over the tool corpus.
struct Bm25 {
    docs: Vec<ToolDoc>,
    doc_tokens: Vec<Vec<String>>,
    df: HashMap<String, usize>,
    avgdl: f64,
    n: usize,
}

impl Bm25 {
    fn build(docs: Vec<ToolDoc>) -> Self {
        let doc_tokens: Vec<Vec<String>> = docs
            .iter()
            .map(|d| {
                let mut t = tokenize(&d.name);
                t.extend(tokenize(&d.description));
                t
            })
            .collect();
        let n = docs.len();
        let mut df: HashMap<String, usize> = HashMap::new();
        for toks in &doc_tokens {
            let uniq: std::collections::HashSet<&String> = toks.iter().collect();
            for t in uniq {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
        }
        let total: usize = doc_tokens.iter().map(Vec::len).sum();
        let avgdl = if n == 0 { 0.0 } else { total as f64 / n as f64 };
        Self { docs, doc_tokens, df, avgdl, n }
    }

    fn score(&self, q_tokens: &[String], doc_idx: usize) -> f64 {
        const K1: f64 = 1.5;
        const B: f64 = 0.75;
        let toks = &self.doc_tokens[doc_idx];
        let dl = toks.len() as f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in toks {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0;
        for q in q_tokens {
            let f = *tf.get(q.as_str()).unwrap_or(&0) as f64;
            if f == 0.0 {
                continue;
            }
            let nq = *self.df.get(q).unwrap_or(&0) as f64;
            let idf = (1.0 + (self.n as f64 - nq + 0.5) / (nq + 0.5)).ln();
            let denom = f + K1 * (1.0 - B + B * dl / self.avgdl.max(1.0));
            score += idf * (f * (K1 + 1.0)) / denom;
        }
        score
    }

    fn top_k(&self, query: &str, k: usize) -> Vec<(usize, f64)> {
        let q = tokenize(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, f64)> = (0..self.n)
            .map(|i| (i, self.score(&q, i)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        // Highest score first; deterministic name tiebreak.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| self.docs[a.0].name.cmp(&self.docs[b.0].name))
        });
        scored.truncate(k);
        scored
    }
}

/// Ranked discovery tool over a per-request corpus snapshot.
pub struct ToolSearchTool {
    index: Arc<Bm25>,
}

impl ToolSearchTool {
    pub const NAME: &'static str = "tool_search";

    #[must_use]
    pub fn new(docs: Vec<ToolDoc>) -> Self {
        Self { index: Arc::new(Bm25::build(docs)) }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.n == 0
    }
}

#[async_trait]
impl LoopTool for ToolSearchTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn description(&self) -> &str {
        "Search the full tool catalog by capability and get the best-matching tools \
         WITH their input schemas, ready to call. Use this to find tools not shown in \
         your initial tool list (e.g. connected MCP server tools). Query with plain \
         keywords describing what you want to do, e.g. \"send a slack message\"."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords describing the capability you need."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 5).",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        let query = input.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.trim().is_empty() {
            return ToolResult::Error {
                error: "tool_search requires a non-empty `query`.".to_string(),
                retryable: false,
            };
        }
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 20) as usize;
        let hits = self.index.top_k(query, limit);
        let results: Vec<Value> = hits
            .iter()
            .map(|(i, score)| {
                let d = &self.index.docs[*i];
                json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.schema,
                    "score": (score * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        ToolResult::Success {
            output: json!({ "query": query, "count": results.len(), "results": results }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<ToolDoc> {
        vec![
            ToolDoc { name: "slack_post_message".into(), description: "Send a message to a Slack channel".into(), schema: json!({"type":"object","properties":{"channel":{"type":"string"}}}) },
            ToolDoc { name: "github_create_issue".into(), description: "Open a new GitHub issue in a repository".into(), schema: json!({"type":"object"}) },
            ToolDoc { name: "browser_navigate".into(), description: "Navigate the browser to a URL".into(), schema: json!({"type":"object"}) },
        ]
    }

    #[test]
    fn tokenize_splits_identifiers_and_camel() {
        assert!(tokenize("browser_navigate").contains(&"navigate".to_string()));
        assert!(tokenize("mcp:slack:post").contains(&"slack".to_string()));
        assert!(tokenize("getUserById").contains(&"user".to_string()));
    }

    #[tokio::test]
    async fn ranks_relevant_tool_first_with_schema() {
        let t = ToolSearchTool::new(corpus());
        let out = t.execute(json!({"query":"send slack message"}), CancellationToken::new()).await;
        match out {
            ToolResult::Success { output } => {
                let results = output["results"].as_array().unwrap();
                assert!(!results.is_empty());
                assert_eq!(results[0]["name"], "slack_post_message");
                // top hit carries its full schema so the model can call directly
                assert!(results[0]["parameters"]["properties"]["channel"].is_object());
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn limit_is_respected() {
        let t = ToolSearchTool::new(corpus());
        let out = t.execute(json!({"query":"a e i o u the to in","limit":2}), CancellationToken::new()).await;
        if let ToolResult::Success { output } = out {
            assert!(output["results"].as_array().unwrap().len() <= 2);
        } else {
            panic!("expected success");
        }
    }

    #[tokio::test]
    async fn empty_query_errors() {
        let t = ToolSearchTool::new(corpus());
        let out = t.execute(json!({"query":"   "}), CancellationToken::new()).await;
        assert!(matches!(out, ToolResult::Error { .. }));
    }

    #[tokio::test]
    async fn no_match_returns_empty_not_error() {
        let t = ToolSearchTool::new(corpus());
        let out = t.execute(json!({"query":"zzzqqq_nonexistent_capability"}), CancellationToken::new()).await;
        if let ToolResult::Success { output } = out {
            assert_eq!(output["count"], 0);
        } else {
            panic!("expected success with empty results");
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `src/tools/mod.rs`, next to `pub mod schema_lookup;` (line 63), add:

```rust
pub mod tool_search;
```

- [ ] **Step 3: Run tests to verify pass**

Run: `cargo test -p alephcore --lib tool_search::tests`
Expected: PASS (all 5 — tokenizer, ranking+schema, limit, empty-query error, no-match empty).

- [ ] **Step 4: Commit**

```bash
git add src/tools/tool_search.rs src/tools/mod.rs
git commit -m "tools: add tool_search meta-tool with self-contained BM25 ranker (deferred exposure discovery)"
```

---

### Task 3: Add the deferred-drop filter to `ScopedToolService`

**Files:**
- Modify: `src/tools/scoped/mod.rs` (add `deferred` struct field; add `is_deferred`; add retain in `list()` ~183 and `metadata_schema()` ~405)
- Modify: `src/tools/scoped/builder.rs` (init `deferred` in `new`; add `with_deferred`)
- Test: `src/tools/scoped/tests.rs` inline

**Interfaces:**
- Consumes: `ToolSearchTool` from Task 2 (not directly — this task only filters names).
- Produces: `ScopedToolService::with_deferred(self, deferred: BTreeSet<String>) -> Self`. Field `deferred: BTreeSet<String>` (empty = no-op).

- [ ] **Step 1: Write the failing test**

Add to `src/tools/scoped/tests.rs` (reuse the test harness there; adapt tool-registration helpers to the file's existing patterns):

```rust
    #[tokio::test]
    async fn deferred_tools_dropped_from_list_but_still_describable_and_executable() {
        // Registry with two tools; defer "beta".
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(NamedStub::new("alpha")));
        reg.register(Box::new(NamedStub::new("beta")));
        let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
            .with_deferred(["beta".to_string()].into_iter().collect());

        // list() and metadata_schema() omit the deferred tool.
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(!names.contains(&"beta".to_string()), "deferred tool must not be listed");
        let meta_names: Vec<String> =
            svc.metadata_schema().iter().map(|d| d.name.clone()).collect();
        assert!(!meta_names.contains(&"beta".to_string()));

        // describe() and execute() still reach it (searched → callable).
        assert!(svc.describe("beta").await.is_some(), "deferred tool must stay describable");
        assert!(svc.execute("beta", serde_json::json!({})).await.is_ok(),
            "deferred tool must stay executable");
    }

    #[tokio::test]
    async fn empty_deferred_set_is_byte_identical() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(NamedStub::new("alpha")));
        let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new());
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"alpha".to_string()));
    }
```

If a reusable `NamedStub` (a `LoopTool` whose `name()` is configurable) does not already exist in `scoped/tests.rs`, add one at the top of the test module:

```rust
    struct NamedStub(String);
    impl NamedStub { fn new(n: &str) -> Self { Self(n.to_string()) } }
    #[async_trait::async_trait]
    impl crate::tools::runtime::LoopTool for NamedStub {
        fn name(&self) -> &str { &self.0 }
        fn description(&self) -> &str { "stub" }
        fn schema(&self) -> serde_json::Value { serde_json::json!({"type":"object"}) }
        async fn execute(&self, _i: serde_json::Value, _c: tokio_util::sync::CancellationToken)
            -> crate::tools::runtime::ToolResult {
            crate::tools::runtime::ToolResult::Success { output: serde_json::json!({}) }
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib scoped::tests::deferred_tools_dropped_from_list_but_still_describable_and_executable`
Expected: FAIL to compile (`no method with_deferred`).

- [ ] **Step 3: Add the `deferred` field + initializer**

In `src/tools/scoped/mod.rs`, add to the `ScopedToolService` struct (after `definition_rewriters` at line ~103):

```rust
    /// Tool names dropped from `list()` / `metadata_schema()` (the "deferred"
    /// exposure tier) but kept executable + describable. Empty = no-op.
    /// Populated at the request seam from the MCP tool set when
    /// `[tools] defer_mcp_tools` is on. See [`ScopedToolService::with_deferred`].
    pub(super) deferred: BTreeSet<String>,
```

In `src/tools/scoped/builder.rs`, add `deferred: BTreeSet::new(),` to the `Self { ... }` in `new()` (after `definition_rewriters: Vec::new(),` at line 47).

- [ ] **Step 4: Add the `with_deferred` builder + `is_deferred` helper**

In `src/tools/scoped/builder.rs`, after `with_definition_rewriter` (line 82), add:

```rust
    /// Set the deferred tool-name set (the "deferred" exposure tier). Deferred
    /// tools are dropped from `list()` / `metadata_schema()` but remain
    /// executable + describable. Empty set = no-op.
    #[must_use]
    pub fn with_deferred(mut self, deferred: BTreeSet<String>) -> Self {
        self.deferred = deferred;
        self
    }

    /// Whether `name` is in the deferred tier (dropped from LLM-visible lists).
    pub(super) fn is_deferred(&self, name: &str) -> bool {
        self.deferred.contains(name)
    }
```

- [ ] **Step 5: Add the retain filter to `list()` and `metadata_schema()`**

In `src/tools/scoped/mod.rs`, in `list()`, immediately after the health retain block (before `self.apply_definition_rewriters(&mut defs);` at line ~183) add:

```rust
        // Deferred-tier drop: remove tools deferred out of the model's initial
        // list. They stay executable (execute resolves against self.inner) and
        // describable, and are discoverable via `tool_search`.
        if !self.deferred.is_empty() {
            defs.retain(|d| !self.is_deferred(&d.name));
        }
```

In `metadata_schema()`, the identical block immediately before `self.apply_definition_rewriters(&mut defs);` at line ~405:

```rust
        if !self.deferred.is_empty() {
            defs.retain(|d| !self.is_deferred(&d.name));
        }
```

Do NOT add this to `describe()` (a deferred-then-searched tool must stay describable).

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p alephcore --lib scoped::tests::deferred_tools_dropped_from_list_but_still_describable_and_executable scoped::tests::empty_deferred_set_is_byte_identical`
Expected: PASS.

Run: `cargo check -p alephcore --lib`
Expected: compiles clean (`new()` initializes the new field; no other `ScopedToolService::new` caller changes since the field is not a `new` parameter).

- [ ] **Step 7: Commit**

```bash
git add src/tools/scoped/mod.rs src/tools/scoped/builder.rs src/tools/scoped/tests.rs
git commit -m "tools: ScopedToolService deferred tier — drop deferred names from list/metadata_schema, keep executable"
```

---

### Task 4: Wire the deferred set + `tool_search` registration at the request seam

**Files:**
- Modify: `src/gateway/execution_engine/run_loop/inner.rs` (collect MCP names in the join loop; compute `deferred_tool_names`; register `ToolSearchTool`; pass the set to both `build_request_tool_service` calls)
- Modify: `src/gateway/execution_engine/tool_service_builder.rs` (add `deferred_tool_names` param; call `with_deferred`)
- Test: `src/gateway/execution_engine/tool_service_builder.rs` inline

**Interfaces:**
- Consumes: `ScopedToolService::with_deferred` (Task 3); `ToolSearchTool::{new, NAME}` + `ToolDoc` (Task 2); `self.config.defer_mcp_tools` (Task 1).
- Produces: `build_request_tool_service(...)` gains a trailing `deferred_tool_names: BTreeSet<String>` parameter.

- [ ] **Step 1: Write the failing test (builder honors deferred set)**

Add to `src/gateway/execution_engine/tool_service_builder.rs` `#[cfg(test)] mod tests` (extend the existing `StubTool` harness — note `StubTool::name()` returns `"read_file"`; add a second stub name for the deferral):

```rust
    struct OtherStub;
    #[async_trait]
    impl LoopTool for OtherStub {
        fn name(&self) -> &str { "web_fetch" }
        fn description(&self) -> &str { "stub2" }
        fn schema(&self) -> Value { json!({ "type": "object" }) }
        async fn execute(&self, _i: Value, _c: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    #[tokio::test]
    async fn builder_defers_named_tools() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));   // read_file
        reg.register(Box::new(OtherStub));  // web_fetch
        let deferred: BTreeSet<String> = ["web_fetch".to_string()].into_iter().collect();
        let svc = build_request_tool_service(
            Arc::new(reg), BTreeSet::new(), None, None, None, None, "",
            None, false, &[], false, deferred,
        );
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(!names.contains(&"web_fetch".to_string()), "deferred tool must be dropped");
    }
```

Also update the existing `builder_returns_listable_service` call (line ~186) to pass the new trailing arg `BTreeSet::new()` (empty deferred set).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib tool_service_builder::tests::builder_defers_named_tools`
Expected: FAIL to compile (arity mismatch — new param not yet added).

- [ ] **Step 3: Add the `deferred_tool_names` parameter**

In `src/gateway/execution_engine/tool_service_builder.rs`, add a trailing parameter to `build_request_tool_service` (after `truncate_tool_descriptions: bool,` at line 101):

```rust
    deferred_tool_names: BTreeSet<String>,
```

Then, just before `Arc::new(svc)` (line 151), add:

```rust
    // Deferred exposure tier: drop these names from the LLM-visible lists.
    // Empty set (defer_mcp_tools off) is a no-op — byte-identical surface.
    svc = svc.with_deferred(deferred_tool_names);
```

Update the doc-comment block above the fn to describe the new param (one line, matching the `core_tools` doc style).

- [ ] **Step 4: Collect MCP names + compute the deferred set at the seam**

In `src/gateway/execution_engine/run_loop/inner.rs`, in the MCP-join block (before line 571 `let mut joined = 0usize;`), declare a name collector:

```rust
                let mut mcp_tool_names: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
```

Inside the loop, after the successful `loop_registry_inner.register(...)` for the MCP tool (after line 589), add:

```rust
                    mcp_tool_names.insert(name.clone());
```

Because `mcp_tool_names` is declared inside the `if let Some(mcp_registry) = ...` block, hoist its declaration ABOVE that `if let` (to line ~559, before the block) so it survives for use after. I.e. declare `let mut mcp_tool_names = BTreeSet::new();` before `if let Some(mcp_registry)` and populate it inside.

- [ ] **Step 5: Register `ToolSearchTool` + build the deferred set**

In `src/gateway/execution_engine/run_loop/inner.rs`, after the `SchemaLookupTool` registration block (after line 634, before `let loop_registry = Arc::new(loop_registry_inner);` at line 636), add:

```rust
            // Deferred exposure tier (opt-in): when `[tools] defer_mcp_tools`
            // is on, MCP tools are dropped from the model's initial list and
            // surfaced only via `tool_search`. Build the search corpus from the
            // pre-collapse registry snapshot (name + description + full schema)
            // and register the meta-tool. Empty when off ⇒ byte-identical.
            let deferred_tool_names: std::collections::BTreeSet<String> =
                if self.config.defer_mcp_tools {
                    mcp_tool_names
                } else {
                    std::collections::BTreeSet::new()
                };
            if !deferred_tool_names.is_empty() {
                let docs: Vec<crate::tools::tool_search::ToolDoc> = loop_registry_inner
                    .tool_definitions()
                    .into_iter()
                    .map(|d| crate::tools::tool_search::ToolDoc {
                        name: d.name,
                        description: d.description,
                        schema: d.parameters,
                    })
                    .collect();
                loop_registry_inner.register(Box::new(
                    crate::tools::tool_search::ToolSearchTool::new(docs),
                ));
                if !allowed_names.is_empty() {
                    allowed_names
                        .insert(crate::tools::tool_search::ToolSearchTool::NAME.to_string());
                }
            }
```

(Note: `mcp_tool_names` is moved into `deferred_tool_names` here; that is fine — it is not used afterwards. If a borrow-check error arises because `deferred_tool_names` is later `.clone()`d into two build calls, that is expected — clone at each call site per Step 6.)

- [ ] **Step 6: Pass the deferred set to both `build_request_tool_service` calls**

In `src/gateway/execution_engine/run_loop/inner.rs`:
- Call at line 665 (`parent_view_for_children`): add trailing arg after `self.config.truncate_tool_descriptions,` (line 676):

```rust
                    deferred_tool_names.clone(),
```

- Call at line 891 (`tool_service`): add trailing arg after `self.config.truncate_tool_descriptions,` (line ~902):

```rust
                deferred_tool_names.clone(),
```

- [ ] **Step 7: Run tests + compile**

Run: `cargo test -p alephcore --lib tool_service_builder::tests`
Expected: PASS (`builder_defers_named_tools` + updated `builder_returns_listable_service`).

Run: `cargo check -p alephcore --lib`
Expected: compiles clean (both seam call sites carry the new arg; `deferred_tool_names` moved then cloned twice).

- [ ] **Step 8: Commit**

```bash
git add src/gateway/execution_engine/run_loop/inner.rs src/gateway/execution_engine/tool_service_builder.rs
git commit -m "gateway: wire deferred MCP tier + tool_search registration at the request seam (defer_mcp_tools)"
```

---

### Task 5: End-to-end escape-hatch + deferral integration check

A single integration-style test proving the two invariants: (a) OFF → surface byte-identical; (b) ON → MCP tool absent from the model list but reachable via `tool_search` + `execute`.

**Files:**
- Test: `src/gateway/execution_engine/tool_service_builder.rs` inline (or a small integration test module near the seam)

- [ ] **Step 1: Write the invariant test**

Add to `src/gateway/execution_engine/tool_service_builder.rs` tests:

```rust
    #[tokio::test]
    async fn off_vs_on_deferral_surface() {
        let make = || {
            let mut reg = LoopToolRegistry::new();
            reg.register(Box::new(StubTool));   // read_file
            reg.register(Box::new(OtherStub));  // web_fetch (stands in for an MCP tool)
            Arc::new(reg)
        };
        // OFF: empty deferred set → both listed.
        let off = build_request_tool_service(
            make(), BTreeSet::new(), None, None, None, None, "",
            None, false, &[], false, BTreeSet::new(),
        );
        let off_names: Vec<String> = off.list().await.into_iter().map(|d| d.name).collect();
        assert!(off_names.contains(&"web_fetch".to_string()));

        // ON: web_fetch deferred → absent from list, still describable/executable.
        let on = build_request_tool_service(
            make(), BTreeSet::new(), None, None, None, None, "",
            None, false, &[], false, ["web_fetch".to_string()].into_iter().collect(),
        );
        let on_names: Vec<String> = on.list().await.into_iter().map(|d| d.name).collect();
        assert!(!on_names.contains(&"web_fetch".to_string()));
        assert!(on.describe("web_fetch").await.is_some());
        assert!(on.execute("web_fetch", json!({})).await.is_ok());
    }
```

- [ ] **Step 2: Run + verify**

Run: `cargo test -p alephcore --lib tool_service_builder::tests::off_vs_on_deferral_surface`
Expected: PASS.

- [ ] **Step 3: Final scoped compile of touched crates**

Run: `cargo check -p alephcore --lib`
Expected: clean. (One consolidated check at the end honors the cargo-economy constraint.)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/tool_service_builder.rs
git commit -m "gateway: cover deferral off/on surface invariants (escape-hatch + searchable-but-hidden)"
```

---

## Self-Review

**Spec coverage (C1 of `2026-07-06-tools-3.2-3.5-toolsearch-design.md`):**
- Three-tier model (Direct/Collapsed/Deferred) → Direct+Collapsed pre-exist (`ProgressiveDisclosureRewriter`); Deferred added via Tasks 3+4 ✓
- `ToolExposure` classification → realized as the name-set computed at the seam (Task 4) rather than a per-tool classifier, because the presentation-layer `ToolDefinition.source` is hardcoded `Builtin` (documented deviation; simpler + avoids touching the loop-side type) ✓
- Deferral filter at ScopedToolService `list()`/`metadata_schema()`, not `describe()` → Task 3 ✓
- `ToolSearchTool` mirroring `SchemaLookupTool` registration; corpus = pre-collapse snapshot; top-K with schema → Tasks 2+4 ✓
- BM25 self-contained, zero deps, identifier-aware → Task 2 ✓
- Default OFF / escape-hatch byte-identical → Task 1 (`defer_mcp_tools=false`) + Task 5 invariant ✓
- Deferred scoped to MCP (plugin deferral YAGNI-deferred — plugin identification at the seam is murky) → documented scope narrowing ✓
- A1×C1 orthogonality (catalog vs prompt layer) → unaffected; C1 never touches ToolCatalog ✓

**Placeholder scan:** no TBD/TODO; every code step shows real code. Test harness reuse (`StubTool`/`NamedStub`) is spelled out with fallback definitions.

**Type consistency:** `ToolDoc { name, description, schema }` used identically in Task 2 (def) and Task 4 (corpus build, mapping loop-side `d.parameters → schema`). `ToolSearchTool::{new, NAME}` consistent. `with_deferred(BTreeSet<String>)` signature matches all call sites. `build_request_tool_service` trailing `deferred_tool_names: BTreeSet<String>` param added consistently at the def + all 3 call sites (2 production seam + test). Loop-side schema field is `parameters` (not `input_schema`) — mapped correctly.

**Ordering invariant:** Task 1 (config) → 2 (tool) → 3 (filter) → 4 (wire) → 5 (integration). Task 4 depends on 1+2+3; Task 5 depends on 4. `mcp_tool_names` declaration must be hoisted above the `if let Some(mcp_registry)` block (Task 4 Step 4) so it is in scope at Step 5.
