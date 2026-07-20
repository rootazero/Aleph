# File Ops Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the `file_ops` universal tool into 3 independent tools (`file_read`, `file_write`, `file_edit`) with explicit required fields, fixing the LLM `content: null` bug. Keep `file_ops` for remaining operations (list/move/copy/delete/mkdir/search).

**Architecture:** Create 3 new AlephTool implementations in `src/builtin_tools/file_ops/`. Each has its own Args struct with required fields (no `Option` for mandatory params). Register all 3 alongside the existing (simplified) `file_ops`. Remove `Read`/`Write` variants from the `FileOperation` enum.

**Tech Stack:** Rust, schemars (JsonSchema), async-trait, serde

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/builtin_tools/file_ops/read.rs` | Create | FileReadTool — AlephTool impl |
| `src/builtin_tools/file_ops/write.rs` | Create | FileWriteTool — AlephTool impl |
| `src/builtin_tools/file_ops/edit.rs` | Create | FileEditTool — AlephTool impl |
| `src/builtin_tools/file_ops/types.rs` | Modify | Remove Read/Write from FileOperation enum |
| `src/builtin_tools/file_ops/tool.rs` | Modify | Remove Read/Write match branches |
| `src/builtin_tools/file_ops/mod.rs` | Modify | Add `pub mod read/write/edit`, re-exports |
| `src/executor/builtin_registry/builder.rs` | Modify | Register 3 new tools |
| `src/executor/builtin_registry/registry.rs` | Modify | Add fields for new tools |

---

### Task 1: Create FileReadTool

Create an independent file read tool with its own Args struct.

**Files:**
- Create: `src/builtin_tools/file_ops/read.rs`

**Context to read first:**
- `src/builtin_tools/file_ops/ops.rs` — find `execute_read()` function signature (line 79). Understand its parameters: `(path, denied_paths, max_read_size, output_dir_override)`
- `src/builtin_tools/file_ops/tool.rs` — see how FileOpsTool uses execute_read (line 133-134)
- `src/tools/traits.rs` — AlephTool trait definition (NAME, DESCRIPTION, Args, Output, call)
- `src/builtin_tools/file_ops/path_utils.rs` — `check_and_resolve_path` and `get_denied_paths`

- [ ] **Step 1: Create `src/builtin_tools/file_ops/read.rs`**

Implement `FileReadTool` as an independent `AlephTool`:

```rust
//! FileReadTool — independent file read tool with explicit schema.

