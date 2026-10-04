# Workspace, Repositories, Memory & Tools

> Workspace 成员 / 官方仓库 / 长期记忆与质量门 / 内置工具使用约定。**Tier 1 只放指针**；详情在这里。

---

## Workspace Members

```
desktop/shared       # DesktopCapability trait + IPC
desktop/macos        # macOS native implementation
desktop/linux        # Linux native implementation
desktop/windows      # Windows native implementation
shared/logging       # Logging infrastructure
shared/protocol      # Shared protocol types
shared/ui_logic      # Shared UI logic
shared/client        # Shared client utilities
interfaces/cli       # CLI client
interfaces/tui       # TUI client
interfaces/webchat   # Web chat interface
```

→ [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)

---

## Official Repositories

| 仓库 | 路径 |
|------|------|
| **Aleph（主项目）** — Rust Core + 多端架构 | `/Volumes/TBU/Workspace/Aleph` |
| Aleph-Hub（扩展目录中心）· Aleph-homepage（Next.js 首页）· Aleph-docs · Aleph-mcp · Aleph-plugins · Aleph-skills | `/Volumes/TBU4/Workspace/`（Hub 与 homepage 在 TBU 上另有检出） |

> ⚠️ **挂载点**：`/Volumes/TBU4` **经常未挂载**。**会话工作检出、git root、编辑落点一律是 `/Volumes/TBU/Workspace/Aleph`**——`TBU4/Workspace/Aleph` 是第二份检出，别跨盘编辑。动周边仓前先 `ls /Volumes/`，别从一次 TBU4 miss 得出"参考项目不可用"。
> 7 仓为同级兄弟目录，远端均在 `github.com/rootazero/`。**始终从主项目 `Aleph/` 启动会话**，周边仓作为兄弟目录就地操作——这样跨会话记忆统一沉淀到主项目的全局 memory 库，spec/plan 统一落在 `docs/superpowers/{specs,plans}`（`docs/` 树已纳入 git）。周边仓的 spec 以子项目名作文件名前缀。

---

## Long-term Memory & Quality Hooks

- **长期记忆**：走各自 agent 的全局 memory（Claude Code = `~/.claude/projects/.../memory/`，Pi 走 Pi 自身全局库），跨会话、Git 不追踪。**不在项目内另造 MEMORY.md**——避免与全局记忆双源冲突。
- **质量门 (Hooks)**：当前**未挂**对应 agent 的 hooks 目录。本文件的规则目前靠模型遵守；未来如需强制执行层（如 PostToolUse → `cargo fmt`），在对应 agent 的 hooks 目录配置即可。
- 记忆系统全文 → [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md)

---

## 工具使用约定

搜索用 grep/find 内置工具，多 OR 词用一次 alternation（如 `grep{pattern:"a|b|c"}`）；必须走 bash 时用 `rg` 而不是 `grep`。定位后用 `file_read{offset,limit}` 只读命中附近，工作区外已知文件直接 read。先 `files_only: true` 拿路径，再读命中行——避免一次性 dump 整文件。

工具机制与闸 → [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [FEATURE_LOCATOR §3.4](docs/reference/FEATURE_LOCATOR.md)
