# Tasks: Refactor Skills UI Architecture

## Phase 1: 数据模型统一 (Rust Core)

### 1.1 定义统一数据模型
- [ ] 在 `aether.udl` 中定义 `SkillType` 枚举
- [ ] 在 `aether.udl` 中定义 `UnifiedSkillConfig` 结构
- [ ] 在 `aether.udl` 中定义 `SkillPermissions` 结构
- [ ] 在 `aether.udl` 中定义 `SkillStatus` 枚举

### 1.2 实现 Rust 数据结构
- [ ] 创建 `src/skills/mod.rs` 模块
- [ ] 实现 `SkillType` 枚举 (`BuiltinMcp`, `ExternalMcp`, `PromptTemplate`)
- [ ] 实现 `UnifiedSkillConfig` 结构及序列化
- [ ] 实现 `SkillPermissions` 结构
- [ ] 实现 `SkillStatus` 枚举及状态管理

### 1.3 配置迁移
- [ ] 实现旧配置检测逻辑 (`[mcp]` + `[skills]`)
- [ ] 实现配置迁移函数 `migrate_legacy_config()`
- [ ] 编写迁移单元测试
- [ ] 确保迁移后旧配置备份

### 1.4 UniFFI 接口更新
- [ ] 添加 `list_skills() -> Vec<UnifiedSkillConfig>`
- [ ] 添加 `get_skill(id: String) -> Option<UnifiedSkillConfig>`
- [ ] 添加 `update_skill(config: UnifiedSkillConfig)`
- [ ] 添加 `delete_skill(id: String)`
- [ ] 添加 `add_skill(config: UnifiedSkillConfig)`
- [ ] 添加 `get_skill_status(id: String) -> SkillStatus`
- [ ] 添加 `get_skill_logs(id: String, max_lines: u32) -> Vec<String>`
- [ ] 保留旧接口作为兼容层 (deprecated)

### 1.5 生成 Swift 绑定
- [ ] 运行 `uniffi-bindgen generate`
- [ ] 验证生成的 `aether.swift` 包含新类型
- [ ] 编译验证无错误

---

## Phase 2: Swift 组件库

### 2.1 创建组件目录结构
- [x] 创建 `Aether/Sources/Components/Skills/` 目录
- [x] 创建组件索引文件

### 2.2 UnifiedSkillCard 组件 (renamed from SkillCard to avoid conflict)
- [x] 定义 `UnifiedSkillCard` 视图结构
- [x] 实现图标 + 名称 + 状态指示器布局
- [x] 实现 hover 效果
- [x] 实现 Toggle 开关绑定
- [x] 实现更多操作按钮 (context menu)
- [x] 添加 Preview
- [x] 添加 `SkillListRow` 紧凑列表行变体

### 2.3 SkillFilterSidebar 组件
- [x] 定义 `SkillFilterSidebar` 视图结构
- [x] 实现状态筛选 (全部/已启用/已停用/错误)
- [x] 实现类型筛选 (内置核心/外部扩展/提示模板)
- [x] 实现 "添加" 按钮
- [x] 实现 "JSON 模式" 按钮
- [x] 添加 Preview
- [x] 添加 `SkillCounts` 统计结构

### 2.4 SkillEnvVarEditor 组件
- [x] 定义 `SkillEnvVarEditor` 视图结构
- [x] 实现 Key-Value 行编辑
- [x] 实现 SecureField + 眼睛按钮切换
- [x] 实现添加/删除行
- [x] 实现空状态提示
- [x] 添加 Preview

### 2.5 SkillArgsEditor 组件
- [x] 定义 `SkillArgsEditor` 视图结构
- [x] 实现动态参数列表
- [x] 实现重排序 (上下移动按钮)
- [x] 实现添加/删除参数
- [x] 添加 Preview
- [x] 实现命令路径浏览器
- [x] 实现工作目录选择

### 2.6 SkillPermissionsEditor 组件
- [x] 定义 `SkillPermissionsEditor` 视图结构
- [x] 实现 "需要确认" Toggle
- [x] 实现 Allowed Paths 编辑 (文件夹选择)
- [x] 实现 Allowed Commands 编辑 (shell 服务)
- [x] 添加 Preview
- [x] 实现自动批准警告提示

### 2.7 SkillStatusIndicator 组件
- [x] 定义 `SkillStatusIndicator` 视图结构
- [x] 实现状态颜色映射 (Running=绿, Stopped=灰, Error=红, Starting=黄)
- [x] 实现状态文本本地化
- [x] 添加 Preview
- [x] 添加 `SkillStatusBadge` 胶囊样式变体

---

## Phase 3: 主视图集成

