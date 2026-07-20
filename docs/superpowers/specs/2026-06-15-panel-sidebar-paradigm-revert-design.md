# Panel 左侧栏范式回退设计 (Sidebar Paradigm Revert)

**日期**: 2026-06-15
**状态**: 待实现
**类型**: 回退 / 死代码清理

## 背景与问题

`panel-sidebar-enrich` 分支（合并于 `eaa595ac0`）的初衷只是**针对 chat 窗口添加项目管理/团队管理快速入口**，但实际改动**替换了整个左侧栏的交互范式**，这是一个严重的范式越界错误。原先设计的操作范式不应被改动。

用户的三点反馈，本质上指向同一件事——回退到原始范式：

1. 失去了原先**弹窗选择 tab** 的操作感 → 恢复弹窗。
2. 底部排列的 tab 标签网格（`NavMenu` 2 列 grid）**过滤掉了 Chat**，一旦选择其他 tab 就**回不到 chat 窗口** → 删除底部网格，恢复弹窗（弹窗本来就含 Chat）。
3. 顶部新增的设置齿轮按钮 → 删除，Settings 回到弹窗菜单（弹窗本来就含 Settings）。

进一步澄清后确认：会话列表的"置顶/最近/项目"三分区折叠分组，以及底部 Footer（agent 切换器 + 网关状态点）也**一并回退**。

## 基线（原始范式）

原始范式 = 合并前的 main `87cd90138`（即合并提交 `eaa595ac0` 的另一个 parent）。其形态：

- **`ModeSidebar`**: 品牌行（ℵ + 主题切换 + 折叠按钮，**无设置齿轮**） → 区域二级菜单 → **`NavMenu` 弹窗**（紧凑触发按钮，向上弹出，含全部 6 个区域 Chat/Dashboard/Memory/Agents/Teams/Settings，当前项打勾）。**无 Footer。**
- **`chat_sidebar.rs`**: 原始扁平会话列表 + **内联 agent 切换器**（Footer 提交把它移走了，需还原）。无置顶/最近/项目分区、无 pin/unpin 菜单项。
- **`sidebar_footer.rs`**: 不存在。

## 要回退的分支提交清单

| 提交 | 内容 | 处理 |
|---|---|---|
| `9f1f1d69e` | 弹窗 → 持久化 2 列网格（丢掉 Chat/Settings） | 回退 |
| `8d1cadb34` | header 设置齿轮 | 回退 |
| `a2552260f` | SidebarFooter（agent 切换器 + 网关状态点），移除内联 agent 下拉 | 回退 |
| `283e6f948` | chat 侧栏 置顶/最近/项目 三分区 | 回退 |
| `a162c965b` | pin/unpin 菜单项 | 回退 |
| `5a20a883c` | **后端**: SessionInfo pinned/project_root 字段 + sessions.set_pinned RPC + project_root 持久化 | 清死代码（见下） |

## 设计

### 第一部分：前端范式回退（4 文件回退，1 文件删除）

机制：**对完全回退的 3 个文件用 `git checkout 87cd90138 -- <path>`**（字节级精确还原，零漂移，优于 `git revert` 合并提交——后者会牵连后端提交和后续的合并修复 `8d38ed520`）；`mod.rs` 做**外科编辑**（仅删一行），避免冲掉无关模块声明。

1. `interfaces/webchat/src/components/nav_menu.rs` → `checkout 87cd90138`（弹窗版本）。
2. `interfaces/webchat/src/components/mode_sidebar.rs` → `checkout 87cd90138`（无齿轮、无 Footer 行、保留弹窗 `<NavMenu/>`）。
3. `interfaces/webchat/src/components/chat_sidebar.rs` → `checkout 87cd90138`（扁平会话列表 + 内联 agent 切换器）。
4. `interfaces/webchat/src/components/mod.rs` → 外科删除 `pub mod sidebar_footer;` 一行。
5. `interfaces/webchat/src/components/sidebar_footer.rs` → 删除文件。

### 第二部分：后端死代码清理（保留 updated_at 真 bug 修复）

