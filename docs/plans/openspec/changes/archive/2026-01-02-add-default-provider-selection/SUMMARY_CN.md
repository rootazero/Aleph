# OpenSpec 提案摘要 - add-default-provider-selection

## 提案概述

为 Aleph 添加默认供应商选择功能,并支持通过系统菜单栏快速切换默认供应商。

## 核心功能

### 1. 设置 UI 增强
- ✅ 在 ProvidersView 中为默认供应商显示"Default"徽章
- ✅ 通过编辑面板中的"Set as Default"按钮设置默认供应商
- ✅ 只有已激活的供应商才能设为默认

### 2. 菜单栏快速切换
- ✅ 菜单栏只显示已激活的供应商
- ✅ 当前默认供应商前显示对勾(✓)
- ✅ 点击菜单中的供应商名称即可切换默认
- ✅ 立即更新配置和路由系统

### 3. 配置集成
- ✅ `general.default_provider` 与 UI 选择保持同步
- ✅ 验证默认供应商存在且已启用
- ✅ 优雅处理边缘情况(默认供应商被禁用、删除等)

## 技术架构

```
用户操作 (Settings UI / Menu Bar)
    ↓
Swift UI (ProvidersView / AppDelegate)
    ├─ Edit Panel: "Set as Default" Button
    └─ Menu Bar: Click Provider Name
    ↓ UniFFI
Rust Core (AlephCore)
    ├─ get_default_provider()
    ├─ set_default_provider(name)
    └─ get_enabled_providers()
    ↓
Config Layer (验证 + 原子保存)
    ↓
Router Layer (使用默认供应商 + 降级处理)
```

## 实现任务 (8 个阶段, 29 个任务)

### Phase 1: 创建新规范
- 创建 `default-provider-management` 规范(新能力)
- 定义 7 个核心需求,每个需求包含 2-3 个场景

### Phase 2: 更新现有规范
- 修改 `ai-routing` 规范(验证 + 降级逻辑)
- 修改 `settings-ui-layout` 规范(UI 指示器)
- 修改 `provider-active-state` 规范(活跃状态影响)

### Phase 3: Rust 核心实现
- Config 验证逻辑增强
- Router 降级处理
- UniFFI 桥接方法

### Phase 4: Swift UI - Settings
- ProvidersView 状态管理
- SimpleProviderCard 默认徽章
- ProviderEditPanel 集成("Set as Default"按钮)

### Phase 5: Swift UI - Menu Bar
- 动态供应商菜单
- 只显示已激活供应商
- 对勾(✓)指示器
- 快速切换功能

### Phase 6-8: 测试 + 文档 + 验证

## 设计决策

### 1. 验证策略: 宽容模式
- ✅ 允许配置中的默认供应商被禁用(使用降级)
- ✅ 显示警告提示用户修复
- ❌ 不阻止应用启动(更好的 UX)

### 2. 设置禁用供应商为默认: 显示错误
- ✅ 防止设置禁用供应商为默认
- ✅ 显示错误提示:"请先启用该供应商"
- ❌ 不自动启用(明确性原则)

### 3. 菜单栏更新: 观察者模式
- ✅ 配置变更时立即重建菜单
- ✅ 使用 NotificationCenter
- ❌ 不使用轮询(效率更高)

### 4. 徽章位置: 右上角
- ✅ 与现有"Active"指示器一致
- ✅ 紧凑布局,适合 240px 宽侧边栏
- ✅ 不干扰供应商名称点击区域

## 验证状态

```bash
✅ openspec validate add-default-provider-selection --strict
   Change 'add-default-provider-selection' is valid
```

## 文件清单

```
openspec/changes/add-default-provider-selection/
├── proposal.md          # 提案文档(问题陈述 + 解决方案)
├── tasks.md             # 实现任务清单(30 tasks, 8 phases)
├── design.md            # 设计文档(架构 + 权衡 + 代码示例)
└── specs/
    ├── default-provider-management/
    │   └── spec.md      # 新规范: 7 requirements, 24 scenarios
    ├── ai-routing/
    │   └── spec.md      # MODIFIED: 2 requirements, 5 scenarios
    ├── settings-ui-layout/
    │   └── spec.md      # MODIFIED + ADDED: 3 requirements, 9 scenarios
    └── provider-active-state/
        └── spec.md      # MODIFIED + ADDED: 3 requirements, 6 scenarios
```

## 下一步

等待用户批准后开始实现。建议按以下顺序:
1. Phase 3: Rust 核心(UniFFI API + Config 验证)
2. Phase 4: Settings UI(徽章 + 编辑面板按钮)
3. Phase 5: Menu Bar(动态菜单 + 快速切换)
4. Phase 6-8: 测试 + 文档

预计总工作量: 2-3 天(包括测试和文档)
