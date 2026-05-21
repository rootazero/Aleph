# Capability: openspec-structure

## Description

定义 OpenSpec 目录结构的组织规范，确保单一规范的根目录、扁平的变更层级和清晰的职责分离。

## ADDED Requirements

### Requirement: 单一 OpenSpec 根目录 {#single-root}

**Priority**: MUST
**Category**: Structure

The project MUST maintain only one OpenSpec root directory, located at `/openspec/` from the project root path.

#### Scenario: 检查 OpenSpec 根目录唯一性

**Given** 项目根目录结构
**When** 搜索所有名为 "openspec" 的目录（排除 `.git` 和 `.claude/commands/`）
**Then** 应该只找到一个目录：`/openspec/`
**And** 不应存在 `/Aleph/openspec/` 或其他子目录中的 openspec

```bash
# 验证命令
find . -type d -name "openspec" \
  -not -path "./.git/*" \
  -not -path "./.claude/*" \
  | wc -l
# 期望输出: 1
```

#### Scenario: OpenSpec 工具正常工作

**Given** 单一的 `/openspec/` 根目录
**When** 运行 `openspec list` 命令
**Then** 应显示所有在 `/openspec/changes/` 下的活跃 changes
**And** 不应报告路径错误或找不到目录

---

### Requirement: 禁止嵌套 OpenSpec 结构 {#no-nested-openspec}

**Priority**: MUST
**Category**: Structure

The project MUST NOT create any directory named "openspec" within subdirectories of `/openspec/` root, preventing deep nesting and confusion.

#### Scenario: 检测非法嵌套结构

**Given** OpenSpec 根目录 `/openspec/`
**When** 在 `/openspec/` 及其子目录中搜索名为 "openspec" 的目录
**Then** 除了根目录本身，不应找到任何其他 "openspec" 目录
**And** 特别是不应存在：
  - `/openspec/changes/*/openspec/`
  - `/openspec/specs/*/openspec/`
  - 任何深度超过 2 层的 openspec 目录

```bash
# 验证命令
find openspec/ -mindepth 1 -type d -name "openspec"
# 期望输出: (空)
```

#### Scenario: 预提交钩子拒绝嵌套结构

**Given** 开发者尝试提交包含嵌套 openspec 的代码
**When** 运行 Git pre-commit 钩子
**Then** 钩子应检测到非法结构
**And** 阻止提交并显示错误消息
**And** 提示开发者将内容移到正确位置

---

### Requirement: 扁平的 Changes 层级 {#flat-changes}

**Priority**: MUST
**Category**: Structure

All change proposals MUST be located as direct subdirectories of `/openspec/changes/`, and archived changes MUST be under `/openspec/changes/archive/`.

#### Scenario: Change 目录位于正确深度

**Given** 一个新的变更提案 ID 为 `my-new-feature`
**When** 创建变更目录
**Then** 应位于 `/openspec/changes/my-new-feature/`
**And** 深度应该是 2（从 openspec 根开始计数）
**And** 不应创建更深的嵌套结构

#### Scenario: 归档的 Change 位于 archive 目录

**Given** 一个已完成的变更需要归档
**When** 归档该变更
**Then** 应移动到 `/openspec/changes/archive/YYYY-MM-DD-<id>/`
**And** 深度应该是 3（从 openspec 根开始计数）
**And** 日期前缀格式为 `YYYY-MM-DD`

```bash
# 验证 changes 深度
find openspec/changes -mindepth 1 -maxdepth 1 -type d | while read dir; do
  if [ "$(basename "$dir")" != "archive" ]; then
    # 活跃 changes 深度应为 2
    depth=$(echo "$dir" | tr -cd '/' | wc -c)
    echo "$dir: depth=$depth (expected: 2)"
  fi
done
```

---

### Requirement: 清晰的内容分离 {#content-separation}

**Priority**: MUST
**Category**: Organization

OpenSpec content (specifications, proposals, tasks) MUST be physically separated from application source code. The OpenSpec directory SHALL only contain metadata and documentation files.

#### Scenario: OpenSpec 不包含源代码

**Given** OpenSpec 根目录 `/openspec/`
**When** 遍历 openspec 目录中的所有文件
**Then** 应只包含文档文件：
  - `.md` (Markdown)
  - `.txt` (纯文本)
  - `.json` (配置或元数据)
**And** 不应包含源代码文件：
  - `.rs`, `.swift`, `.c`, `.cpp`, `.py`, `.js`, `.ts` 等

```bash
# 检查非法源代码文件
find openspec/ -type f \
  \( -name "*.rs" -o -name "*.swift" -o -name "*.c" \
     -o -name "*.cpp" -o -name "*.py" -o -name "*.js" \) \
  | wc -l
# 期望输出: 0
```

#### Scenario: 应用代码不包含 openspec 子目录

**Given** 应用源代码目录（如 `/Aleph/`, `/core/`）
**When** 在应用代码目录中搜索 openspec 目录
**Then** 不应找到任何 openspec 子目录
**And** 应用代码只通过文档引用 openspec 内容

```bash
# 验证应用代码目录无 openspec
find Aleph/ -type d -name "openspec" | wc -l
# 期望输出: 0
```

---

### Requirement: 标准目录结构 {#standard-structure}

**Priority**: MUST
**Category**: Convention

The OpenSpec root directory MUST follow a standard structure, containing required subdirectories and files.

#### Scenario: 必需的顶层元素存在

