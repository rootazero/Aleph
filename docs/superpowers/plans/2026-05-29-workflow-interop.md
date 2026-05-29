# Workflow Interop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Aleph workflow 子系统新增与 Claude Code `.workflow.js` 工程文件的双向互换(导出 / 导入)+ 声明式 AWI manifest 规范 + 文档,执行核心(`WorkflowDef`/`compile`/dispatcher)零改动。

**Architecture:** 新增独立 `src/workflow/interop/` 纯数据层。`WorkflowManifest`(AWI 超集)是规范唯一真相,承载全部 `.workflow.js` 兼容元数据;只有可执行内核映射进现有 `WorkflowDef`,其余元数据经导出文件头的 `/* @aleph-workflow {json} */` 内嵌块实现无损往返。`workflow` R8 工具新增 `export` / `import` 两个动作。导入裸 `.workflow.js` 用手写轻量扫描(无 JS 引擎),命令式构造经 `dropped[]` 透明上报。

**Tech Stack:** Rust,`serde` / `serde_json`(已有依赖),`schemars::JsonSchema`,`async_trait`,`tempfile`(测试)。验证:`cargo check -p alephcore` + `cargo test -p alephcore --lib workflow`。**绝不** `cargo fmt`。

**红线对照:** R3 零新重依赖;R7/R10 纯数据变换、无推理、命令式控制流明确拒绝;R8 工具动作。

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/workflow/interop/manifest.rs` | `WorkflowManifest` / `WorkflowPhase` / `WorkflowManifestStep` 类型 + `from_def` / `to_def` 双向映射 | 新建 |
| `src/workflow/interop/export.rs` | `render_workflow_js` + 拓扑分层 helper + 内嵌块/字符串转义 | 新建 |
| `src/workflow/interop/import.rs` | `parse_workflow_js` → `ImportOutcome { manifest, dropped }`,三条解析路径 | 新建 |
| `src/workflow/interop/mod.rs` | 重导出 | 新建 |
| `src/workflow/mod.rs` | 挂 `pub mod interop;` + 重导出 | 改 `:19-25` |
| `src/builtin_tools/workflow_tool.rs` | `WorkflowArgs` +2 动作、`WorkflowToolOutput` +2 字段、`call` +2 分支、examples/description、测试 | 改 |
| `docs/reference/WORKFLOW_INTEROP.md` | 规范文档 | 新建 |
| `CLAUDE.md` | 文档索引一行 | 改 |

参考已有签名(实现时遵循):
- `AlephError::invalid_input<S: Into<String>>(msg)` (`src/error.rs:604`)、`AlephError::config<S: Into<String>>(msg)` (`src/error.rs:255`)。
- `crate::canvas_io::sanitise_name(raw: &str) -> String` (`src/canvas_io.rs:44`)。
- `WorkflowDef { name, description, steps }`、`WorkflowStepDef { id, agent, prompt, depends_on }`(`src/workflow/def.rs`)。
- store 原子写模式(temp + rename)见 `src/workflow/store.rs:56-75`。

---

## Task 1: AWI manifest 类型与双向映射

**Files:**
- Create: `src/workflow/interop/manifest.rs`

- [ ] **Step 1: 写失败测试**

先创建文件,内容为类型定义占位 + 测试(类型在 Step 3 填实,此步只放测试让其因缺类型而编译失败)。直接写完整文件骨架:

```rust
//! AWI (Aleph Workflow Interchange) manifest — the declarative superset that
//! bridges Aleph's `WorkflowDef` and Claude Code's `.workflow.js` format.
//!
//! Pure data + field shuffling, no reasoning (R7/R10). The manifest carries the
//! full `.workflow.js`-compatible metadata (`whenToUse`, `phases`, per-step
//! `label`/`model`/`phase`/`schema`); only the executable core round-trips into
//! `WorkflowDef`, the rest is preserved for lossless export.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workflow::def::{WorkflowDef, WorkflowStepDef};

// NOTE: no `JsonSchema` derive — these types never appear in a tool arg schema
// (the `workflow` tool's args use `WorkflowDef`), so deriving it would be dead
// surface (R10/YAGNI) and needlessly assume `serde_json::Value: JsonSchema`.

