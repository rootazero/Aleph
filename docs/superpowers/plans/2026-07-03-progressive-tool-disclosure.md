# Progressive Tool Disclosure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut per-turn context ~56% (50.6K → ~22K) by collapsing non-core tools' `input_schema` to an open placeholder while keeping name+description visible, and letting the model load full schemas on demand via a `get_tool_schema` tool.

**Architecture:** A request-time `ToolDefinitionRewriter` (existing seam on `ScopedToolService`) strips non-core tools' `input_schema` and appends a retrieval hint to their description. All tools stay in the provider `tools` array (their name+description ARE the catalog). A per-request `get_tool_schema` `LoopTool` holds a snapshot of every tool's original full schema and serves it on demand. A `[tools] core` config list controls which tools stay full; `core = ["*"]` or empty disables collapsing (byte-identical to today). Zero changes to `src/harness/`, `src/executor/`, or the boot path.

**Tech Stack:** Rust (tokio + serde + serde_json), `async_trait`, existing `ToolDefinitionRewriter` / `LoopTool` traits.

## Global Constraints

- MSRV 1.95; tokio is the only async runtime; serde is the only serialization stack; no platform-API crates in `src/`.
- **Zero changes to `src/harness/`** (R10 file budget) and **zero changes to `src/executor/`**. Boot is touched in exactly ONE place: a 2-line passthrough at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` sourcing `[tools] core` into `ExecutionEngineConfig` (shaped identically to the adjacent `scratchpad_progress_push: app_config.execution.progress_push`). This minimal passthrough is REQUIRED — the escape hatch cannot work without it (user-approved 2026-07-03). No other boot logic changes. All remaining logic lives in `src/config/`, `src/tools/`, `src/gateway/execution_engine/`, `src/thinker/`.
- **Escape hatch:** `core = ["*"]` or empty ⇒ no rewriter attached ⇒ request bytes identical to pre-feature behavior. This must hold.
- The rewriter MUST NOT rename tools (`def.name` untouched) — dispatch resolves handlers by name.
- Cargo frugality (user rule): each task runs ONE targeted `cargo test -p alephcore --lib <module_path>`, never the full suite. No `cargo build`/full `cargo test`.
- Code comments in English. Commit messages: `<scope>: <description>` (English).
- Core default set (19 tools kept full): `ask_user, subagent, bash, code_exec, code_check, file_read, file_write, file_edit, file_ops, search, web_fetch, memory_search, remember, skill_read, skill_list, scratchpad, note_manage, system, get_tool_schema`. (`subagent` added post-final-review: it is attached out-of-band, so if collapsed its schema would be absent from the get_tool_schema snapshot — keeping it core avoids a misleading failed lookup on the default path.)

---

### Task 1: `[tools] core` + `truncate_tool_descriptions` config

**Files:**
- Modify: `src/config/types/tools.rs` (add two fields to `ToolsConfig` at line ~96, a `default_core_tools()` fn, update `impl Default`)
- Test: `src/config/types/tools.rs` (`#[cfg(test)] mod` at file end)

**Interfaces:**
- Produces: `ToolsConfig.core: Vec<String>`, `ToolsConfig.truncate_tool_descriptions: bool`, `pub fn default_core_tools() -> Vec<String>`.

- [ ] **Step 1: Write the failing test**

Add to a `#[cfg(test)] mod tests` at the end of `src/config/types/tools.rs`:

```rust
#[cfg(test)]
mod core_tools_tests {
    use super::*;

    #[test]
    fn default_core_contains_essentials() {
        let c = ToolsConfig::default();
        assert!(c.core.iter().any(|t| t == "bash"));
        assert!(c.core.iter().any(|t| t == "get_tool_schema"));
        assert_eq!(c.core.len(), 18);
        assert!(!c.truncate_tool_descriptions);
    }

    #[test]
    fn core_roundtrips_and_supports_wildcard_sentinel() {
        let toml = r#"core = ["*"]
truncate_tool_descriptions = true"#;
        let c: ToolsConfig = toml::from_str(toml).unwrap();
        assert_eq!(c.core, vec!["*".to_string()]);
        assert!(c.truncate_tool_descriptions);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::types::tools::core_tools_tests`
