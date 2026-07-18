# File Ops Split Design — Independent File Tools

## Problem

Aleph's `file_ops` is a "universal tool" with 10 operations sharing one parameter struct where all fields are `Option<T>`. When LLM calls Write, it sends `"content": null` because the JSON Schema marks content as optional. This causes repeated failures (8+ retries per conversation). Schema description improvements and error message fixes do not resolve the issue — the LLM consistently sends null.

## Solution

Split file_ops into independent tools with explicit required fields, following claude-code's pattern (FileReadTool, FileWriteTool, FileEditTool). Keep file_ops for remaining operations.

## Architecture

```
Before:
  file_ops (1 tool, 10 operations, all params optional)

After:
  file_read  — read file content (path required)
  file_write — write/create file (path + content required)
  file_edit  — string replacement edit (file_path + old_string + new_string required)
  file_ops   — retained for: list, move, copy, delete, mkdir, search, batch_move, organize
```

## New Tool Definitions

### file_read

```rust
pub struct FileReadArgs {
    /// Absolute path to the file to read
    pub path: String,
    /// Line offset to start reading from (0-based)
    #[serde(default)]
    pub offset: Option<u64>,
    /// Maximum number of lines to read
    #[serde(default)]
    pub limit: Option<u64>,
}

pub struct FileReadOutput {
    pub content: String,
    pub path: String,
    pub size: u64,
    pub lines: usize,
}
```

Inherits from current `execute_read()` implementation. Denied paths and max_read_size checks preserved.

### file_write

```rust
pub struct FileWriteArgs {
    /// Absolute path to the file to write
    pub file_path: String,
    /// The full content to write to the file
    pub content: String,           // NOT Option — required in JSON Schema
    /// Create parent directories if they don't exist
    #[serde(default = "default_true")]
    pub create_parents: bool,
}

pub struct FileWriteOutput {
    pub success: bool,
    pub path: String,
    pub bytes_written: u64,
    pub message: String,
}
```

Key: `content: String` (not `Option<String>`) ensures JSON Schema marks it as required. LLM cannot send null.

### file_edit (new, aligned with claude-code FileEditTool)

```rust
pub struct FileEditArgs {
    /// Absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find and replace
    pub old_string: String,
    /// The replacement string
    pub new_string: String,
    /// Replace all occurrences (default: false, fails if multiple matches)
    #[serde(default)]
    pub replace_all: bool,
}

pub struct FileEditOutput {
    pub success: bool,
    pub path: String,
    pub replacements: usize,
    pub message: String,
}
```

Behavior:
- Read file, find `old_string`, replace with `new_string`
- If `replace_all` is false and multiple matches found, return error asking to set `replace_all: true` or provide more context
- If `old_string` not found, return error with helpful message
- Preserves file encoding

### file_ops (retained, simplified)

Remove `Read` and `Write` variants from `FileOperation` enum:

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

All other behavior unchanged.

## File Structure

```
src/builtin_tools/file_ops/
├── tool.rs       — Modify: remove Read/Write branches from match
├── types.rs      — Modify: remove Read/Write from FileOperation enum
├── ops.rs        — Existing operation implementations (unchanged)
├── read.rs       — NEW: FileReadTool (AlephTool impl)
├── write.rs      — NEW: FileWriteTool (AlephTool impl)
├── edit.rs       — NEW: FileEditTool (AlephTool impl)
└── mod.rs        — Modify: add pub mod read/write/edit
```

## Registration

In `BuiltinToolRegistry` builder:

```rust
// New independent tools
registry.register(FileReadTool::new(denied_paths.clone(), max_read_size));
registry.register(FileWriteTool::new(denied_paths.clone()));
registry.register(FileEditTool::new(denied_paths.clone()));
// Retained (simplified)
registry.register(FileOpsTool::new()); // list, move, copy, delete, mkdir, search
```

## Shared Infrastructure

All 4 tools share:
- `denied_paths: Vec<PathBuf>` — path sandboxing
- `check_and_resolve_path()` — path validation helper
- `output_dir` resolution from `ToolContext`
- Existing `execute_*` helper functions from `ops.rs`

## Prompt / System Context Updates

Files referencing `file_ops` in prompts need to mention the new tools:
- `src/prompt/executor.rs`
- `src/prompt/conversational.rs`
- `src/thinker/prompt_builder/sections.rs`
- Agent tool whitelists using glob patterns (`fs_*` or explicit names)

## Migration

- No config changes needed — tool names are auto-discovered
- Agent whitelists: if an agent has `file_ops` in whitelist, also add `file_read`, `file_write`, `file_edit` (or use prefix matching)
- No data migration — purely code change

## Expected Impact

| Before | After |
|--------|-------|
| `content: null` bug in Write | `content: String` is required — impossible to be null |
| LLM retries 8+ times on Write | Write succeeds on first attempt |
| LLM cannot do partial edits | file_edit enables precise string replacement |
| 1 tool with 10 operations | 4 focused tools with clear schemas |