/// Declarative interchange manifest. JSON keys are camelCase to match the
/// `.workflow.js` `meta` block (`dependsOn`, `whenToUse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when_to_use: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<WorkflowPhase>,
    pub steps: Vec<WorkflowManifestStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowManifestStep {
    pub id: String,
    pub agent: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Opaque JSON Schema, passed through verbatim. Aleph never interprets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

impl WorkflowManifest {
    /// Build a manifest from the executable core. Extra metadata fields start
    /// empty/None — a `WorkflowDef` carries none of them.
    pub fn from_def(def: &WorkflowDef) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            when_to_use: String::new(),
            phases: Vec::new(),
            steps: def
                .steps
                .iter()
                .map(|s| WorkflowManifestStep {
                    id: s.id.clone(),
                    agent: s.agent.clone(),
                    prompt: s.prompt.clone(),
                    depends_on: s.depends_on.clone(),
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                })
                .collect(),
        }
    }

    /// Project to the executable core, dropping extra metadata. Callers
    /// typically `validate()` the result before persisting or running.
    pub fn to_def(&self) -> WorkflowDef {
        WorkflowDef {
            name: self.name.clone(),
            description: self.description.clone(),
            steps: self
                .steps
                .iter()
                .map(|s| WorkflowStepDef {
                    id: s.id.clone(),
                    agent: s.agent.clone(),
                    prompt: s.prompt.clone(),
                    depends_on: s.depends_on.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_def() -> WorkflowDef {
        WorkflowDef {
            name: "rep".into(),
            description: "demo".into(),
            steps: vec![
                WorkflowStepDef {
                    id: "a".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                },
                WorkflowStepDef {
                    id: "b".into(),
                    agent: "writer".into(),
                    prompt: "write".into(),
                    depends_on: vec!["a".into()],
                },
            ],
        }
    }

    #[test]
    fn from_def_then_to_def_preserves_core() {
        let def = core_def();
        let manifest = WorkflowManifest::from_def(&def);
        assert_eq!(manifest.to_def(), def);
    }

    #[test]
    fn from_def_leaves_extras_empty() {
        let manifest = WorkflowManifest::from_def(&core_def());
        assert!(manifest.when_to_use.is_empty());
        assert!(manifest.phases.is_empty());
        assert!(manifest.steps.iter().all(|s| s.label.is_none()
            && s.model.is_none()
            && s.phase.is_none()
            && s.schema.is_none()));
    }

    #[test]
    fn to_def_drops_extra_metadata() {
        let manifest = WorkflowManifest {
            name: "x".into(),
            description: "d".into(),
            when_to_use: "when".into(),
            phases: vec![WorkflowPhase {
                title: "P".into(),
                detail: "det".into(),
            }],
            steps: vec![WorkflowManifestStep {
                id: "s".into(),
                agent: "ag".into(),
                prompt: "p".into(),
                depends_on: vec![],
                label: Some("L".into()),
                model: Some("haiku".into()),
                phase: Some("P".into()),
                schema: Some(serde_json::json!({"type":"object"})),
            }],
        };
        let def = manifest.to_def();
        assert_eq!(def.name, "x");
        assert_eq!(def.steps.len(), 1);
        assert_eq!(def.steps[0].id, "s");
        // WorkflowStepDef has no label/model/phase/schema fields to carry —
        // their absence is structural.
    }

    #[test]
    fn serde_uses_camel_case_keys() {
        let manifest = WorkflowManifest {
            name: "x".into(),
            description: String::new(),
            when_to_use: "w".into(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "s".into(),
                agent: "ag".into(),
                prompt: "p".into(),
                depends_on: vec!["dep".into()],
                label: None,
                model: None,
                phase: None,
                schema: None,
            }],
        };
        let v = serde_json::to_value(&manifest).unwrap();
        assert!(v.get("whenToUse").is_some(), "whenToUse camelCase");
        assert!(v["steps"][0].get("dependsOn").is_some(), "dependsOn camelCase");
        // Empty extras are skipped on the wire.
        assert!(v.get("phases").is_none(), "empty phases skipped");
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let manifest = WorkflowManifest::from_def(&core_def());
        let s = serde_json::to_string(&manifest).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(manifest, back);
    }
}
```

注意:此步还**不能**编译,因为 `src/workflow/interop/mod.rs` 与 `src/workflow/mod.rs` 的挂载在 Task 4。为让 Task 1 可独立 `cargo test`,**本步同时**临时在 `src/workflow/mod.rs:21` 后插入 `pub mod interop;`,并新建最小 `src/workflow/interop/mod.rs` 内容 `pub mod manifest;`。(Task 4 再补全重导出。)

- [ ] **Step 2: 运行测试,确认失败/编译**

Run: `cargo test -p alephcore --lib workflow::interop::manifest`
Expected: 编译通过、5 个测试 PASS(类型与实现已在 Step 1 一并写出,这是“写完即绿”的纯数据模块;若有编译错按报错修正)。

- [ ] **Step 3: 提交**

```bash
git add src/workflow/interop/mod.rs src/workflow/interop/manifest.rs src/workflow/mod.rs
git commit -m "workflow: add AWI interchange manifest type + WorkflowDef mapping"
```

---

## Task 2: 导出器 `render_workflow_js`

**Files:**
- Create: `src/workflow/interop/export.rs`
- Modify: `src/workflow/interop/mod.rs`(加 `pub mod export;`)

- [ ] **Step 1: 写实现 + 失败测试**

新建 `src/workflow/interop/export.rs`:

```rust
//! Render an AWI manifest into a Claude-Code-compatible `.workflow.js`.
//!
//! Deterministic string rendering — no reasoning. The static dependency DAG is
//! reconstructed into the declarative `phase()` / `parallel()` / sequential
//! `agent()` skeleton; imperative control flow is never emitted (Aleph's source
//! has none). A `/* @aleph-workflow {json} */` header carries the full manifest
//! for lossless re-import.

use std::collections::HashMap;

use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

/// Embedded round-trip marker. `import` reads the JSON between prefix/suffix.
pub const EMBED_PREFIX: &str = "/* @aleph-workflow ";
pub const EMBED_SUFFIX: &str = " */";

/// Render `manifest` as a `.workflow.js` source string.
pub fn render_workflow_js(manifest: &WorkflowManifest) -> String {
    let manifest_json = serde_json::to_string(manifest).unwrap_or_else(|_| "{}".to_string());
    let mut out = String::new();

    // 1. Lossless round-trip header.
    out.push_str(EMBED_PREFIX);
    out.push_str(&manifest_json);
    out.push_str(EMBED_SUFFIX);
    out.push('\n');

    // 2. meta block (pure literal).
    out.push_str(&render_meta(manifest));
    out.push('\n');

    // 3. Body: topological layers → parallel/sequential agent() skeleton.
    match topo_levels(manifest) {
        Some(levels) => {
            let mut last_phase: Option<&str> = None;
            for layer in &levels {
                if let Some(&first) = layer.first() {
                    if let Some(ph) = manifest.steps[first].phase.as_deref() {
                        if last_phase != Some(ph) {
                            out.push_str(&format!("phase({})\n", js_str(ph)));
                            last_phase = Some(ph);
                        }
                    }
                }
                if layer.len() == 1 {
                    out.push_str(&format!("await {}\n", render_agent_call(&manifest.steps[layer[0]])));
                } else {
                    out.push_str("await parallel([\n");
                    for &i in layer {
                        out.push_str(&format!("  () => {},\n", render_agent_call(&manifest.steps[i])));
                    }
                    out.push_str("])\n");
                }
            }
        }
        None => {
            // Cycle / unknown dep — should not happen for a validated manifest.
            // Degrade to a flat sequence rather than panicking.
            for step in &manifest.steps {
                out.push_str(&format!("await {}\n", render_agent_call(step)));
            }
        }
    }

    out
}

/// Render the `export const meta = {...}` literal.
fn render_meta(manifest: &WorkflowManifest) -> String {
    let mut phases = String::new();
    for p in &manifest.phases {
        phases.push_str(&format!(
            "    {{ title: {}, detail: {} }},\n",
            js_str(&p.title),
            js_str(&p.detail)
        ));
    }
    format!(
        "export const meta = {{\n  name: {},\n  description: {},\n  whenToUse: {},\n  phases: [\n{}  ],\n}}\n",
        js_str(&manifest.name),
        js_str(&manifest.description),
        js_str(&manifest.when_to_use),
        phases
    )
}

/// Render a single `agent(prompt, { opts })` call.
fn render_agent_call(step: &WorkflowManifestStep) -> String {
    let mut opts: Vec<String> = Vec::new();
    if let Some(l) = &step.label {
        opts.push(format!("label: {}", js_str(l)));
    }
    if let Some(p) = &step.phase {
        opts.push(format!("phase: {}", js_str(p)));
    }
    if let Some(m) = &step.model {
        opts.push(format!("model: {}", js_str(m)));
    }
    if let Some(sc) = &step.schema {
        let schema_json = serde_json::to_string(sc).unwrap_or_else(|_| "{}".to_string());
        opts.push(format!("schema: {schema_json}"));
    }
    if opts.is_empty() {
        format!("agent({})", js_str(&step.prompt))
    } else {
        format!("agent({}, {{ {} }})", js_str(&step.prompt), opts.join(", "))
    }
}

/// Render a Rust string as a safe double-quoted JS string literal (handles all
/// escaping via serde_json — avoids the raw-backtick / quote-escape traps).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Group step indices into dependency layers: layer 0 = no-dep steps, layer k =
/// steps whose deps are all in layers < k. Within-layer order follows manifest
/// list order. Returns `None` on cycle/unknown dep (mirrors `WorkflowDef::topo_order`).
fn topo_levels(manifest: &WorkflowManifest) -> Option<Vec<Vec<usize>>> {
    let index_of: HashMap<&str, usize> = manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let n = manifest.steps.len();
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, s) in manifest.steps.iter().enumerate() {
        for d in &s.depends_on {
            let j = *index_of.get(d.as_str())?;
            dependents[j].push(i);
            indegree[i] += 1;
        }
    }

    let mut placed = 0usize;
    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    while !current.is_empty() {
        placed += current.len();
        let mut next: Vec<usize> = Vec::new();
        for &i in &current {
            for &c in &dependents[i] {
                indegree[c] -= 1;
                if indegree[c] == 0 {
                    next.push(c);
                }
            }
        }
        next.sort_unstable(); // keep deterministic list order within a layer
        levels.push(current);
        current = next;
    }
    if placed != n {
        return None;
    }
    Some(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

    fn step(id: &str, deps: &[&str]) -> WorkflowManifestStep {
        WorkflowManifestStep {
            id: id.into(),
            agent: "ag".into(),
            prompt: format!("do {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            label: None,
            model: None,
            phase: None,
            schema: None,
        }
    }

    fn manifest(steps: Vec<WorkflowManifestStep>) -> WorkflowManifest {
        WorkflowManifest {
            name: "wf".into(),
            description: "d".into(),
            when_to_use: String::new(),
            phases: vec![],
            steps,
        }
    }

    #[test]
    fn header_and_meta_present() {
        let js = render_workflow_js(&manifest(vec![step("a", &[])]));
        assert!(js.starts_with(EMBED_PREFIX), "embedded header first");
        assert!(js.contains("export const meta = {"));
        assert!(js.contains("name: \"wf\""));
    }

    #[test]
    fn linear_chain_renders_sequential_agents() {
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &["a"])]));
        // Two single-step layers → two sequential awaits, no parallel.
        assert_eq!(js.matches("await agent(").count(), 2);
        assert!(!js.contains("parallel("));
    }

    #[test]
    fn sibling_steps_render_parallel() {
        // a, b both depend on nothing → same layer → parallel([...]).
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &[])]));
        assert!(js.contains("await parallel(["));
        assert_eq!(js.matches("() => agent(").count(), 2);
    }

    #[test]
    fn phase_marker_emitted_for_phased_step() {
        let mut s = step("a", &[]);
        s.phase = Some("Audit".into());
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("phase(\"Audit\")"));
    }

    #[test]
    fn opts_rendered_when_present() {
        let mut s = step("a", &[]);
        s.label = Some("audit:a".into());
        s.model = Some("haiku".into());
        s.schema = Some(serde_json::json!({"type": "object"}));
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("label: \"audit:a\""));
        assert!(js.contains("model: \"haiku\""));
        assert!(js.contains("schema: {\"type\":\"object\"}"));
    }

    #[test]
    fn prompt_with_quotes_is_escaped() {
        let mut s = step("a", &[]);
        s.prompt = "say \"hi\"\nnewline".into();
        let js = render_workflow_js(&manifest(vec![s]));
        // serde_json escaping keeps the source parseable — the literal contains
        // the escaped quote and \n, never a raw newline inside the call.
        assert!(js.contains("say \\\"hi\\\""));
        assert!(js.contains("\\n"));
    }
}
```

在 `src/workflow/interop/mod.rs` 加一行(置于 `pub mod manifest;` 旁):

```rust
pub mod export;
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p alephcore --lib workflow::interop::export`
Expected: 编译通过,6 个测试 PASS。

- [ ] **Step 3: 提交**

```bash
git add src/workflow/interop/export.rs src/workflow/interop/mod.rs
git commit -m "workflow: render AWI manifest to compatible .workflow.js"
```

---

## Task 3: 导入器 `parse_workflow_js`

**Files:**
- Create: `src/workflow/interop/import.rs`
- Modify: `src/workflow/interop/mod.rs`(加 `pub mod import;`)

- [ ] **Step 1: 写实现 + 失败测试**

新建 `src/workflow/interop/import.rs`:

```rust
//! Parse a `.workflow.js` (or raw AWI manifest JSON) into a `WorkflowManifest`.
//!
//! Three paths, in priority order:
//! 0. **Bare manifest JSON** (starts with `{`) → exact parse, lossless.
//! 1. **Embedded block** (`/* @aleph-workflow {json} */`) → exact parse, lossless.
//! 2. **Bare `.workflow.js`** → light-weight scan of the pure-literal `meta`
//!    block + `agent()` prompts; imperative constructs go into `dropped`.
//!
//! No JS engine, no full parser (R3). The scan's limits are surfaced via
//! `dropped`, never hidden.

