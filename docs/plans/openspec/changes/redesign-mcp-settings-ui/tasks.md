# Tasks: Redesign MCP Settings UI

## Phase 1: 数据模型扩展 (Rust Core)

- [x] **T1.1**: 在 `mcp/types.rs` 中添加 `McpServerConfig` 结构体
  - 包含 `command`, `args`, `env`, `permissions`, `working_directory` 字段
  - 实现 `Serialize`/`Deserialize` trait
  - 验证：单元测试覆盖序列化/反序列化

- [x] **T1.2**: 添加 `McpServerStatus` 枚举
  - 状态：`Stopped`, `Starting`, `Running`, `Error(String)`
  - 导出到 UniFFI
  - 验证：状态转换测试

- [x] **T1.3**: 扩展 `McpConfig` 支持外部服务器
  - 添加 `external_servers: HashMap<String, McpServerConfig>` 字段
  - 兼容现有 `builtin.*` 配置
  - 验证：配置文件加载/保存测试

- [x] **T1.4**: 添加 UniFFI 接口方法
  - `add_external_mcp_server(id, config)` → `Result<()>`
  - `remove_external_mcp_server(id)` → `Result<()>`
  - `update_external_mcp_server(id, config)` → `Result<()>`
  - `get_mcp_server_status(id)` → `McpServerStatus`
  - 验证：UniFFI binding 生成成功

- [x] **T1.5**: 实现 `claude_desktop_config.json` 导入/导出
  - 解析 `mcpServers` JSON 结构
  - 转换为内部 `McpServerConfig` 格式
  - 验证：导入真实 claude_desktop_config.json 文件测试

## Phase 2: Master-Detail 布局 (Swift UI)

- [x] **T2.1**: 创建 `McpServerListView` 组件 (侧边栏)
  - 分组显示：Built-in Core / Extensions
  - 选中状态绑定
  - 添加/删除按钮
  - 验证：预览可见，交互正常

- [x] **T2.2**: 创建 `McpServerDetailView` 组件 (详情面板)
  - 头部区域：图标、名称、状态指示器、开关
  - 骨架布局：Form with sections
  - 验证：预览可见

- [x] **T2.3**: 重构 `McpSettingsView` 为 HSplitView
  - 左侧：`McpServerListView`
  - 右侧：`McpServerDetailView`
  - 选中状态管理：`@State selectedServerId`
  - 验证：双栏布局正确渲染

- [x] **T2.4**: 实现服务器选择逻辑
  - 点击列表项更新详情面板
  - 空选择状态处理
  - 首次加载默认选中第一项
  - 验证：交互测试

## Phase 3: 详情编辑器 (Swift UI)

- [x] **T3.1**: 创建 `McpCommandEditor` 组件
  - Command 路径输入 + Browse 按钮
  - Working Directory 输入
  - 验证：文件选择对话框正常工作

- [x] **T3.2**: 创建 `McpArgsEditor` 组件
  - 参数列表（可添加/删除/排序）
  - 每行一个参数
  - 验证：参数增删改正常

- [x] **T3.3**: 创建 `McpEnvVarEditor` 组件
  - Key-Value 表格
  - SecureField 掩码显示值
  - 眼睛图标切换可见性
  - 添加/删除变量按钮
  - 验证：敏感信息默认隐藏

- [x] **T3.4**: 创建 `McpPermissionsEditor` 组件
  - 确认对话框开关
  - 自动批准开关（带警告）
  - 允许路径列表
  - 验证：权限设置保存正确

- [x] **T3.5**: 实现 GUI/JSON 模式切换
  - Segmented Picker: GUI | JSON
  - JSON 模式使用 TextEditor
  - 模式切换时数据双向转换
  - 验证：GUI 修改后切换 JSON 显示正确，反之亦然

## Phase 4: 日志与调试

- [x] **T4.1**: 创建 `McpServerLogView` Sheet
  - 实时日志滚动显示
  - 时间戳 + 日志级别 + 消息
  - 清除日志按钮
  - 验证：日志实时更新

- [x] **T4.2**: 添加状态指示器到列表项
  - 绿色圆点：Running
  - 灰色圆点：Stopped
  - 红色圆点：Error
  - 旋转图标：Starting
  - 验证：状态变化时 UI 更新

- [x] **T4.3**: 实现错误提示与修复建议
  - 启动失败时显示错误消息
  - 常见错误的修复建议（如路径不存在、权限不足）
  - 验证：模拟错误场景

## Phase 5: 集成与本地化

- [x] **T5.1**: 更新 Localizable.strings
  - 添加所有新 UI 字符串
  - 英文/简体中文双语
  - 验证：切换语言后 UI 正确显示

- [x] **T5.2**: 集成到 SettingsView
  - 确保 SaveBar 正常工作
  - 配置变更检测
  - 验证：保存/取消功能正常

- [x] **T5.3**: 端到端测试
  - 添加外部服务器 → 配置 → 启动 → 查看日志 → 删除
  - 导入 claude_desktop_config.json
  - 验证：完整流程无错误

## 依赖关系

```
T1.1 ─┬─ T1.2 ─┬─ T1.3 ─── T1.4 ─── T1.5
      │        │
      └────────┴─────────────────────────────┐
                                             │
T2.1 ─┬─ T2.2 ─┬─ T2.3 ─── T2.4 ─────────────┤
      │        │                              │
      └────────┴──────────────────────────────┤
                                              │
T3.1 ─┬─ T3.2 ─┬─ T3.3 ─── T3.4 ─── T3.5 ────┤
      │        │                              │
      └────────┴──────────────────────────────┤
                                              │
T4.1 ─┬─ T4.2 ─── T4.3 ───────────────────────┤
      │                                       │
      └───────────────────────────────────────┤
                                              │
                              T5.1 ─── T5.2 ─── T5.3
```

**可并行工作：**
- Phase 1 (Rust) 与 Phase 2 (Swift 布局) 可部分并行
- T3.1, T3.2, T3.3, T3.4 可并行开发
- T4.1, T4.2 可并行开发

**关键路径：**
- T1.4 (UniFFI) → T2.3 (HSplitView) → T5.2 (集成) → T5.3 (E2E 测试)
