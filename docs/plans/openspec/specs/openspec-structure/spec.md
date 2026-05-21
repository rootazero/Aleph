# openspec-structure Specification

## Purpose
TBD - created by archiving change consolidate-openspec-architecture. Update Purpose after archive.
## Requirements
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