use crate::error::{AlephError, Result};
use crate::workflow::interop::export::{EMBED_PREFIX, EMBED_SUFFIX};
use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

/// Result of importing a `.workflow.js`.
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub manifest: WorkflowManifest,
    /// Imperative constructs the scan could not map (empty on lossless paths).
    pub dropped: Vec<String>,
}

/// Parse `src` into a manifest. See module docs for the three paths.
pub fn parse_workflow_js(src: &str) -> Result<ImportOutcome> {
    // Path 0: bare manifest JSON document.
    let trimmed = src.trim_start();
    if trimmed.starts_with('{') {
        let manifest: WorkflowManifest = serde_json::from_str(trimmed)
            .map_err(|e| AlephError::invalid_input(format!("manifest JSON parse failed: {e}")))?;
        return Ok(ImportOutcome { manifest, dropped: Vec::new() });
    }

    // Path 1: embedded lossless block.
    if let Some(json) = extract_embedded(src) {
        let manifest: WorkflowManifest = serde_json::from_str(&json).map_err(|e| {
            AlephError::invalid_input(format!("embedded @aleph-workflow parse failed: {e}"))
        })?;
        return Ok(ImportOutcome { manifest, dropped: Vec::new() });
    }

    // Path 2: best-effort scan of a bare .workflow.js.
    scan_bare(src)
}