Expected: FAIL — `no field 'core' on type 'ToolsConfig'` (compile error).

- [ ] **Step 3: Add the fields, default fn, and Default update**

In `struct ToolsConfig { … }` (after the existing `system_info_enabled` field) add:

```rust
    /// Tools kept at FULL schema in every request. Every other tool has its
    /// `input_schema` collapsed to an open placeholder (name + description stay
    /// visible); the model calls `get_tool_schema("<name>")` to load full
    /// parameters on demand. `["*"]` or empty = disable collapsing (all tools
    /// full — byte-identical to pre-feature behavior).
    #[serde(default = "default_core_tools")]
    pub core: Vec<String>,

    /// When true, non-core tools also have their description truncated to the
    /// first sentence (extra token savings at some discoverability cost).
    #[serde(default)]
    pub truncate_tool_descriptions: bool,
```

Add near `default_shell_timeout`:

```rust
/// Default "kept full" tool set — daily single-chat essentials plus the
/// on-demand schema loader. Non-core tools are schema-collapsed. See
/// `ProgressiveDisclosureRewriter`.
pub fn default_core_tools() -> Vec<String> {
    [
        "ask_user", "bash", "code_exec", "code_check",
        "file_read", "file_write", "file_edit", "file_ops",
        "search", "web_fetch", "memory_search", "remember",
        "skill_read", "skill_list", "scratchpad", "note_manage",
        "system", "get_tool_schema",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}
```

In `impl Default for ToolsConfig`, add the two fields to the returned literal:

```rust
            core: default_core_tools(),
            truncate_tool_descriptions: false,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::types::tools::core_tools_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/types/tools.rs
git commit -m "config: add [tools] core + truncate_tool_descriptions for progressive tool disclosure"
```

---

### Task 2: `ProgressiveDisclosureRewriter`

**Files:**
- Create: `src/tools/scoped/progressive_disclosure.rs`
- Modify: `src/tools/scoped/mod.rs` (add `mod progressive_disclosure;` near line 14-16, and re-export at the `pub use` near line 21)
- Test: `src/tools/scoped/progressive_disclosure.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `crate::tools::scoped::ToolDefinitionRewriter` (`src/tools/scoped/traits.rs:32`, `fn rewrite(&self, def: &mut ToolDefinition)`), `crate::tools::service::ToolDefinition` (type B: `name`, `description`, `input_schema: serde_json::Value`, …).
- Produces: `pub struct ProgressiveDisclosureRewriter`; `pub fn from_config(core: &[String], truncate_desc: bool) -> Option<Arc<dyn ToolDefinitionRewriter>>` (returns `None` when `core` is empty or contains `"*"`).

- [ ] **Step 1: Write the failing test**

Create `src/tools/scoped/progressive_disclosure.rs` with ONLY the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
    use serde_json::json;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "Does a thing. Second sentence.".to_string(),
            input_schema: json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn collapses_non_core_keeps_core() {
        let rw = ProgressiveDisclosureRewriter::new(["bash".into()].into_iter().collect(), false);
        let mut core = def("bash");
        let mut other = def("browser_navigate");
        rw.rewrite(&mut core);
        rw.rewrite(&mut other);
        // core untouched
        assert!(core.input_schema.get("properties").is_some());
        assert_eq!(core.description, "Does a thing. Second sentence.");
        // non-core collapsed + hint, name never renamed
        assert_eq!(other.name, "browser_navigate");
        assert!(other.input_schema.get("properties").is_none());
        assert_eq!(other.input_schema["additionalProperties"], json!(true));
        assert!(other.description.contains("get_tool_schema"));
    }

    #[test]
    fn truncate_desc_option_shortens_first_sentence() {
        let rw = ProgressiveDisclosureRewriter::new(std::collections::BTreeSet::new(), true);
        let mut d = def("x");
        rw.rewrite(&mut d);
        assert!(d.description.starts_with("Does a thing"));
        assert!(!d.description.contains("Second sentence"));
    }

    #[test]
    fn from_config_disabled_on_wildcard_or_empty() {
        assert!(ProgressiveDisclosureRewriter::from_config(&["*".into()], false).is_none());
        assert!(ProgressiveDisclosureRewriter::from_config(&[], false).is_none());
        assert!(ProgressiveDisclosureRewriter::from_config(&["bash".into()], false).is_some());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tools::scoped::progressive_disclosure`
