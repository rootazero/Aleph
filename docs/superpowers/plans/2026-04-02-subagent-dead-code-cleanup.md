# Subagent Dead Code Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove dead code (SubAgentHandler) and empty placeholder (EscalateTaskTool) from the subagent system.

**Architecture:** Two independent deletions — one in `components/` (event handler never instantiated), one in `builtin_tools/` + `executor/builtin_registry/` (tool that accepts but never executes). No new code, pure cleanup.

**Tech Stack:** Rust, cargo

**Spec:** `docs/superpowers/specs/2026-04-02-subagent-dead-code-cleanup-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/components/subagent_handler.rs` | Delete | Dead event handler (~472 lines) |
| `src/components/mod.rs` | Modify | Remove module declaration + re-export |
| `src/builtin_tools/escalate_task.rs` | Delete | Empty placeholder tool (~103 lines) |
| `src/builtin_tools/mod.rs` | Modify | Remove module declaration + re-export |
| `src/executor/builtin_registry/definitions.rs` | Modify | Remove import, definition entry, match arm |
| `src/executor/builtin_registry/groups.rs` | Modify | Remove from "spawn" tool category |

---

### Task 1: Delete SubAgentHandler

**Files:**
- Delete: `src/components/subagent_handler.rs`
- Modify: `src/components/mod.rs`

- [ ] **Step 1: Remove module declaration from components/mod.rs**

In `src/components/mod.rs`, delete the line:

```rust
mod subagent_handler;
```

And delete the re-export line:

```rust
pub use subagent_handler::SubAgentHandler;
```

- [ ] **Step 2: Update module doc comment**

In `src/components/mod.rs`, update the doc comment to remove SubAgentHandler from the list. Change:

```rust
//! - `SubAgentHandler`: Sub-agent lifecycle management (Phase 4)
```

Delete this line entirely.

- [ ] **Step 3: Delete the file**

Delete `src/components/subagent_handler.rs`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: no errors (SubAgentHandler had zero consumers outside its own tests)

- [ ] **Step 5: Run component tests**

Run: `cargo test -p alephcore --lib -- components`
Expected: all remaining component tests pass

- [ ] **Step 6: Commit**

```bash
git add -A src/components/
git commit -m "refactor: remove dead SubAgentHandler (zero consumers outside tests)"
```

---

### Task 2: Delete EscalateTaskTool

**Files:**
- Delete: `src/builtin_tools/escalate_task.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/groups.rs`

- [ ] **Step 1: Remove module declaration from builtin_tools/mod.rs**

In `src/builtin_tools/mod.rs`, delete the module declaration (line 49):

```rust
pub mod escalate_task;
```

And delete the re-export (line 111):

```rust
pub use escalate_task::{EscalateTaskArgs, EscalateTaskOutput, EscalateTaskTool};
```

- [ ] **Step 2: Remove import from definitions.rs**

In `src/executor/builtin_registry/definitions.rs`, remove `EscalateTaskTool` from the import block (line 29). Change:

```rust
use crate::builtin_tools::{
    BashExecTool, CodeExecTool, DesktopTool, EscalateTaskTool, FileOpsTool, ImageGenerateTool,
    PdfGenerateTool, ReadConfigGuideTool, SearchTool, SelfManageTool, VaultStoreTool, WebFetchTool,
};
```

To:

```rust
use crate::builtin_tools::{
    BashExecTool, CodeExecTool, DesktopTool, FileOpsTool, ImageGenerateTool,
    PdfGenerateTool, ReadConfigGuideTool, SearchTool, SelfManageTool, VaultStoreTool, WebFetchTool,
};
```

- [ ] **Step 3: Remove BuiltinToolDefinition entry from definitions.rs**

In `src/executor/builtin_registry/definitions.rs`, delete the definition entry (lines 139-143):

```rust
    BuiltinToolDefinition {
        name: "escalate_task",
        description: "Request escalation to a more capable execution strategy",
        requires_config: false,
    },
```

- [ ] **Step 4: Remove match arm from create_tool in definitions.rs**

In `src/executor/builtin_registry/definitions.rs`, delete the match arm (line 507):

```rust
        "escalate_task" => Some(Box::new(EscalateTaskTool)),
```

- [ ] **Step 5: Remove from tool category in groups.rs**

In `src/executor/builtin_registry/groups.rs`, remove `"escalate_task"` from the "spawn" category (line 95). Change:

```rust
    ToolCategory {
        id: "spawn",
        name: "子 Agent 派发",
        tools: &[
            "subagent_spawn",
            "subagent_steer",
            "subagent_kill",
            "escalate_task",
        ],
    },
```

To:

```rust
    ToolCategory {
        id: "spawn",
        name: "子 Agent 派发",
        tools: &[
            "subagent_spawn",
            "subagent_steer",
            "subagent_kill",
        ],
    },
```

- [ ] **Step 6: Delete the file**

Delete `src/builtin_tools/escalate_task.rs`.

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 8: Run related tests**

Run: `cargo test -p alephcore --lib -- builtin_tools`
Expected: all remaining builtin_tools tests pass

Run: `cargo test -p alephcore --lib -- executor::builtin_registry`
Expected: all registry tests pass

- [ ] **Step 9: Commit**

```bash
git add -A src/builtin_tools/ src/executor/builtin_registry/
git commit -m "refactor: remove empty EscalateTaskTool placeholder (accept-only, no actual routing)"
```

---

### Task 3: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Check for residual dead code**

Run: `cargo check -p alephcore 2>&1 | grep -i "unused\|dead_code"`
Expected: no matches related to our changes