/// Extract the JSON between `EMBED_PREFIX` and `EMBED_SUFFIX`, if present.
fn extract_embedded(src: &str) -> Option<String> {
    let start = src.find(EMBED_PREFIX)? + EMBED_PREFIX.len();
    let rest = &src[start..];
    let end = rest.find(EMBED_SUFFIX)?;
    Some(rest[..end].trim().to_string())
}

/// Light-weight scan of a hand-written `.workflow.js`.
fn scan_bare(src: &str) -> Result<ImportOutcome> {
    let name = scan_meta_field(src, "name").ok_or_else(|| {
        AlephError::invalid_input(
            "no @aleph-workflow block and no `meta.name` found; cannot import",
        )
    })?;
    let description = scan_meta_field(src, "description").unwrap_or_default();
    let when_to_use = scan_meta_field(src, "whenToUse").unwrap_or_default();

    let mut dropped = Vec::new();
    for (needle, label) in [
        ("pipeline(", "pipeline(...) — runtime item list not statically known"),
        ("budget", "budget-driven control flow"),
        ("workflow(", "nested workflow() call"),
        ("for ", "for loop"),
        ("while ", "while loop"),
        ("if (", "if conditional"),
        ("if(", "if conditional"),
    ] {
        if src.contains(needle) {
            dropped.push(label.to_string());
        }
    }
    if src.contains("parallel(") {
        dropped.push("parallel(...) grouping approximated as a sequential chain".to_string());
    }

    let prompts = scan_agent_prompts(src);
    if prompts.is_empty() {
        return Err(AlephError::invalid_input(
            "no agent() calls found in .workflow.js; nothing to import",
        ));
    }
    let steps: Vec<WorkflowManifestStep> = prompts
        .iter()
        .enumerate()
        .map(|(i, p)| WorkflowManifestStep {
            id: format!("step_{}", i + 1),
            agent: "agent".to_string(),
            prompt: p.clone(),
            depends_on: if i == 0 { Vec::new() } else { vec![format!("step_{i}")] },
            label: None,
            model: None,
            phase: None,
            schema: None,
        })
        .collect();

    Ok(ImportOutcome {
        manifest: WorkflowManifest {
            name,
            description,
            when_to_use,
            phases: Vec::new(),
            steps,
        },
        dropped,
    })
}

