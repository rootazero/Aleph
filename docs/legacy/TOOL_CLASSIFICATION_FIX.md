# 工具分类系统修复总结

**日期**: 2026-01-27
**修复版本**: Phase 9+
**问题类型**: 工具智能推断系统缺陷

---

## 问题概述

在使用 classical-poetry skill 时发现，LLM 未按照预期执行 Python 脚本进行格律验证。经分析发现，这是由于工具分类系统设计缺陷导致 `bash` 工具被降级为 "Additional Tools"，LLM 倾向于跳过需要额外步骤的工具调用。

---

## 根本原因分析

### 1. ToolCategory 缺失关键类别

**位置**: `core/src/ffi/tool_discovery.rs` (第 12-23 行)

**问题**: ToolCategory enum 只包含 8 个类别，缺少 `Bash` 和 `CodeExec`：

```rust
pub enum ToolCategory {
    FileOps, Search, WebFetch, YouTube,
    ImageGen, VideoGen, AudioGen, SpeechGen,
    // ❌ 缺少：Bash, CodeExec
}
```

**影响**: 即使 skill 包含 `python3 $SKILL_ROOT/scripts/poetry_checker.py`，推断系统也无法识别需要 Bash 工具。

---

### 2. 推断逻辑不完整

**位置**: `core/src/ffi/tool_discovery.rs::infer_required_tools()` (第 27-139 行)

**问题**: 缺少对脚本执行模式的检测逻辑：
- 未检测 `python3`、`bash`、`scripts/` 等关键词
- 未识别 classical-poetry 的特定模式（格律验证、poetry_checker）

**结果**: 日志显示 `inferred_categories=[FileOps]`，Bash 工具被遗漏。

---

### 3. Smart Filter 过滤映射缺失

**位置**: `core/src/dispatcher/smart_filter.rs::tool_matches_category()` (第 165-195 行)

**问题**: 匹配函数缺少 Bash 和 CodeExec 的分支，导致即使推断出这些类别，也无法将工具提升为 full-schema。

---

### 4. 日志证据

从 `dilog.md` 第 1261-1265 行：

```
inferred_categories=[FileOps]
Smart filter result, core_tools=2, filtered_tools=0, indexed_tools=5
```

LLM prompt 中 bash 被降级：

```
### Additional Tools (use `get_tool_schema` to get parameters)
- bash: Bash命令执行 - 执行bash/shell命令...
```

**影响**: LLM 需要两步操作（get_tool_schema + 调用），倾向于直接 `complete`。

---

## 修复内容

### 修改 1: 扩展 ToolCategory enum

**文件**: `core/src/ffi/tool_discovery.rs`
**行数**: 12-23

```rust
pub enum ToolCategory {
    FileOps,
    Search,
    WebFetch,
    YouTube,
    Bash,      // ← 新增
    CodeExec,  // ← 新增
    ImageGen,
    VideoGen,
    AudioGen,
    SpeechGen,
}
```

---

### 修改 2: 增强脚本检测逻辑

**文件**: `core/src/ffi/tool_discovery.rs`
**行数**: 130-163

添加 Bash 和 CodeExec 的检测逻辑：

```rust
// Bash/shell execution - detect script execution patterns
let needs_bash = combined.contains("bash")
    || combined.contains("shell")
    || combined.contains("python3")
    || combined.contains("node ")
    || combined.contains("scripts/")
    || combined.contains(".py")
    || combined.contains(".sh")
    || combined.contains("$skill_root")
    || combined.contains("格律验证")  // classical-poetry specific
    || combined.contains("poetry_checker")
    || combined.contains("reference_builder");
if needs_bash {
    categories.push(ToolCategory::Bash);
}

// Code execution
let needs_code_exec = combined.contains("code_exec")
    || combined.contains("execute code")
    || combined.contains("代码执行")
    || (combined.contains("execute") && combined.contains("python"));
if needs_code_exec {
    categories.push(ToolCategory::CodeExec);
}
```

---

### 修改 3: 更新过滤映射

**文件**: `core/src/ffi/tool_discovery.rs`
**行数**: 502-513

```rust
categories.iter().any(|cat| match cat {
    ToolCategory::FileOps => name == "file_ops",
    ToolCategory::Search => name == "search",
    ToolCategory::WebFetch => name == "web_fetch",
    ToolCategory::YouTube => name == "youtube",
    ToolCategory::Bash => name == "bash",           // ← 新增
    ToolCategory::CodeExec => name == "code_exec",  // ← 新增
    ToolCategory::ImageGen => name == "generate_image",
    // ...
})
```

---

### 修改 4: 更新 Smart Filter 匹配逻辑