### 3.1 SkillInspectorPanel 组件
- [ ] 定义 `SkillInspectorPanel` 视图结构
- [ ] 实现 Header 区域 (图标/名称/状态/Toggle)
- [ ] 实现 Connection 区域 (仅外部 MCP 显示)
  - [ ] Transport 选择器
  - [ ] Command 输入 + Browse
  - [ ] Args 编辑器集成
  - [ ] Working Directory 输入
- [ ] 实现 Environment Variables 区域
- [ ] 实现 Permissions 区域
- [ ] 实现 Tools 只读列表 (从服务获取)
- [ ] 实现 Action Bar (View Logs / Cancel / Save)
- [ ] 实现滑入/滑出动画

### 3.2 SkillsSettingsView 主视图
- [ ] 定义 `SkillsSettingsView` 视图结构
- [ ] 实现 HSplitView 布局 (Sidebar + Content)
- [ ] 集成 `SkillFilterSidebar`
- [ ] 实现 Skill 列表 (LazyVStack)
- [ ] 集成 `SkillCard` 组件
- [ ] 实现选中状态管理
- [ ] 实现 Inspector Panel 显示/隐藏

### 3.3 SkillAddSheet 组件
- [ ] 定义 `SkillAddSheet` 视图结构
- [ ] 实现类型选择 (内置预设 / 外部 MCP / 导入 Skill)
- [ ] 实现外部 MCP 表单
- [ ] 实现 URL 导入表单
- [ ] 实现 ZIP 导入表单

### 3.4 SkillLogsSheet 组件
- [ ] 定义 `SkillLogsSheet` 视图结构
- [ ] 实现日志实时刷新
- [ ] 实现日志搜索/过滤
- [ ] 实现日志导出

### 3.5 SkillJsonEditor 组件
- [ ] 定义 `SkillJsonEditor` 视图结构
- [ ] 实现 JSON 语法高亮 (可选)
- [ ] 实现保存前验证
- [ ] 实现导入/导出按钮

### 3.6 删除旧视图
- [ ] 删除 `Aether/Sources/McpSettingsView.swift`
- [ ] 删除 `Aether/Sources/SkillsSettingsView.swift` (原版)
- [ ] 更新 `SettingsView.swift` 移除旧 Tab

### 3.7 更新导航
- [ ] 更新 `RootContentView.swift` 侧边栏
- [ ] 合并 MCP + Skills Tab 为单一 "Skills" Tab
- [ ] 更新 Tab 图标和标签

---

## Phase 4: 高级功能

### 4.1 JSON 模式
- [ ] 实现 GUI ↔ JSON 双向切换
- [ ] 实现 JSON 编辑保存
- [ ] 实现 JSON 语法错误提示

### 4.2 配置导入/导出
- [ ] 实现 `claude_desktop_config.json` 格式导入
- [ ] 实现 `claude_desktop_config.json` 格式导出
- [ ] 实现批量导入 UI

### 4.3 自动发现 (可选)
- [ ] 扫描全局 npm 包 (`mcp-*`)
- [ ] 扫描全局 pip 包 (`mcp-*`)
- [ ] 显示发现的 MCP Server 建议

### 4.4 调试控制台 (可选)
- [ ] 实现 JSON-RPC Trace Window
- [ ] 显示请求/响应实时日志
- [ ] 实现日志过滤

---

## Phase 5: 本地化与测试

### 5.1 本地化
- [ ] 添加英文字符串 (`en.lproj/Localizable.strings`)
- [ ] 添加中文字符串 (`zh-Hans.lproj/Localizable.strings`)
- [ ] 验证所有字符串已本地化

### 5.2 单元测试 (Rust)
- [ ] 配置迁移测试
- [ ] `UnifiedSkillConfig` 序列化测试
- [ ] 状态管理测试

### 5.3 UI 测试 (Swift)
- [ ] 组件 Preview 测试
- [ ] 交互流程测试

### 5.4 文档更新
- [ ] 更新 `docs/ComponentsIndex.md`
- [ ] 更新 `CLAUDE.md` 相关章节
- [ ] 更新用户文档

---

## Dependencies

### 前置依赖
- `implement-mcp-capability` Phase 0-1 (共享基础模块)
- `add-skills-capability` Phase 0-8 (Skills 基础功能)

### 并行开发
- 本提案 Phase 2 (组件库) 可与其他提案并行开发
- 本提案 Phase 1 需等待前置依赖完成

---

## Validation Criteria

### Phase 1 完成标准
- [ ] 新旧配置格式可互相转换
- [ ] UniFFI 绑定编译通过
- [ ] 旧 API 调用兼容

### Phase 3 完成标准
- [ ] 统一视图可显示所有 Skill 类型
- [ ] 可启用/禁用任意 Skill
- [ ] 可编辑外部 MCP 配置
- [ ] 可查看日志

### Phase 5 完成标准
- [ ] 所有字符串已本地化
- [ ] 无运行时警告/错误
- [ ] 组件 Preview 正常显示