/// Find `<field>:` then read the next JS string literal that follows it.
fn scan_meta_field(src: &str, field: &str) -> Option<String> {
    let key = format!("{field}:");
    let pos = src.find(&key)? + key.len();
    read_first_string_literal(&src[pos..])
}

/// Read the first single- or double-quoted string literal in `s` (UTF-8 safe,
/// honours backslash escapes by keeping the escaped char verbatim).
fn read_first_string_literal(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let quote = loop {
        match chars.next()? {
            c @ ('\'' | '"') => break c,
            _ => continue,
        }
    };
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(esc) = chars.next() {
                out.push(esc);
            }
            continue;
        }
        if c == quote {
            return Some(out);
        }
        out.push(c);
    }
    None
}

/// Collect the first string-literal argument of each `agent(` call, in order.
/// Catches both top-level `agent(` and `() => agent(` inside `parallel([...])`.
fn scan_agent_prompts(src: &str) -> Vec<String> {
    let needle = "agent(";
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        if let Some(lit) = read_first_string_literal(after) {
            out.push(lit);
        }
        rest = after;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::interop::export::render_workflow_js;
    use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

    fn sample_manifest() -> WorkflowManifest {
        WorkflowManifest {
            name: "rep".into(),
            description: "demo".into(),
            when_to_use: "use it".into(),
            phases: vec![],
            steps: vec![
                WorkflowManifestStep {
                    id: "a".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    label: Some("audit:a".into()),
                    model: None,
                    phase: None,
                    schema: None,
                },
                WorkflowManifestStep {
                    id: "b".into(),
                    agent: "writer".into(),
                    prompt: "write".into(),
                    depends_on: vec!["a".into()],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                },
            ],
        }
    }

    #[test]
    fn embedded_block_roundtrips_losslessly() {
        let original = sample_manifest();
        let js = render_workflow_js(&original);
        let outcome = parse_workflow_js(&js).expect("parse rendered js");
        assert_eq!(outcome.manifest, original, "embedded block is lossless");
        assert!(outcome.dropped.is_empty());
    }

    #[test]
    fn bare_manifest_json_parses() {
        let json = serde_json::to_string(&sample_manifest()).unwrap();
        let outcome = parse_workflow_js(&json).expect("parse bare json");
        assert_eq!(outcome.manifest, sample_manifest());
    }

    #[test]
    fn bare_js_extracts_meta_and_agents() {
        let src = r#"
export const meta = {
  name: 'hand-written',
  description: 'a manual workflow',
  whenToUse: 'when testing',
}
await agent('first step')
await agent('second step')
"#;
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(outcome.manifest.name, "hand-written");
        assert_eq!(outcome.manifest.description, "a manual workflow");
        assert_eq!(outcome.manifest.when_to_use, "when testing");
        assert_eq!(outcome.manifest.steps.len(), 2);
        assert_eq!(outcome.manifest.steps[0].prompt, "first step");
        assert_eq!(outcome.manifest.steps[1].depends_on, vec!["step_1".to_string()]);
    }

    #[test]
    fn imperative_constructs_recorded_in_dropped() {
        let src = r#"
export const meta = { name: 'loopy' }
for (const x of items) {
  await agent('do thing')
}
const r = await pipeline(items, s1, s2)
"#;
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(outcome.dropped.iter().any(|d| d.contains("for loop")));
        assert!(outcome.dropped.iter().any(|d| d.contains("pipeline")));
    }

    #[test]
    fn bare_js_without_name_errors() {
        let src = "await agent('x')";
        assert!(parse_workflow_js(src).is_err());
    }

    #[test]
    fn bare_js_without_agents_errors() {
        let src = "export const meta = { name: 'empty' }";
        assert!(parse_workflow_js(src).is_err());
    }
}
```

在 `src/workflow/interop/mod.rs` 加一行:

```rust
pub mod import;
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p alephcore --lib workflow::interop::import`
Expected: 编译通过,6 个测试 PASS。`embedded_block_roundtrips_losslessly` 验证 export↔import 闭环。

- [ ] **Step 3: 提交**

```bash
git add src/workflow/interop/import.rs src/workflow/interop/mod.rs
git commit -m "workflow: parse .workflow.js into AWI manifest (lossless + best-effort scan)"
```

---

## Task 4: 补全 interop / workflow 模块重导出

**Files:**
- Modify: `src/workflow/interop/mod.rs`
- Modify: `src/workflow/mod.rs:19-25`

- [ ] **Step 1: 写最终 `mod.rs`**

把 `src/workflow/interop/mod.rs` 整文件替换为:

```rust
//! `.workflow.js` interoperability — bidirectional bridge between Aleph's
//! declarative `WorkflowDef` and Claude Code's workflow engineering format.
//!
//! Pure data layer (R7/R10): a declarative `WorkflowManifest` superset is the
//! single source of truth; only the executable core maps into `WorkflowDef`.