**文件**: `core/src/dispatcher/smart_filter.rs`
**行数**: 182-187

```rust
ToolCategory::Bash => {
    name == "bash" || desc_lower.contains("bash")
        || desc_lower.contains("shell") || desc_lower.contains("command")
}
ToolCategory::CodeExec => {
    name == "code_exec" || desc_lower.contains("code")
        || desc_lower.contains("execute")
}
```

---

## 测试验证

### 1. 编译测试

```bash
cd core && cargo build
# ✅ Finished `dev` profile in 32.78s
```

### 2. Smart Filter 测试

```bash
cargo test --lib dispatcher::smart_filter
# ✅ 5 passed; 0 failed
```

### 3. 新增工具推断测试

**文件**: `core/src/ffi/tool_discovery.rs` (新增 tests 模块)

测试用例：
- `test_infer_bash_from_script_patterns` - 检测 python3 脚本调用
- `test_infer_bash_from_poetry_keywords` - 检测格律验证关键词
- `test_infer_bash_from_shell_patterns` - 检测多种 shell 模式
- `test_infer_code_exec` - 检测代码执行需求
- `test_filter_tools_includes_bash` - 验证过滤器包含 bash

```bash
cargo test --lib ffi::tool_discovery::tests
# ✅ 5 passed; 0 failed
```

---

## 预期效果

### 修复前

```
inferred_categories=[FileOps]
Smart filter result, core_tools=2, filtered_tools=0, indexed_tools=5

### Additional Tools
- bash: Bash命令执行...
```

LLM 跳过 bash 工具调用，直接 complete。

---

### 修复后

```
inferred_categories=[FileOps, Bash]
Smart filter result, core_tools=3, filtered_tools=0, indexed_tools=4

### Tools (with full parameters)
#### bash
Bash命令执行 - 执行bash/shell命令...
Parameters: {...}
```

Bash 工具成为 core tool，LLM 可以直接调用执行格律验证脚本。

---

## 影响范围

### 受益 Skills

所有需要脚本执行的 skills：
- ✅ **classical-poetry**: poetry_checker.py, reference_builder.py
- ✅ **build-macos-apps**: xcodebuild, swift scripts
- ✅ **build-iphone-apps**: 同上
- ✅ 任何包含 Python/Node/Shell 脚本调用的自定义 skills

### 向后兼容性

✅ **完全兼容**：所有现有工具和 skills 继续正常工作。新增类别不影响已有逻辑。

---

## 遗留问题与建议

### 1. Skill 执行验证机制（建议实现）

**问题**: 当前 LLM 可以绕过 skill 工作流直接 complete，导致关键步骤被跳过。

**建议**: 在 `core/src/ffi/agent_loop_adapter.rs` 中添加工作流验证：

```rust
// 检查 skill 是否按照预期执行
if let Some(skill_id) = &context.skill_id {
    if skill_id == "classical-poetry" {
        // 验证是否调用了 poetry_checker.py
        if !execution_history.contains_bash_call("poetry_checker.py") {
            return Err("Skill workflow incomplete: missing poetry_checker.py");
        }
    }
}
```

---

### 2. Skill 元数据增强（未来优化）

**建议**: 在 skill 定义中显式声明所需工具：

```yaml
# skill.yaml
tools:
  required:
    - bash
    - file_ops
  optional:
    - search
```

这样可以绕过推断逻辑，直接保证必需工具可用。

---

### 3. 工具调用日志增强（监控需求）

**建议**: 为每个 skill 执行记录工具调用序列，便于调试：

```
[Skill: classical-poetry]
  Step 1: ask_user (韵书选择)
  Step 2: bash (reference_builder.py)  // ❌ 缺失
  Step 3: bash (poetry_checker.py)     // ❌ 缺失
  Step 4: complete (跳过验证)
```

---

## 总结

此次修复解决了工具分类系统的根本性缺陷，确保需要脚本执行的 skills 能正确获得 Bash/CodeExec 工具支持。修复过程中：

- ✅ 未丢失任何现有逻辑
- ✅ 未引入向后兼容性问题
- ✅ 所有测试通过
- ✅ 增强了系统对复杂 skills 的支持能力

**重要性**: 这是 Aether Skills 功能完善的关键一步，直接影响所有需要外部工具调用的 skills 的可靠性。

---

## 参考

- **Commit**: `[待 commit]`
- **相关文件**:
  - `core/src/ffi/tool_discovery.rs`
  - `core/src/dispatcher/smart_filter.rs`
- **测试覆盖**: 10 个单元测试全部通过
- **文档**: 本文档 + inline 代码注释