**Given** OpenSpec 根目录 `/openspec/`
**When** 列出顶层内容
**Then** 必须包含以下元素：
  - `AGENTS.md` - OpenSpec 代理使用指南
  - `project.md` - 项目上下文和约定
  - `changes/` - 变更提案目录
  - `specs/` - 规范定义目录

```bash
# 验证必需元素
for item in AGENTS.md project.md changes specs; do
  if [ ! -e "openspec/$item" ]; then
    echo "Missing required: $item"
  fi
done
```

#### Scenario: Changes 目录包含正确的子结构

**Given** Changes 目录 `/openspec/changes/`
**When** 检查 changes 的子目录结构
**Then** 每个活跃的 change 目录必须包含：
  - `proposal.md` - 提案文档
  - `tasks.md` - 任务列表
  - (可选) `design.md` - 设计文档
  - (可选) `specs/` - Spec deltas

#### Scenario: Specs 目录包含能力定义

**Given** Specs 目录 `/openspec/specs/`
**When** 列出 specs 的子目录
**Then** 每个子目录应代表一个能力（capability）
**And** 每个能力目录必须包含 `spec.md` 文件
**And** spec.md 使用规范的 Requirements 格式

---

## REMOVED Requirements

(无删除的需求 - 这是新增的规范)

---

## MODIFIED Requirements

(无修改的需求 - 这是新增的规范)

---

## Dependencies

本规范依赖以下能力：
- 无（这是基础组织规范）

本规范被以下能力依赖：
- 所有使用 OpenSpec 的功能开发流程

---

## Implementation Notes

### 迁移现有项目

对于已有嵌套或重复 OpenSpec 结构的项目：

1. **审计**: 使用 `find` 命令识别所有 openspec 目录
2. **备份**: 完整备份当前结构
3. **比较**: 识别重复内容
4. **整合**: 将独特内容移到主 openspec，删除重复
5. **验证**: 运行 `openspec validate --all`

### 防止未来违规

实施以下机制：

1. **.gitignore 规则**:
   ```gitignore
   # 防止子目录创建 openspec
   */openspec/
   !.claude/commands/openspec/
   !/openspec/
   ```

2. **Pre-commit 钩子**:
   ```bash
   #!/bin/bash
   # 检查嵌套 openspec
   illegal=$(find . -type d -name "openspec" \
     -not -path "./openspec" \
     -not -path "./.claude/*" \
     -not -path "./.git/*")

   if [ -n "$illegal" ]; then
     echo "Error: Nested openspec detected"
     exit 1
   fi
   ```

3. **CI 检查**:
   ```yaml
   - name: Validate OpenSpec Structure
     run: |
       if [ $(find . -type d -name "openspec" \
         -not -path "./openspec" \
         -not -path "./.claude/*" | wc -l) -gt 0 ]; then
         echo "Error: Invalid OpenSpec structure"
         exit 1
       fi
   ```

### 工具兼容性

确保 OpenSpec CLI 工具正常工作：

```bash
# 所有命令应无错误运行
openspec list
openspec list --specs
openspec validate --all
openspec show <change-id>
```

---

## Testing Strategy

### 结构验证测试

```bash
# Test 1: 单一根目录
test_single_root() {
  count=$(find . -type d -name "openspec" \
    -not -path "./.git/*" \
    -not -path "./.claude/*" | wc -l)
  [ $count -eq 1 ] || echo "FAIL: Multiple openspec roots"
}

# Test 2: 无嵌套
test_no_nesting() {
  nested=$(find openspec/ -mindepth 1 -type d -name "openspec")
  [ -z "$nested" ] || echo "FAIL: Nested openspec found"
}

# Test 3: 标准结构
test_standard_structure() {
  for item in AGENTS.md project.md changes specs; do
    [ -e "openspec/$item" ] || echo "FAIL: Missing $item"
  done
}

# Test 4: Changes 深度
test_changes_depth() {
  find openspec/changes -mindepth 1 -maxdepth 1 -type d \
    -not -name "archive" | while read dir; do
    depth=$(echo "$dir" | awk -F/ '{print NF}')
    [ $depth -eq 3 ] || echo "FAIL: Wrong depth for $dir"
  done
}
```

### 工具集成测试

```bash
# Test 5: OpenSpec 命令
test_openspec_commands() {
  openspec list > /dev/null || echo "FAIL: openspec list"
  openspec list --specs > /dev/null || echo "FAIL: openspec list --specs"
  openspec validate --all || echo "FAIL: openspec validate"
}
```

---

## Migration Path

从旧结构迁移到新结构：

### Before (旧结构)

```
project/
├── openspec/
│   └── changes/
│       └── feature-a/
└── Aleph/
    └── openspec/          # 重复
        └── changes/
            └── feature-b/
                └── openspec/  # 嵌套
                    └── changes/
```

### After (新结构)

```
project/
├── openspec/              # 唯一根目录
│   ├── AGENTS.md
│   ├── project.md
│   ├── changes/
│   │   ├── feature-a/
│   │   ├── feature-b/     # 从 Aleph 移动
│   │   └── archive/
│   └── specs/
└── Aleph/                # 无 openspec 子目录
    └── (源代码)
```

### 迁移步骤

1. 备份现有结构
2. 提取 Aleph/openspec 中的独特内容
3. 将独特内容移到主 openspec/changes/
4. 删除 Aleph/openspec 整个目录
5. 更新 .gitignore
6. 运行验证测试
7. 提交变更

---

## References

- OpenSpec 工具文档: `/.claude/commands/openspec/`
- 项目约定: `/openspec/project.md`
- 变更提案: `/openspec/changes/consolidate-openspec-architecture/proposal.md`
