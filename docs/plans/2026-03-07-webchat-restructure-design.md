# WebChat 工程重组设计

## 背景

Aleph 的 UI 层当前存在三个独立项目，位置分散、职责重叠：

| 项目 | 技术栈 | 位置 | 问题 |
|------|--------|------|------|
| **webchat** | React/TypeScript | `ui/webchat/` | 孤立在 `ui/` 目录，与其他 app 不在一起 |
| **dashboard** | Leptos/WASM | `apps/dashboard/` | 功能完全被 control_plane 覆盖，是早期原型 |
| **control_plane** | Leptos/WASM | `core/ui/control_plane/` | 实际上是 Panel，位置藏在 core 子目录里 |

### 关键发现

1. **webchat 保留 React 是正确的** — Chat 是渲染密集型 UI（markdown、代码高亮、流式文本），React 生态的 `react-markdown` + `react-syntax-highlighter` 远优于 Leptos/WASM 中的替代方案（pulldown-cmark + syntect 体积大、生态不成熟）
2. **webchat 符合 R4 红线** — 它是纯 I/O 层，零业务逻辑，只渲染消息 + 发送 WebSocket RPC
3. **apps/dashboard 完全冗余** — Panel 的 Dashboard 标签页已覆盖其全部功能（home、system_status、agent_trace、memory），且多出 chat、settings、cron、logs
4. **control_plane 就是 Panel** — 内含 Chat / Dashboard / Settings 三大底部菜单，是统一管理界面

## 设计

### 目录结构调整

```
调整前：
aleph/
├── ui/webchat/                    # React 聊天（孤立位置）
├── apps/dashboard/                # Leptos 仪表盘（旧原型，冗余）
├── core/ui/control_plane/         # Leptos Panel（藏在 core 里）

调整后：
aleph/
├── apps/
│   ├── cli/
│   ├── desktop/
│   ├── macos-native/
│   ├── webchat/                   # React 聊天 UI（从 ui/webchat/ 移入）
│   └── panel/                     # Leptos Panel（从 core/ui/control_plane/ 移入）
│       └── src/views/
│           ├── chat/              # 底部菜单: Chat
│           ├── (home, trace...)   # 底部菜单: Dashboard
│           └── settings/          # 底部菜单: Settings
```

删除：
- `apps/dashboard/` — 功能已被 Panel 覆盖
- `ui/` — 清空后删除

### 定位明确化

| UI | 技术栈 | 定位 | 用户 |
|----|--------|------|------|
| **webchat** | React/TS | 用户主聊天界面（富 markdown/代码高亮） | 终端用户 |
| **panel** | Leptos/WASM | 管理控制面板（设置/监控/简洁聊天） | 管理员/高级用户 |

### 需要更新的引用

1. **Cargo.toml workspace members** — `core/ui/control_plane` → `apps/panel`
2. **Panel Cargo.toml** — `shared-ui-logic` 的 `path` 引用更新
3. **serve_webchat 路径** — `handlers.rs` 中自动发现路径 `ui/webchat/dist` → `apps/webchat/dist`
4. **crate 名称** — `aleph_dashboard` → `aleph_panel`（消除与旧 dashboard 的混淆）

### 构建集成

在 justfile 中添加统一构建任务：

```just
webchat-dev:
    cd apps/webchat && pnpm dev

webchat-build:
    cd apps/webchat && pnpm build

panel-dev:
    cd apps/panel && trunk serve

panel-build:
    cd apps/panel && trunk build --release
```

### 不做的事

- 不迁移 React 代码到 Leptos — 用正确的工具做正确的事
- 不合并 Panel chat 和 webchat — 定位不同（管理 vs 用户）
- 不改变 webchat 的技术栈或内部结构
- 不改变 Panel 的内部视图结构
