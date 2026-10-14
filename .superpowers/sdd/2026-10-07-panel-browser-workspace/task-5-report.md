# Task 5 / #19 验收报告：Panel browser model / viewer shell

状态：**已完成（范围内：Panel 纯模型 + 渲染壳），独立 review PASS**。本报告只记录实际实现范围与实际验证结果。

## 1. 已实现范围

### 1.1 Panel browser model / viewer shell
- `views/browser/model.rs`：纯状态模型（`BrowserViewerModel` / `BoundBrowser` / `BrowserWorkspace`）。
- `views/browser/viewer.rs`：无 I/O 的可访问状态视图（`BrowserViewer`）。
- `views/browser/mod.rs`：模块声明与 re-export。

### 1.2 仅接受 Gateway 授权数据
- 模型**只能**由 Gateway 已授权的 `PreviewResource`（bus 事件）或 `BrowserLiveSnapshot`（viewer exchange）构造。
- 没有任何接受 profile / URL / "active tab" / target id 的构造器；Panel 不"决定"自己在看哪个浏览器。
- **无** URL / hash / network / issuer / producer / WS：不读 `location.hash`、不消费 fragment nonce、不换 ticket、不开 socket、不签发/生产资源。

### 1.3 Browser tab 可见性
- `WorkspaceBody::Browser`（`state/layout.rs`）：仅当 `BrowserWorkspace` context 存在**且**持有 bound model 时才出现（`available_with` / `coerce_body_with`）。
- context 缺失或 model 未绑定 -> Browser tab 隐藏（absent context / unbound model）；已存储的 Browser body 会 coerce 回该模式的首个 body。
- Browser 排在最后，不挤占 fallback 条目；`available` / `coerce_body` 保持原行为。
- `workspace_panel.rs` 中 context 为 optional；根部尚未提供（`TODO(Task6)`）。

### 1.4 单调 revision 与 binding 校验
- 同一 `resource_id`：viewer binding 必须完全一致，否则 `BindingMismatch`；旧 revision -> `StaleRevision`；同 revision -> 幂等 no-op；新 revision 仅刷新资源元数据（live facts / phase / mute / 仍被提供的选择保留）。
- 不同 `resource_id` 替换模型。
- 所有入口共用 `vet_resource`（`PreviewResource::validate` + 必须为 browser binding），不存在某路径接受另一路径会拒绝的数据。
- 更新（snapshot/state/closed）必须与绑定的 resource / binding 精确匹配，失败不改变模型（`ResourceMismatch` / `BindingMismatch` / `NoSnapshot` 等）。

### 1.5 Shared epoch 规则
- `connection_epoch` / `stream_epoch` / `control_epoch` **各自独立**要求非递减；任一轴回退 -> `EpochRegression`。
- 比字典序更严：不允许新 connection 上 `stream_epoch` 回落（wire 契约不承诺重置，故拒绝而非猜测）。

### 1.6 Pending 不展示（withheld）
- redaction 为 Pending（或防御性的 Refused）时，视图只显示一条本地化状态，不描述资源内容。

### 1.7 RefusedResource tombstone
- 被 redaction 拒绝的资源记 `RefusedResource { resource_id, revision }`（不含 target）。
- 同 id 且 revision <= 被拒 revision 不能复活（`StaleRevision` / `AlreadyRefused`）；同 revision 由 Safe 变 Refused 不是幂等 no-op，fail closed 并记 tombstone；严格更新的 revision 可重新开始。

### 1.8 ClosedResource tombstone
- viewer 关闭后记 `ClosedResource { resource_id, revision, reason }`（不含 target）。
- 同 id 且 revision <= 关闭 revision 不能复活（`StaleRevision` / `AlreadyClosed`）；仅严格更新的 revision 可重新开启。

### 1.9 Candidate write-through
- 候选选择只接受绑定资源自己发布的 candidate id，未知 id -> `UnknownCandidate`；选择写入模型（`select_candidate`），不是 viewer 本地状态；资源刷新时仍被提供的选择保留。

## 2. 变更文件
- `interfaces/webchat/locales/en.json`
- `interfaces/webchat/locales/zh.json`
- `interfaces/webchat/src/components/workspace_panel.rs`
- `interfaces/webchat/src/platform/wide/views/mod.rs`（`pub mod browser;`）
- `interfaces/webchat/src/platform/wide/views/browser/mod.rs`（新）
- `interfaces/webchat/src/platform/wide/views/browser/model.rs`（新）
- `interfaces/webchat/src/platform/wide/views/browser/viewer.rs`（新）
- `interfaces/webchat/src/state/layout.rs`
- `.superpowers/sdd/2026-10-07-panel-browser-workspace/task-5-report.md`（本报告）

## 3. 实际验证

| 项 | 命令 / 检查 | 结果 |
|---|---|---|
| Panel lib 测试 | `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo test --offline -p aleph-panel --lib` | 1428 passed |
| 格式 | rustfmt（针对性 browser 文件） | passed |
| Clippy | `cargo clippy --offline -p aleph-panel --lib -- -D warnings` | passed |
| 独立 review | reviewer | **PASS**；browser 相关测试 38/38 |

## 4. 明确限制 / 未做事项
- **未**做 wasm target 构建。
- **未**做组件 / 浏览器运行时测试（无 DOM 渲染、无真机 QA）。
- workspace 级 rustfmt 存在**既有**差异（与本任务无关，未处理）。
- Task6 的 producer / issuer / WS 与 `NetworkContextGrant` 仍为 **pending / 被 security gate 阻塞**；`BrowserWorkspace` context 目前无人提供，因此 Browser tab 在真实应用中暂不出现。
