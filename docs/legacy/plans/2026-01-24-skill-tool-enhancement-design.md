# Skill Tool Enhancement Design

**Date**: 2026-01-24
**Status**: Approved for Implementation

## Background

Based on comparative analysis with OpenCode (Claude Code open-source implementation), Aether's Skill system has several gaps:

1. **Missing Skill Tool** - Skills are statically injected into prompts; LLM cannot dynamically invoke them
2. **No Instance Caching** - File system scanned on every access
3. **Limited Template System** - Only `$ARGUMENTS` substitution, no file references
4. **Permission Not Integrated** - Permission types exist but not enforced during execution

## Goals

- Implement Skill as an LLM-callable Tool
- Add lazy-loading with instance-level caching
- Enhance template system with `@file` references
- Integrate permission checks into skill execution

## Non-Goals

- Shell command execution in templates (`!`command``) - security risk
- Full OpenCode parity - maintain Aether's architectural identity
- Breaking changes to existing APIs

## Design

### 1. SkillTool Implementation

New file: `core/src/extension/skill_tool.rs`

```rust
/// Skill Tool execution result
pub struct SkillToolResult {
    pub title: String,           // "Loaded skill: {name}"
    pub content: String,         // Rendered skill content
    pub base_dir: PathBuf,       // Source directory for relative paths
    pub metadata: SkillMetadata,
}

pub struct SkillMetadata {
    pub name: String,
    pub qualified_name: String,
    pub source: DiscoverySource,
}

/// Skill execution context (passed from Agent Loop)
pub struct SkillContext {
    pub session_id: String,
    pub agent_permissions: Option<HashMap<String, PermissionRule>>,
}
```

### 2. Caching Mechanism

Modify: `core/src/extension/mod.rs`

```rust
struct CacheState {
    loaded: bool,
    loaded_at: Option<Instant>,
}

impl ExtensionManager {
    /// Lazy-load entry point
    pub async fn ensure_loaded(&self) -> ExtensionResult<()>;

    /// Force reload (hot-update scenario)
    pub async fn reload(&self) -> ExtensionResult<LoadSummary>;
}
```

### 3. Template System

New file: `core/src/extension/template.rs`

Supported syntax:
| Syntax | Description | Example |
|--------|-------------|---------|
| `$ARGUMENTS` | Argument substitution | `Hello $ARGUMENTS!` |
| `@./path` | Relative file reference | `See @./config.json` |
| `@/abs/path` | Absolute file reference | `See @/etc/hosts` |

```rust
pub struct SkillTemplate {
    content: String,
    base_dir: PathBuf,
}

impl SkillTemplate {
    pub async fn render(&self, arguments: &str) -> ExtensionResult<String>;
    async fn expand_file_refs(&self, content: &str) -> ExtensionResult<String>;
}
```

### 4. Permission Integration

Modify: `core/src/extension/mod.rs` and `error.rs`

```rust
impl ExtensionManager {
    pub async fn invoke_skill_tool(
        &self,
        name: &str,
        arguments: &str,
        ctx: &SkillContext,
    ) -> ExtensionResult<SkillToolResult> {
        // 1. Check permission
        self.check_skill_permission(name, ctx).await?;

        // 2. Ensure loaded
        self.ensure_loaded().await?;

        // 3. Load and render
        let skill = self.get_skill(name).await?;
        let template = SkillTemplate::new(&skill.content, &skill.source_path);
        let rendered = template.render(arguments).await?;

        Ok(SkillToolResult { ... })
    }
}
```

New error variant:
```rust
pub enum ExtensionError {
    // existing...
    #[error("Permission denied for skill: {0}")]
    PermissionDenied(String),
}
```

## File Changes

| File | Type | Description |
|------|------|-------------|
| `extension/mod.rs` | Modify | Export new modules, add caching |
| `extension/skill_tool.rs` | **New** | SkillTool implementation |
| `extension/template.rs` | **New** | Template processor |
| `extension/types.rs` | Modify | Add result/context types |
| `extension/error.rs` | Modify | Add PermissionDenied |

## Public API

```rust
// New exports
pub use skill_tool::{SkillToolResult, SkillContext, SkillMetadata};
pub use template::SkillTemplate;

impl ExtensionManager {
    // New methods
    pub async fn ensure_loaded(&self) -> ExtensionResult<()>;
    pub async fn reload(&self) -> ExtensionResult<LoadSummary>;
    pub async fn invoke_skill_tool(...) -> ExtensionResult<SkillToolResult>;

    // Preserved (backward compatible)
    pub async fn execute_skill(&self, name: &str, args: &str) -> ExtensionResult<String>;
}
```

## Agent Loop Integration

```rust
// In agent_loop tool execution
match tool_call.name.as_str() {
    "skill" => {
        let ctx = SkillContext {
            session_id: session.id.clone(),
            agent_permissions: agent.permission.clone(),
        };
        let result = extension_mgr.invoke_skill_tool(
            &tool_call.params.name,
            &tool_call.params.arguments.unwrap_or_default(),
            &ctx,
        ).await?;
        // Return result.content as tool_result
    }
    // ...
}
```

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| File reference security | Only allow relative paths within skill directory |
| Cache staleness | Provide `reload()` for hot-update; consider file watcher later |
| Permission bypass | Default to `Ask` if no rule matches |

## Migration

No breaking changes. Existing `execute_skill()` preserved for backward compatibility.

## Open Questions

- [ ] Should we add a file watcher for auto-reload?
- [ ] Should permission requests go through EventBus?