pub mod export;
pub mod import;
pub mod manifest;

pub use export::render_workflow_js;
pub use import::{parse_workflow_js, ImportOutcome};
pub use manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};
```

在 `src/workflow/mod.rs`,确认 `pub mod interop;` 在模块声明区(`pub mod compile;` 等旁),并在重导出区追加一行(置于 `pub use store::...` 之后):

```rust
pub use interop::{parse_workflow_js, render_workflow_js, ImportOutcome, WorkflowManifest};
```

- [ ] **Step 2: 运行测试,确认整层编译 + 全绿**

Run: `cargo test -p alephcore --lib workflow::interop`
Expected: 编译通过,17 个测试全 PASS(manifest 5 + export 6 + import 6)。

- [ ] **Step 3: 提交**

```bash
git add src/workflow/interop/mod.rs src/workflow/mod.rs
git commit -m "workflow: re-export interop bridge API"
```

---

## Task 5: `workflow` 工具新增 `export` / `import` 动作

**Files:**
- Modify: `src/builtin_tools/workflow_tool.rs`

- [ ] **Step 1: 加动作枚举变体**

在 `WorkflowArgs` enum(`workflow_tool.rs:26-47`)末尾、`Run {...}` 之后追加:

```rust
    /// Render a saved template into a Claude-Code-compatible `.workflow.js`.
    Export {
        /// Name of the saved template to render.
        name: String,
        /// Also write it to `$ALEPH_HOME/workflows/<name>.workflow.js`.
        #[serde(default)]
        write_file: bool,
    },
    /// Parse a `.workflow.js` (or AWI manifest JSON) into a WorkflowDef.
    Import {
        /// Raw `.workflow.js` text or AWI manifest JSON.
        source: String,
        /// Also persist the parsed template via the store.
        #[serde(default)]
        save: bool,
    },
```

- [ ] **Step 2: 加输出字段**

在 `WorkflowToolOutput`(`workflow_tool.rs:49-62`)的 `task_ids` 字段后追加:

```rust
    /// Populated by `export` — the rendered `.workflow.js` text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    /// Populated by `import` — imperative constructs that could not be mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<Vec<String>>,
```

并在 `WorkflowToolOutput::msg`(`workflow_tool.rs:65-73`)的结构体字面量里补两个 `None`:

```rust
    fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            names: None,
            definition: None,
            task_ids: None,
            rendered: None,
            dropped: None,
        }
    }
```

注意:文件内其它构造 `WorkflowToolOutput { ... }` 字面量的地方(List/Describe/Run 三处)也要补 `rendered: None, dropped: None,`,否则 E0063 缺字段。逐处加上。

- [ ] **Step 3: 加 import 用到的引用**

在文件顶部 `use` 区(`workflow_tool.rs:22` 附近)把:

```rust
use crate::workflow::{self, WorkflowDef};
```

扩展为:

```rust
use crate::workflow::{self, WorkflowDef, WorkflowManifest};
```

- [ ] **Step 4: 加 `call` 分支**

在 `call`(`workflow_tool.rs:121-185`)的 `match args` 中,`WorkflowArgs::Run {...} => {...}` 分支之后追加两个分支:

```rust
            WorkflowArgs::Export { name, write_file } => {
                debug!(name = %name, write_file, "workflow: export");
                let def = workflow::store::load(&name)?;
                let manifest = WorkflowManifest::from_def(&def);
                let rendered = workflow::render_workflow_js(&manifest);
                let message = if write_file {
                    let dir = workflow::store::workflow_dir();
                    let path = dir.join(format!("{}.workflow.js", crate::canvas_io::sanitise_name(&name)));
                    std::fs::create_dir_all(&dir).map_err(|e| {
                        crate::error::AlephError::config(format!(
                            "create workflows dir {} failed: {e}",
                            dir.display()
                        ))
                    })?;
                    let tmp = path.with_extension("js.tmp");
                    std::fs::write(&tmp, &rendered).map_err(|e| {
                        crate::error::AlephError::config(format!(
                            "write {} failed: {e}",
                            tmp.display()
                        ))
                    })?;
                    std::fs::rename(&tmp, &path).map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        crate::error::AlephError::config(format!(
                            "rename {} → {} failed: {e}",
                            tmp.display(),
                            path.display()
                        ))
                    })?;
                    format!("exported workflow '{name}' → {}", path.display())
                } else {
                    format!("rendered workflow '{name}' ({} bytes)", rendered.len())
                };
                Ok(WorkflowToolOutput {
                    action: "export".into(),
                    message,
                    names: None,
                    definition: None,
                    task_ids: None,
                    rendered: Some(rendered),
                    dropped: None,
                })
            }
            WorkflowArgs::Import { source, save } => {
                debug!(save, "workflow: import");
                let outcome = workflow::parse_workflow_js(&source)?;
                let def = outcome.manifest.to_def();
                def.validate()?;
                let message = if save {
                    let path = workflow::store::save(&def)?;
                    format!(
                        "imported workflow '{}' ({} step(s)) → {}",
                        def.name,
                        def.steps.len(),
                        path.display()
                    )
                } else {
                    format!(
                        "parsed workflow '{}' ({} step(s); not saved)",
                        def.name,
                        def.steps.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    action: "import".into(),
                    message,
                    names: None,
                    definition: Some(def),
                    task_ids: None,
                    rendered: None,
                    dropped: Some(outcome.dropped),
                })
            }