Expected: FAIL — `ProgressiveDisclosureRewriter` not found (and module not declared).

- [ ] **Step 3: Write the implementation**

Prepend to `src/tools/scoped/progressive_disclosure.rs` (above the test module):

```rust
//! Request-time rewriter that implements progressive tool disclosure:
//! non-core tools get their `input_schema` collapsed to an open placeholder
//! (name + description stay visible) so the model can still discover them but
//! pays no schema tokens until it loads the full schema via `get_tool_schema`.
//!
//! This is a STATIC partition (core set is config, decided ahead of any
//! message) applied at the tool-presentation layer — not per-message intent
//! filtering. See CLAUDE.md R10 (第2不 例外注).

use std::collections::BTreeSet;

use serde_json::json;

use crate::sync_primitives::Arc;
use crate::tools::scoped::ToolDefinitionRewriter;
use crate::tools::service::ToolDefinition;

/// Collapses non-core tools' schemas at request time. Deterministic per
/// `(name, core, truncate)`, so it is safe under the `metadata_schema()`
/// generation cache.
pub struct ProgressiveDisclosureRewriter {
    core: BTreeSet<String>,
    truncate_desc: bool,
}

impl ProgressiveDisclosureRewriter {
    /// Construct directly from a resolved core set.
    #[must_use]
    pub fn new(core: BTreeSet<String>, truncate_desc: bool) -> Self {
        Self { core, truncate_desc }
    }

    /// Build from config. Returns `None` (⇒ attach nothing ⇒ old behavior)
    /// when `core` is empty or contains the `"*"` wildcard sentinel.
    #[must_use]
    pub fn from_config(core: &[String], truncate_desc: bool) -> Option<Arc<dyn ToolDefinitionRewriter>> {
        if core.is_empty() || core.iter().any(|c| c == "*") {
            return None;
        }
        let set: BTreeSet<String> = core.iter().cloned().collect();
        Some(Arc::new(Self::new(set, truncate_desc)))
    }
}

impl ToolDefinitionRewriter for ProgressiveDisclosureRewriter {
    fn rewrite(&self, def: &mut ToolDefinition) {
        if self.core.contains(&def.name) {
            return; // keep full schema + description
        }
        // Collapse to an open object so the eventual real call is accepted
        // by the provider (the model supplies args learned via get_tool_schema).
        def.input_schema = json!({ "type": "object", "additionalProperties": true });

        if self.truncate_desc {
            if let Some((head, _)) = def.description.split_once(". ") {
                def.description = head.to_string();
            }
        }
        def.description.push_str(&format!(
            " [Parameters collapsed — call get_tool_schema(tool_name=\"{}\") to load the full input schema before calling this tool.]",
            def.name
        ));
    }
}
```

In `src/tools/scoped/mod.rs`: add `mod progressive_disclosure;` alongside the other `mod` lines (14-16), and add `pub use progressive_disclosure::ProgressiveDisclosureRewriter;` next to the existing `pub use traits::{…};` (line 21).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib tools::scoped::progressive_disclosure`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/tools/scoped/progressive_disclosure.rs src/tools/scoped/mod.rs
git commit -m "tools: add ProgressiveDisclosureRewriter (collapse non-core tool schemas)"
```

---

### Task 3: `get_tool_schema` LoopTool (on-demand full-schema loader)