use std::path::Path;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ops::execute_read;
use super::path_utils::get_denied_paths;
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for file_read tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileReadArgs {
    /// The absolute path to the file to read
    pub path: String,
    /// Line offset to start reading from (optional)
    #[serde(default)]
    pub offset: Option<u64>,
    /// Maximum number of lines to read (optional)
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Output from file_read tool
#[derive(Debug, Clone, Serialize)]
pub struct FileReadOutput {
    pub success: bool,
    pub path: String,
    pub content: String,
    pub size: u64,
    pub message: String,
}

/// Read file contents
pub struct FileReadTool {
    max_read_size: u64,
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileReadTool {
    pub fn new() -> Self {
        Self {
            max_read_size: 100 * 1024 * 1024,
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }
}

impl Clone for FileReadTool {
    fn clone(&self) -> Self {
        Self {
            max_read_size: self.max_read_size,
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for FileReadTool {
    const NAME: &'static str = "file_read";
    const DESCRIPTION: &'static str = "Read the contents of a file. Returns the text content, file size, and line count. Use offset and limit for partial reads of large files.";

    type Args = FileReadArgs;
    type Output = FileReadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(path = %args.path, "file_read invoked");
        let path = Path::new(&args.path);
        let output_dir = self.resolve_output_dir().await;
        // Delegate to existing execute_read
        let result = execute_read(path, &self.denied_paths, self.max_read_size, output_dir.as_deref()).await?;
        Ok(FileReadOutput {
            success: result.success,
            path: args.path,
            content: result.content.unwrap_or_default(),
            size: result.bytes_written.unwrap_or(0),
            message: result.message,
        })
    }
}
```

Note: The `execute_read` function returns `FileOpsOutput`. Extract the relevant fields. If `offset`/`limit` aren't supported by `execute_read` yet, add them later — for now just pass through to the existing function.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```
feat(tools): add independent FileReadTool with explicit schema
```

---

### Task 2: Create FileWriteTool

Create an independent file write tool where `content` is a **required `String`** (not `Option<String>`).

**Files:**
- Create: `src/builtin_tools/file_ops/write.rs`

**Context to read first:**
- `src/builtin_tools/file_ops/ops.rs` — find `execute_write()` function signature (line 129). Understand its parameters: `(path, content, create_parents, denied_paths, output_dir_override)`
- `src/builtin_tools/file_ops/tool.rs` — see how FileOpsTool uses execute_write (lines 136-153)

- [ ] **Step 1: Create `src/builtin_tools/file_ops/write.rs`**

```rust
//! FileWriteTool — independent file write tool with content as REQUIRED field.

use std::path::Path;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ops::execute_write;
use super::path_utils::get_denied_paths;
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for file_write tool.
/// Both file_path and content are REQUIRED (non-Optional String).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileWriteArgs {
    /// The absolute path to the file to write
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
    /// Create parent directories if they don't exist (default: true)
    #[serde(default = "default_true")]
    pub create_parents: bool,
}

fn default_true() -> bool { true }

/// Output from file_write tool
#[derive(Debug, Clone, Serialize)]
pub struct FileWriteOutput {
    pub success: bool,
    pub path: String,
    pub bytes_written: u64,
    pub message: String,
}

/// Write content to a file
pub struct FileWriteTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileWriteTool {
    pub fn new() -> Self {
        Self {
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }
}

impl Clone for FileWriteTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for FileWriteTool {
    const NAME: &'static str = "file_write";
    const DESCRIPTION: &'static str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Both file_path and content are required parameters.";

    type Args = FileWriteArgs;
    type Output = FileWriteOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(path = %args.file_path, bytes = args.content.len(), "file_write invoked");
        let path = Path::new(&args.file_path);
        let output_dir = self.resolve_output_dir().await;
        let result = execute_write(path, &args.content, args.create_parents, &self.denied_paths, output_dir.as_deref()).await?;
        Ok(FileWriteOutput {
            success: result.success,
            path: args.file_path,
            bytes_written: result.bytes_written.unwrap_or(0),
            message: result.message,
        })
    }
}
```

**Key**: `content: String` — NOT `Option<String>`. JSON Schema will mark it as `required`. LLM cannot send null.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```
feat(tools): add independent FileWriteTool with required content field

content is String (not Option<String>) so JSON Schema marks it as
required. This fixes the LLM "content: null" bug.
```

---

### Task 3: Create FileEditTool

New tool for string replacement editing, aligned with claude-code's FileEditTool.

**Files:**
- Create: `src/builtin_tools/file_ops/edit.rs`

**Context to read first:**
- `/Volumes/TBU4/Github/claude-code/src/tools/FileEditTool/FileEditTool.ts` — reference implementation. Key behaviors: find old_string in file, replace with new_string, error if multiple matches without replace_all=true, error if old_string not found.
- `src/builtin_tools/file_ops/path_utils.rs` — `check_and_resolve_path`

- [ ] **Step 1: Create `src/builtin_tools/file_ops/edit.rs`**

```rust
//! FileEditTool — string replacement editing, aligned with claude-code.

use std::path::Path;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::path_utils::{check_and_resolve_path, get_denied_paths};
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for file_edit tool.
/// file_path, old_string, and new_string are all REQUIRED.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileEditArgs {
    /// The absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find in the file
    pub old_string: String,
    /// The string to replace it with (must be different from old_string)
    pub new_string: String,
    /// Replace all occurrences (default: false — fails if multiple matches)
    #[serde(default)]
    pub replace_all: bool,
}

/// Output from file_edit tool
#[derive(Debug, Clone, Serialize)]
pub struct FileEditOutput {
    pub success: bool,
    pub path: String,
    pub replacements: usize,
    pub message: String,
}

/// Edit a file by replacing exact string matches
pub struct FileEditTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileEditTool {
    pub fn new() -> Self {
        Self {
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }
}

impl Clone for FileEditTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for FileEditTool {
    const NAME: &'static str = "file_edit";
    const DESCRIPTION: &'static str = r#"Edit a file by replacing an exact string match. Provide file_path, old_string (the text to find), and new_string (the replacement). If old_string appears multiple times and replace_all is false (default), the edit will fail — either provide more context in old_string to make it unique, or set replace_all to true."#;

    type Args = FileEditArgs;
    type Output = FileEditOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(path = %args.file_path, replace_all = args.replace_all, "file_edit invoked");

        if args.old_string == args.new_string {
            return Err(ToolError::InvalidArgs(
                "old_string and new_string must be different".to_string()
            ).into());
        }

        let path = Path::new(&args.file_path);
        let output_dir = self.resolve_output_dir().await;
        let canonical = check_and_resolve_path(path, &self.denied_paths, output_dir.as_deref())?;

        // Read file
        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to read {}: {}", args.file_path, e)))?;

        // Count matches
        let match_count = content.matches(&args.old_string).count();

        if match_count == 0 {
            return Err(ToolError::Execution(format!(
                "old_string not found in {}. Make sure the string matches exactly (including whitespace and newlines).",
                args.file_path
            )).into());
        }

        if match_count > 1 && !args.replace_all {
            return Err(ToolError::Execution(format!(
                "Found {} matches of old_string in {}. Either provide more context to make the match unique, or set replace_all to true.",
                match_count, args.file_path
            )).into());
        }

        // Replace
        let new_content = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };

        // Write back
        std::fs::write(&canonical, &new_content)
            .map_err(|e| ToolError::Execution(format!("Failed to write {}: {}", args.file_path, e)))?;

        let replacements = if args.replace_all { match_count } else { 1 };

        Ok(FileEditOutput {
            success: true,
            path: args.file_path,
            replacements,
            message: format!("Replaced {} occurrence(s)", replacements),
        })
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```
feat(tools): add FileEditTool for string replacement editing

Aligned with claude-code's FileEditTool pattern. All parameters
(file_path, old_string, new_string) are required. Supports
replace_all flag and validates unique matches.
```

---

### Task 4: Wire New Tools and Simplify file_ops

Register the 3 new tools, remove Read/Write from file_ops, update mod.rs.

**Files:**
- Modify: `src/builtin_tools/file_ops/mod.rs` — add `pub mod read; pub mod write; pub mod edit;` and re-exports
- Modify: `src/builtin_tools/file_ops/types.rs` — remove `Read` and `Write` from `FileOperation` enum
- Modify: `src/builtin_tools/file_ops/tool.rs` — remove Read/Write match branches and update DESCRIPTION
- Modify: `src/executor/builtin_registry/builder.rs` — register FileReadTool, FileWriteTool, FileEditTool
- Modify: `src/executor/builtin_registry/registry.rs` — add fields for new tools

**Context to read first:**
- `src/executor/builtin_registry/builder.rs` — how FileOpsTool is created and registered (lines 48-55, 705-710)
- `src/executor/builtin_registry/registry.rs` — struct fields for tool instances
- `src/builtin_tools/mod.rs` — re-exports

- [ ] **Step 1: Update `src/builtin_tools/file_ops/mod.rs`**

Add modules and re-exports:
```rust
pub mod edit;
pub mod read;
pub mod write;

pub use edit::FileEditTool;
pub use read::FileReadTool;
pub use write::FileWriteTool;
```

- [ ] **Step 2: Update `src/builtin_tools/file_ops/types.rs`**

Remove `Read` and `Write` from `FileOperation` enum. Remove the `content` field from `FileOpsArgs` (no longer needed). Remove the `deserialize_content` function. Update the enum to only have:
```rust
pub enum FileOperation {
    List,
    Move,
    Copy,
    Delete,
    Mkdir,
    Search,
    BatchMove,
    Organize,
}
```

Remove from `FileOpsArgs`:
- The `content` field
- The `deserialize_content` function

- [ ] **Step 3: Update `src/builtin_tools/file_ops/tool.rs`**

Remove the `Read` and `Write` match branches from `call_impl()`. Update `DESCRIPTION` to remove read/write mentions. Remove the `content` null warning code. Remove `Read` and `Write` from the `op_name` match.

- [ ] **Step 4: Update `src/builtin_tools/mod.rs`**

Add re-exports for new tools:
```rust
pub use file_ops::{FileEditTool, FileReadTool, FileWriteTool};
```

- [ ] **Step 5: Register new tools in `src/executor/builtin_registry/builder.rs`**

Find where `FileOpsTool::new()` is created (around line 51-55). Add similar construction for the 3 new tools:

```rust
let file_read_tool = if let Some(ref tc) = config.tool_context {
    FileReadTool::new().with_tool_context(std::sync::Arc::clone(tc))
} else {
    FileReadTool::new()
};
let file_write_tool = if let Some(ref tc) = config.tool_context {
    FileWriteTool::new().with_tool_context(std::sync::Arc::clone(tc))
} else {
    FileWriteTool::new()
};
let file_edit_tool = if let Some(ref tc) = config.tool_context {
    FileEditTool::new().with_tool_context(std::sync::Arc::clone(tc))
} else {
    FileEditTool::new()
};
```

Add fields to the `BuiltinToolRegistry` struct and register them in the tool dispatch.

- [ ] **Step 6: Update `src/executor/builtin_registry/registry.rs`**

Add fields for the 3 new tools. Follow the same pattern as the existing `file_ops_tool` field.

- [ ] **Step 7: Update tests in `src/builtin_tools/file_ops/mod.rs`**

The existing `test_read_write_file` test uses `FileOperation::Read` and `FileOperation::Write` which no longer exist. Update or split this test:
- Test read via `FileReadTool` directly
- Test write via `FileWriteTool` directly
- Keep remaining tests (list, mkdir, move, search) unchanged

- [ ] **Step 8: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib -- file_ops`
Fix any compilation errors.

- [ ] **Step 9: Commit**

```
refactor(tools): wire FileReadTool/FileWriteTool/FileEditTool, simplify file_ops

Registered 3 independent tools with explicit required schemas.
Removed Read/Write from file_ops enum — file_ops now only handles
list/move/copy/delete/mkdir/search/batch_move/organize.
```

---

### Task 5: Build, Restart, Test

Full build, restart Aleph, test file_write with the LLM to confirm content=null is fixed.

- [ ] **Step 1: Full release build**

```bash
just build
```

- [ ] **Step 2: Restart server**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
nohup target/release/aleph-server start > /tmp/aleph-server.log 2>&1 &
```

- [ ] **Step 3: Test via Panel**

Open Panel, start a new session (`/new`), send:
"请用file_write工具把以下内容写入MEMORY.md：我叫张三"

Verify in logs that:
- `file_write` tool is called (not `file_ops`)
- `content` field is present and non-null
- Write succeeds

- [ ] **Step 4: Test file_edit**

Send: "请用file_edit工具把MEMORY.md中的张三替换为李四"

Verify edit succeeds.

- [ ] **Step 5: Test file_read**

Send: "请读取MEMORY.md的内容"

Verify file_read tool is called and returns content.

- [ ] **Step 6: Final commit if fixes needed**