```

> 说明:`outcome.manifest.to_def()` 需 `WorkflowManifest::to_def`(Task 1)。`def.validate()` 已是 `WorkflowDef` 的方法(`def.rs:60`)。`workflow::store::workflow_dir` 为 `pub`(`store.rs:20`)。

- [ ] **Step 5: 更新 `examples` 与 `DESCRIPTION`**

在 `examples()`(`workflow_tool.rs:111-119`)返回的 `vec![...]` 末尾(`delete` 例子之后)追加两条:

```rust
            "workflow(action='export', name='research-report')".into(),
            r#"workflow(action='import', source='export const meta = { name: \"x\" }\nawait agent(\"do it\")', save=true)"#.into(),
```

把 `DESCRIPTION`(`workflow_tool.rs:100-106`)最后的 `Actions: save / list / describe / delete / run.` 改为:

```rust
         Actions: save / list / describe / delete / run / export / import. \
         `export` renders a template to a Claude-Code-compatible .workflow.js; \
         `import` parses one back into a template. For `run`, create a \
         team first so each step's agent resolves to a member.";
```

(删去原句末尾重复的 "For `run`..." 一句,避免重复。)

- [ ] **Step 6: 加测试**

在 `workflow_tool.rs` 的 `#[cfg(test)] mod tests` 内、`run_*` 测试附近追加。复用现有 `ENV_GUARD` / `setup_store` / `linear_def` / `tool` helper:

```rust
    #[test]
    fn deserialize_export_defaults_write_file_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"export","name":"p"}))
                .expect("deserialise export");
        match args {
            WorkflowArgs::Export { name, write_file } => {
                assert_eq!(name, "p");
                assert!(!write_file, "write_file defaults to false");
            }
            other => panic!("expected Export, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_import_defaults_save_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"import","source":"x"}))
                .expect("deserialise import");
        match args {
            WorkflowArgs::Import { source, save } => {
                assert_eq!(source, "x");
                assert!(!save, "save defaults to false");
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn export_renders_without_writing_then_import_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }

        workflow::store::save(&linear_def()).expect("save template");

        // export (no write_file) populates `rendered`, not task_ids/definition.
        let exported = t
            .call(WorkflowArgs::Export {
                name: "pipeline".into(),
                write_file: false,
            })
            .await
            .expect("export");
        assert_eq!(exported.action, "export");
        let js = exported.rendered.as_ref().expect("export populates rendered");
        assert!(js.contains("export const meta = {"));
        assert!(exported.task_ids.is_none() && exported.definition.is_none());

        // import the rendered text back (no save) → definition equals the core,
        // dropped is empty for the lossless embedded path.
        let imported = t
            .call(WorkflowArgs::Import {
                source: js.clone(),
                save: false,
            })
            .await
            .expect("import");
        assert_eq!(imported.action, "import");
        let def = imported.definition.as_ref().expect("import populates definition");
        assert_eq!(def, &linear_def());
        assert_eq!(imported.dropped.as_deref(), Some(&[][..]));

        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
    }

    #[tokio::test]
    async fn import_with_save_persists_template() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }

        let source = "export const meta = { name: 'scanned' }\nawait agent('do the thing')";
        let imported = t
            .call(WorkflowArgs::Import {
                source: source.into(),
                save: true,
            })
            .await
            .expect("import + save");
        assert!(imported.message.contains("imported"));

        let listed = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(listed.names.as_deref(), Some(&["scanned".to_string()][..]));

        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
    }

    #[tokio::test]
    async fn export_missing_template_errors() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        let res = t
            .call(WorkflowArgs::Export {
                name: "ghost".into(),
                write_file: false,
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        assert!(res.is_err(), "exporting a missing template errors");
    }
```

- [ ] **Step 7: 运行测试**

Run: `cargo test -p alephcore --lib workflow_tool`
Expected: 编译通过,原有 10 个 + 新增 5 个测试全 PASS(共 15)。

- [ ] **Step 8: 全子系统验证**

Run: `cargo check -p alephcore`
Expected: clean(无 error,无新 warning)。

Run: `cargo test -p alephcore --lib workflow`
Expected: 全 PASS(`workflow::*` + `workflow::interop::*` + `builtin_tools::workflow_tool::*`)。

- [ ] **Step 9: 提交**

```bash
git add src/builtin_tools/workflow_tool.rs
git commit -m "workflow: add export/import actions to the workflow tool"
```

---

## Task 6: 规范文档

**Files:**
- Create: `docs/reference/WORKFLOW_INTEROP.md`
- Modify: `CLAUDE.md`(文档索引表)

- [ ] **Step 1: 写规范文档**

新建 `docs/reference/WORKFLOW_INTEROP.md`:

````markdown
# Workflow Interop — `.workflow.js` 双向互换