**Files:**
- Create: `src/tools/schema_lookup.rs`
- Modify: `src/tools/mod.rs` (add `pub mod schema_lookup;`)
- Test: `src/tools/schema_lookup.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `crate::tools::runtime::{LoopTool, ToolResult}` (`ToolResult::Success { output: Value }` / `ToolResult::Error { error: String, retryable: bool }`); `crate::tools::name_repair::suggest_candidates(query: &str, offered: &[&str], k: usize) -> Vec<String>`.
- Produces: `pub struct SchemaLookupTool`; `SchemaLookupTool::NAME == "get_tool_schema"`; `pub fn new(schemas: Arc<HashMap<String, serde_json::Value>>) -> Self`.

- [ ] **Step 1: Write the failing test**

Create `src/tools/schema_lookup.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn tool() -> SchemaLookupTool {
        let mut m = std::collections::HashMap::new();
        m.insert("browser_navigate".to_string(), json!({"type":"object","properties":{"url":{"type":"string"}}}));
        SchemaLookupTool::new(std::sync::Arc::new(m))
    }

    #[tokio::test]
    async fn returns_full_schema_when_found() {
        let out = tool().execute(json!({"tool_name":"browser_navigate"}), CancellationToken::new()).await;
        match out {
            ToolResult::Success { output } => {
                assert_eq!(output["found"], json!(true));
                assert!(output["parameters"]["properties"]["url"].is_object());
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn suggests_on_miss() {
        let out = tool().execute(json!({"tool_name":"browser_navigat"}), CancellationToken::new()).await;
        match out {
            ToolResult::Success { output } => {
                assert_eq!(output["found"], json!(false));
                assert!(output["suggestions"].as_array().unwrap().iter().any(|s| s == "browser_navigate"));
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn errors_on_empty_name() {
        let out = tool().execute(json!({}), CancellationToken::new()).await;
        assert!(matches!(out, ToolResult::Error { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tools::schema_lookup`
Expected: FAIL — `SchemaLookupTool` not found / module not declared.

- [ ] **Step 3: Write the implementation**

Prepend to `src/tools/schema_lookup.rs`:

```rust
//! `get_tool_schema` — on-demand loader for the full input schema of a tool
//! whose schema was collapsed by `ProgressiveDisclosureRewriter`. Registered
//! per-request with a snapshot of every tool's ORIGINAL (pre-collapse) schema.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, ToolResult};

/// Serves original tool schemas from a per-request snapshot (`name → schema`).
pub struct SchemaLookupTool {
    schemas: Arc<HashMap<String, Value>>,
}

impl SchemaLookupTool {
    /// Tool name advertised to the model.
    pub const NAME: &'static str = "get_tool_schema";

    #[must_use]
    pub fn new(schemas: Arc<HashMap<String, Value>>) -> Self {
        Self { schemas }
    }
}

#[async_trait]
impl LoopTool for SchemaLookupTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn description(&self) -> &str {
        "Load the full JSON input schema for a tool whose parameters are collapsed. \
         Call this with the tool's exact name before invoking any tool whose description \
         says '[Parameters collapsed …]', then call that tool with the returned parameters."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_name": { "type": "string", "description": "Exact name of the tool to load the schema for." }
            },
            "required": ["tool_name"]
        })
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        let name = input.get("tool_name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return ToolResult::Error {
                error: "get_tool_schema requires a non-empty `tool_name`.".to_string(),
                retryable: false,
            };
        }
        if let Some(schema) = self.schemas.get(name) {
            ToolResult::Success {
                output: json!({ "found": true, "name": name, "parameters": schema }),
            }
        } else {
            let offered: Vec<&str> = self.schemas.keys().map(String::as_str).collect();
            let suggestions = crate::tools::name_repair::suggest_candidates(name, &offered, 5);
            ToolResult::Success {
                output: json!({
                    "found": false,
                    "name": name,
                    "error": format!("No tool named '{name}'."),
                    "suggestions": suggestions,
                }),
            }
        }
    }
}
```

Add `pub mod schema_lookup;` to `src/tools/mod.rs` (alphabetically near the other `pub mod` lines).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib tools::schema_lookup`
Expected: PASS (3 tests). If `suggest_candidates`'s signature differs, adjust the call to match `src/tools/name_repair.rs` (it is called as `suggest_candidates(&query, &offered, 5)` at `src/builtin_tools/meta_tools.rs:272`).

- [ ] **Step 5: Commit**

```bash
git add src/tools/schema_lookup.rs src/tools/mod.rs
git commit -m "tools: add get_tool_schema LoopTool (on-demand full schema loader)"
```

---

### Task 4: Wire rewriter + `get_tool_schema` into the request path

**Files:**
- Modify: `src/gateway/execution_engine/tool_service_builder.rs` (`build_request_tool_service` signature + attach rewriter, near line 86-137)
- Modify: `src/gateway/execution_engine/run_loop/inner.rs` (register `get_tool_schema` before the `let loop_registry = Arc::new(loop_registry_inner);` line ~600; update both `build_request_tool_service` call sites ~629 and ~810)
- Modify (config carrier): `src/gateway/execution_engine/mod.rs` — add `pub core_tools: Vec<String>` + `pub truncate_tool_descriptions: bool` to `ExecutionEngineConfig` (struct ~line 67) and to its `impl Default` (~line 94): `core_tools: crate::config::types::tools::default_core_tools()`, `truncate_tool_descriptions: false`.
- Modify (boot passthrough — the ONE allowed boot touch): `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` (~line 724, inside the `ExecutionEngineConfig { … }` literal that already sets `scratchpad_progress_push: app_config.execution.progress_push`) — add `core_tools: app_config.tools.core.clone(),` and `truncate_tool_descriptions: app_config.tools.truncate_tool_descriptions,`. `app_config: &alephcore::Config` has `tools: ToolsConfig` (structs.rs:87).
- Test: `src/gateway/execution_engine/tool_service_builder.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `ProgressiveDisclosureRewriter::from_config` (Task 2), `SchemaLookupTool::new` (Task 3), `ScopedToolService::with_definition_rewriter` (`src/tools/scoped/builder.rs:79`).
- Produces: `build_request_tool_service(..., core_tools: &[String], truncate_tool_descriptions: bool)`.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod` in `tool_service_builder.rs` (mirror an existing test's registry setup; the key assertion is the partition). If no test module exists, create one:

```rust
#[cfg(test)]
mod progressive_tests {
    use super::*;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;
    use std::collections::BTreeSet;

    struct Fat(&'static str);
    #[async_trait]
    impl LoopTool for Fat {
        fn name(&self) -> &str { self.0 }
        fn description(&self) -> &str { "fat tool" }
        fn schema(&self) -> Value { json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"]}) }
        async fn execute(&self, _i: Value, _c: CancellationToken) -> ToolResult { ToolResult::Success { output: json!({}) } }
    }

    #[tokio::test]
    async fn non_core_schema_collapsed_core_kept() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(Fat("bash")));
        reg.register(Box::new(Fat("browser_navigate")));
        let svc = build_request_tool_service(
            std::sync::Arc::new(reg), BTreeSet::new(), None, None, None, None, "",
            None, false,
            &["bash".to_string()], false, // core, truncate
        );
        let schema = svc.metadata_schema();
        let bash = schema.iter().find(|d| d.name == "bash").unwrap();
        let nav = schema.iter().find(|d| d.name == "browser_navigate").unwrap();
        assert!(bash.parameters.get("properties").is_some());          // core kept
        assert!(nav.parameters.get("properties").is_none());           // non-core collapsed
        assert_eq!(nav.parameters["additionalProperties"], json!(true));
    }

    #[tokio::test]
    async fn wildcard_core_keeps_all_full() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(Fat("browser_navigate")));
        let svc = build_request_tool_service(
            std::sync::Arc::new(reg), BTreeSet::new(), None, None, None, None, "",
            None, false,
            &["*".to_string()], false,
        );
        let nav = svc.metadata_schema().iter().find(|d| d.name == "browser_navigate").cloned().unwrap();
        assert!(nav.parameters.get("properties").is_some());           // escape hatch: full
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::execution_engine::tool_service_builder::progressive_tests`
Expected: FAIL — `build_request_tool_service` takes 9 args, not 11 (compile error).

- [ ] **Step 3: Add params + attach the rewriter**

In `tool_service_builder.rs`, add two params to the end of `build_request_tool_service`'s signature:

```rust
    unattended: bool,
    core_tools: &[String],
    truncate_tool_descriptions: bool,
) -> Arc<dyn ToolService> {
```

After the last `with_*` wiring and before `Arc::new(svc)` (the function's return), attach the rewriter:

```rust
    // Progressive tool disclosure: collapse non-core tool schemas. `None`
    // (core empty / ["*"]) leaves the service byte-identical to old behavior.
    if let Some(rewriter) =
        crate::tools::scoped::ProgressiveDisclosureRewriter::from_config(core_tools, truncate_tool_descriptions)
    {
        svc = svc.with_definition_rewriter(rewriter);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::execution_engine::tool_service_builder::progressive_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Register `get_tool_schema` + thread config at the call sites**

In `run_loop/inner.rs`, immediately BEFORE `let loop_registry = Arc::new(loop_registry_inner);` (line ~600), insert:

```rust
            // Snapshot every tool's ORIGINAL full schema before progressive
            // disclosure collapses them, then register the on-demand loader.
            let schema_snapshot: std::collections::HashMap<String, serde_json::Value> =
                loop_registry_inner
                    .tool_definitions()
                    .into_iter()
                    .map(|d| (d.name, d.parameters))
                    .collect();
            loop_registry_inner.register(Box::new(
                crate::tools::schema_lookup::SchemaLookupTool::new(std::sync::Arc::new(schema_snapshot)),
            ));
            // Ensure the loader survives a non-empty allow-filter. CRITICAL:
            // mirror the MCP-join pattern below (search `if !allowed_names.is_empty()`,
            // ~lines 583-588). An EMPTY `allowed_names` means allow-all in
            // ScopedToolService; inserting a name into an empty set would flip it
            // to a 1-element allowlist and hide EVERY other tool. Only widen an
            // already-restrictive (non-empty) allow-set — when empty, get_tool_schema
            // is already visible under allow-all.
            if !allowed_names.is_empty() {
                allowed_names
                    .insert(crate::tools::schema_lookup::SchemaLookupTool::NAME.to_string());
            }
```

At BOTH `build_request_tool_service(...)` calls (lines ~629 and ~810), append the two new args after `unattended`:

```rust
                    unattended,
                    &self.config.core_tools,
                    self.config.truncate_tool_descriptions,
                );
```

Thread the config (resolved — `self.config` is `ExecutionEngineConfig`):

1. In `src/gateway/execution_engine/mod.rs`, add to `struct ExecutionEngineConfig` (after `scratchpad_progress_push`):
   ```rust
       /// Tools kept at full schema (progressive tool disclosure). Sourced from
       /// `[tools] core`. `["*"]`/empty disables collapsing (escape hatch).
       pub core_tools: Vec<String>,
       /// Mirror of `[tools] truncate_tool_descriptions`.
       pub truncate_tool_descriptions: bool,
   ```
   and to its `impl Default for ExecutionEngineConfig`:
   ```rust
           core_tools: crate::config::types::tools::default_core_tools(),
           truncate_tool_descriptions: false,
   ```
2. In `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` (~line 724), add two lines inside the existing `ExecutionEngineConfig { … }` literal (right next to `scratchpad_progress_push: app_config.execution.progress_push,`):
   ```rust
           core_tools: app_config.tools.core.clone(),
           truncate_tool_descriptions: app_config.tools.truncate_tool_descriptions,
   ```
   Do not touch any other boot logic. This is the ONLY permitted boot change.

`self.config.core_tools` / `self.config.truncate_tool_descriptions` are then live at both call sites (used in the arg-append above).

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::execution_engine::run_loop`
Expected: PASS (existing run_loop tests still green; no regression).

- [ ] **Step 7: Commit**

```bash
git add src/gateway/execution_engine/tool_service_builder.rs src/gateway/execution_engine/run_loop/inner.rs
git commit -m "gateway: wire progressive tool disclosure (rewriter + get_tool_schema) into request path"
```

---

### Task 5: Prompt guidance layer

**Files:**
- Create: `src/thinker/layers/progressive_tools.rs`
- Modify: `src/thinker/layers/mod.rs` (declare + export the layer), `src/thinker/prompt_pipeline.rs` (register in `default_layers()` near line 281)
- Test: `src/thinker/layers/progressive_tools.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `crate::thinker::prompt_layer::{PromptLayer, LayerInput, AssemblyPath}` — mirror the trait-impl skeleton of `src/thinker/layers/skill_instructions.rs` (its `priority()` / `paths()` / `supports_mode()` / `inject()` methods).

- [ ] **Step 1: Write the failing test**

Create `src/thinker/layers/progressive_tools.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_layer::PromptLayer;

    #[test]
    fn injects_disclosure_protocol() {
        let mut out = String::new();
        let input = crate::thinker::prompt_layer::LayerInput::for_test(); // use the same test ctor skill_instructions.rs tests use
        ProgressiveToolsLayer.inject(&mut out, &input);
        assert!(out.contains("get_tool_schema"));
        assert!(out.to_lowercase().contains("collapsed"));
    }
}
```

(If `LayerInput` has no `for_test`, construct it the same way the tests in `src/thinker/layers/skill_instructions.rs` do.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::layers::progressive_tools`
Expected: FAIL — `ProgressiveToolsLayer` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `src/thinker/layers/progressive_tools.rs` (copy the method skeleton — `priority`/`paths`/`supports_mode`/`stability` — from `skill_instructions.rs`; only `inject` body differs). `priority()` returns `1051` (just after `SkillInstructionsLayer`'s `1050`); `supports_mode` = `Full` only.

```rust
//! One-line explanation of the progressive-disclosure protocol, so the model
//! reliably understands the per-tool "[Parameters collapsed …]" hints.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer /*, LayerStability, PromptMode */};

pub struct ProgressiveToolsLayer;

const GUIDANCE: &str = "\n## Tool schemas\n\n\
Some tools show `[Parameters collapsed …]` in their description: their full \
input schema is not loaded. Before calling such a tool, call \
`get_tool_schema(tool_name=\"<name>\")` to load its parameters, then call the \
tool with them. Tools without that marker are ready to call directly.\n";

impl PromptLayer for ProgressiveToolsLayer {
    fn priority(&self) -> u32 { 1051 }
    // paths()/supports_mode()/stability(): copy verbatim from SkillInstructionsLayer
    // (same AssemblyPath set, Full-only mode).
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str(GUIDANCE);
    }
}
```

Declare the module + export in `src/thinker/layers/mod.rs`, and add `Box::new(ProgressiveToolsLayer)` to the `default_layers()` list in `src/thinker/prompt_pipeline.rs` (~line 310, next to `Box::new(SkillInstructionsLayer)`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib thinker::layers::progressive_tools`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/progressive_tools.rs src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "thinker: add ProgressiveToolsLayer prompt guidance for collapsed tool schemas"
```

---

### Task 6: Token-gate acceptance test

**Files:**
- Test: `src/tools/scoped/progressive_disclosure.rs` (extend the `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ProgressiveDisclosureRewriter` (Task 2).

- [ ] **Step 1: Write the test**

Add to the test module in `progressive_disclosure.rs`:

```rust
    #[test]
    fn collapsing_shrinks_serialized_tools_by_half() {
        // 20 fat tools (schema-heavy) + 2 core.
        let core: std::collections::BTreeSet<String> = ["bash".into(), "get_tool_schema".into()].into_iter().collect();
        let rw = ProgressiveDisclosureRewriter::new(core.clone(), false);
        let fat_schema = json!({
            "type":"object",
            "properties": (0..12).map(|i| (format!("field_{i}"), json!({"type":"string","description":"a reasonably long description of this parameter that costs tokens"}))).collect::<serde_json::Map<_,_>>(),
        });
        let mut defs: Vec<ToolDefinition> = (0..22).map(|i| ToolDefinition {
            name: if i < 2 { ["bash","get_tool_schema"][i].to_string() } else { format!("tool_{i}") },
            description: "does a thing".to_string(),
            input_schema: fat_schema.clone(),
            source: crate::tools::service::ToolSource::Builtin,
            metadata: crate::tools::service::ToolDefinitionMetadata::default(),
        }).collect();
        let before = serde_json::to_string(&defs).unwrap().len();
        for d in &mut defs { rw.rewrite(d); }
        let after = serde_json::to_string(&defs).unwrap().len();
        assert!(after * 2 < before, "expected >50% shrink, got {before} -> {after}");
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p alephcore --lib tools::scoped::progressive_disclosure`
Expected: PASS (4 tests total in the module).

- [ ] **Step 3: Commit**

```bash
git add src/tools/scoped/progressive_disclosure.rs
git commit -m "tools: add token-gate test asserting >50% tool-schema shrink"
```

---

## Post-implementation manual verification (not a code step)

After all tasks land and the binary is rebuilt + restarted, send a plain "你好" and capture the provider request (same method as the original investigation). Expected: `tools` array total ≈ 11–12K tokens (from ~40K), total request ≈ 22K (from ~50.6K), and `get_tool_schema` present. Then ask something needing a collapsed tool (e.g. browser) and confirm the model calls `get_tool_schema` first, then the tool succeeds.