`5a20a883c` 的 `updated_at` 修复是一个真实的正确性修复（`sessions.list` 从不发 `updated_at`），与 `pinned`/`project_root` 在代码中**可干净分离**（已核实 `query.rs` / `types.rs` 两处的 `updated_at` 与 `pinned`/`project_root` 互不依赖）。决策：**删 `pinned`/`project_root`/`set_pinned` 全部死代码，保留 `updated_at`。**

> 注：回退前端后，原始 chat 侧栏不消费 `updated_at`（原始 panel 的 SessionEntry 无此字段，JSON 多余字段被忽略），故 `updated_at` 暂为无消费者的潜在字段——但作为正确性改进保留，且为日后"快速入口"工作预留。

清理范围（以 `87cd90138..eaa595ac0` 的分支 diff 为权威移除清单，逐文件外科移除分支新增；**不是**整文件 checkout，以免冲掉合并后的其他改动）：

**保留 updated_at、仅删 pinned/project_root：**
- `src/gateway/handlers/session/db_handlers/types.rs` — 删 `pinned`、`project_root` 字段；**保留** `updated_at: i64`。
- `src/gateway/handlers/session/db_handlers/query.rs` — 删 `pinned`/`project_root` 的 let 绑定 + 两个 struct 字段；**保留** `let updated_at = m.last_active_at;` + `updated_at,` 字段。

**整段移除（全部关于 set_pinned/set_project_root/project_root 持久化）：**
- `src/gateway/handlers/session/db_handlers/modify.rs` — set_pinned handler（+58）。
- `src/gateway/handlers/session/db_handlers/mod.rs` — modify handler 的接线（+2/-1）。
- `src/gateway/handlers/session/mod.rs` — 导出（+3/-1）。
- `src/gateway/router.rs` — `sessions.set_pinned` 路由（+5）。
- `src/gateway/session_manager/ops/modify.rs` — set_pinned/set_project_root ops（+81）。
- `src/gateway/session_store/mod.rs` — trait 默认方法 set_pinned/set_project_root（+13）。
- `src/gateway/session_store/file_backend/mod.rs` — 实现（+34）。
- `src/gateway/session_store/sqlite_backend/mod.rs` — 实现（+14）。
- `src/bin/aleph-server/commands/start/builder/handlers/session.rs` — project_root 持久化（+6）。
- `src/gateway/handlers/agent.rs` — run-start 路径上的 project_root 捕获接线（+11）。

> ⚠️ `project_root` 作为概念在 `src/session/events.rs`、`src/session/state.rs`、`src/context/compact/session_split.rs`、`src/session/store.rs` 等处**早已存在**，与本分支无关——**不得**误删。仅删本分支在上述 gateway/start 文件中新增的部分。

### 第三部分（本次不做，已明确推迟）

原始真实目标——"针对 chat 窗口添加项目管理/团队管理快速入口"——**本次不做**。回退落定后另开一轮单独 brainstorm，确保新增入口**不打破弹窗范式**。

## 成功标准 / 验证

- [ ] 左侧栏底部恢复为**弹窗触发按钮**（点击向上弹出 6 个区域，含 Chat 与 Settings，当前项打勾）；无 2 列网格。
- [ ] header 无设置齿轮；Settings 仅经弹窗进入。
- [ ] chat 侧栏为原始扁平会话列表 + 内联 agent 切换器；无置顶/最近/项目分区、无 pin/unpin 菜单项。
- [ ] `sidebar_footer.rs` 已删除，`mod.rs` 无 `sidebar_footer` 声明。
- [ ] 后端 `sessions.set_pinned` RPC、`set_pinned`/`set_project_root` ops/impl、SessionInfo `pinned`/`project_root` 字段、start 路径 project_root 持久化全部移除。
- [ ] `updated_at` 修复保留。
- [ ] `cargo check -p alephcore` 与 `just wasm` 编译通过（panel 与 server 双侧）。
- [ ] 部署链：`just wasm` → 重编 `aleph-server` binary → 替换运行中 binary → Reload Panel，肉眼确认弹窗范式回归且可在各区域间往返（特别是能回到 Chat）。

## 风险

- **低**。前端是字节级 checkout 回到验证过的原始版本；后端是按 diff 反向移除新增代码，`updated_at` 与待删项已确认可分离。
- 唯一需要人工核对处：`query.rs`/`types.rs` 的外科分离，以及确保不误删 `src/session/*` 中早已存在的 `project_root`。