Aleph 的 `workflow` 子系统(声明式静态 DAG)与 Claude Code `.workflow.js` 工程文件
(命令式编排脚本)之间的兼容桥。实现见 `src/workflow/interop/`。

## 设计原则

- **声明式 manifest (AWI) 是唯一真相**。`.workflow.js` 是它的渲染视图。
- **执行核心零改动**:`WorkflowDef` / `compile` / dispatcher 不变;额外元数据不入执行
  schema(避免死配置,R10)。
- **无 JS 引擎**(R3):导入裸 `.workflow.js` 用手写轻量扫描;无法映射的命令式构造经
  `dropped[]` 透明上报,绝不静默吞掉。
- **命令式控制流故意不支持**(R7/R10):routing / 循环 / 条件 / evaluator-optimizer
  属 Think→Act 主循环的职责,不外接到这一声明式层。

## AWI manifest schema

JSON,camelCase 键(贴合 `.workflow.js` 的 `meta`)。

```jsonc
{
  "name": "research-report",
  "description": "...",
  "whenToUse": "...",
  "phases": [{ "title": "Gather", "detail": "..." }],
  "steps": [
    {
      "id": "gather",
      "agent": "researcher",
      "prompt": "research {input}",
      "dependsOn": [],
      "label": "audit:gather",
      "model": "haiku",
      "phase": "Gather",
      "schema": { "type": "object" }
    }
  ]
}
```

- 必填:顶层 `name` / `steps`;步骤 `id` / `agent` / `prompt`。
- 可选:`description` / `whenToUse` / `phases`;步骤 `dependsOn`(缺省 `[]`)/ `label` /
  `model` / `phase` / `schema`(原样透传,Aleph 不解释)。
- 只有 `name` / `description` / `steps{id,agent,prompt,dependsOn}` 映射进 `WorkflowDef`;
  其余字段仅在导出的 `.workflow.js` 头部内嵌块中保留。

## `.workflow.js` ↔ DAG 映射

| Claude Code 构造 | 方向 | 说明 |
|---|---|---|
| `meta.{name,description,whenToUse,phases}` | ↔ 无损 | manifest 顶层 |
| 顺序 `await agent()` 链 | ↔ | 线性 `dependsOn` 链 |
| `parallel([agent, agent])` | ↔ | 同拓扑层、彼此无 `dependsOn` 的兄弟步骤 |
| `agent()` fan-in | ↔ | 一步 `dependsOn` 多个上游 |
| `opts.{label,model,phase,schema}` | ↔ 无损(经内嵌块) | 存 manifest,不入 `WorkflowDef` |
| `pipeline(items, s1, s2)` | → 导入近似 | 运行时 item 列表未知 → 记 `dropped`;导出不生成 |
| 循环 / 条件 / `budget` / 嵌套 `workflow()` | ✗ 故意不支持 | 导入记 `dropped`(R7/R10) |

## 无损往返机制

导出在文件首行写入:

```js
/* @aleph-workflow {<完整 manifest 的单行 JSON>} */
```

导入优先读此块 → 精确还原(`dropped` 为空)。无此块的裸文件则走轻量扫描:提取
`meta.{name,description,whenToUse}` + 各 `agent()` 的首个字符串字面量为步骤(线性链),
并把识别到的命令式构造写入 `dropped`。

## 工具用法(R8)

```
workflow(action='export', name='research-report')                 # 渲染为 .workflow.js 文本
workflow(action='export', name='research-report', write_file=true) # 同时写盘
workflow(action='import', source='<.workflow.js 或 manifest JSON>', save=true)
```

- `export` 输出填 `rendered`(渲染文本);`write_file=true` 时写
  `$ALEPH_HOME/workflows/<name>.workflow.js`。
- `import` 输出填 `definition`(解析出的 `WorkflowDef`)+ `dropped`(被丢弃的命令式构造);
  `save=true` 时入库。

## 明确不做(YAGNI / 后续 PR)

- gateway RPC `workflow.export/import`(R4/R6 I/O 表面)。
- CLI `aleph workflow export/import`。
- Panel UI。
- 完整 JS-子集解析器(R3 拒绝;裸文件覆盖面以 `dropped[]` 透明界定)。
````

- [ ] **Step 2: 挂进 CLAUDE.md 文档索引**

在 `CLAUDE.md` 的「📚 文档索引」表中,`PLUGIN_SYSTEM.md` 行之后插入一行:

```markdown
| WORKFLOW_INTEROP.md | [docs/reference/WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) |
```

- [ ] **Step 3: 提交**

```bash
git add docs/reference/WORKFLOW_INTEROP.md CLAUDE.md
git commit -m "docs: workflow interop format spec + index entry"
```

---

## 完成判据

- `cargo check -p alephcore` clean。
- `cargo test -p alephcore --lib workflow` 全 PASS(新增约 22 个 interop/tool 测试 + 原有 45 个 workflow 测试)。
- `src/workflow/def.rs` / `compile.rs` / `store.rs` 核心逻辑、`constructor.rs` / `registry.rs` / `definitions.rs` / `groups.rs` **未改动**(工具已注册,新动作走同一 enum)。
- 往返自验:`export` 出的 `.workflow.js` 经 `import` 还原后 `definition` 等于原 `WorkflowDef`。
- **绝不** `cargo fmt`。
