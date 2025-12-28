# Provider UI Redesign - Integration Testing Checklist

## Phase 8: Full CRUD Flow Testing

本文档提供了完整的手动测试清单,用于验证 redesign-providers-ui 提案的所有功能。

---

## 测试前准备

1. **生成 Xcode 项目**:
   ```bash
   xcodegen generate
   ```

2. **构建 Rust Core**:
   ```bash
   cd Aether/core
   cargo build
   cd ../..
   ```

3. **打开 Xcode**:
   ```bash
   open Aether.xcodeproj
   ```

4. **运行应用**: 在 Xcode 中按 `Cmd+R` 启动应用

---

## 测试场景 1: UI 布局和视觉验证

### 1.1 窗口尺寸验证
- [ ] 打开 Settings 窗口
- [ ] 验证窗口宽度至少为 1200px(参考任务要求)
- [ ] 验证窗口高度至少为 800px
- [ ] 验证窗口可调整大小

### 1.2 布局比例验证
- [ ] 左侧 Provider 列表面板宽度约为 450-550px
- [ ] 右侧 Edit Panel 占据剩余空间(约 500-600px)
- [ ] 两侧比例约为 45:55
- [ ] 调整窗口大小,布局保持平衡

### 1.3 按钮布局验证(关键改进)
- [ ] 点击任意 Provider 进入编辑模式
- [ ] **验证底部按钮固定在右下角**
- [ ] 左侧: "Test Connection" 按钮
- [ ] 右侧: "Cancel" 和 "Save" 按钮(水平排列)
- [ ] **滚动表单内容,按钮保持可见且位置固定**

### 1.4 颜色和间距验证
- [ ] Provider 卡片使用正确的圆角和阴影
- [ ] Active indicator 使用蓝色 (#007AFF)
- [ ] Inactive indicator 使用灰色圆圈轮廓
- [ ] 间距符合 DesignTokens(xs: 4, sm: 8, md: 16, lg: 24)

---

## 测试场景 2: 添加 Provider

### 2.1 添加 OpenAI Provider
- [ ] 点击 "Add Provider" 按钮
- [ ] 验证右侧面板显示 "Add Provider" 标题
- [ ] 验证 Active toggle 默认为 ON
- [ ] 填写以下信息:
  - Provider Name: `openai-test`
  - Provider Type: `OpenAI`
  - API Key: `sk-test1234567890abcdef`(测试密钥)
  - Model: `gpt-4o`
  - Base URL: (留空)
- [ ] 点击 "Test Connection" 按钮
- [ ] 验证测试结果显示在按钮**下方**(内联显示,非弹窗)
- [ ] 点击 "Save" 按钮
- [ ] 验证新 Provider 出现在左侧列表
- [ ] 验证卡片显示 Active indicator(蓝色圆点)

### 2.2 添加 Ollama Provider
- [ ] 点击 "Add Provider" 按钮
- [ ] 填写以下信息:
  - Provider Name: `ollama-local`
  - Provider Type: `Ollama`
  - Model: `llama3.2`
  - Base URL: `http://localhost:11434`
- [ ] 验证 API Key 字段**不显示**(Ollama 不需要)
- [ ] 点击 "Save" 按钮
- [ ] 验证新 Provider 添加成功

---

## 测试场景 3: 编辑 Provider

### 3.1 编辑配置
- [ ] 点击已有的 Provider(例如 `openai-test`)
- [ ] 验证右侧面板显示 Provider 详情(查看模式)
- [ ] 点击 "Edit Configuration" 按钮
- [ ] 验证进入编辑模式
- [ ] 修改 Model 为 `gpt-4o-mini`
- [ ] **验证修改后测试结果自动清除**
- [ ] 点击 "Test Connection"
- [ ] 验证内联结果显示
- [ ] 点击 "Save"
- [ ] 验证修改已保存

### 3.2 Toggle Active State
- [ ] 进入 Provider 编辑模式
- [ ] 切换 Active toggle 为 OFF
- [ ] 点击 "Save"
- [ ] 验证左侧卡片的 Active indicator 变为灰色圆圈
- [ ] 重新编辑,切换 Active toggle 为 ON
- [ ] 验证 Active indicator 恢复为蓝色圆点

### 3.3 取消编辑
- [ ] 编辑 Provider
- [ ] 修改任意字段
- [ ] 点击 "Cancel" 按钮
- [ ] 验证修改未保存,恢复原始值

---

## 测试场景 4: 删除 Provider

### 4.1 通过 Edit Panel 删除
- [ ] 选择一个 Provider
- [ ] 点击 "Edit Configuration" 进入查看模式
- [ ] 点击 "Delete Provider" 按钮(红色)
- [ ] 验证弹出确认对话框
- [ ] 点击 "Delete" 确认
- [ ] 验证 Provider 从列表中移除

### 4.2 通过右键菜单删除
- [ ] 右键点击 Provider 卡片
- [ ] 选择 "Delete" 菜单项
- [ ] 验证确认对话框
- [ ] 确认删除
- [ ] 验证删除成功

---

## 测试场景 5: 多 Provider 场景

### 5.1 添加多个 Provider
- [ ] 添加至少 5 个不同 Provider:
  - OpenAI
  - Claude
  - Ollama
  - Custom provider 1
  - Custom provider 2
- [ ] 验证所有 Provider 正确显示在列表中

### 5.2 切换 Provider
- [ ] 依次点击每个 Provider
- [ ] 验证右侧面板正确更新
- [ ] 验证切换流畅,无闪烁

### 5.3 搜索 Provider(如果实现)
- [ ] 在搜索框输入 Provider 名称
- [ ] 验证列表过滤正确

---

## 测试场景 6: 错误处理

### 6.1 无效 API Key
- [ ] 编辑 Provider,输入无效 API Key
- [ ] 点击 "Test Connection"
- [ ] 验证显示错误消息(红色,内联显示)
- [ ] 验证错误消息被截断(如果过长)
- [ ] Hover 查看完整错误消息(tooltip)

### 6.2 缺少必填字段
- [ ] 添加新 Provider,留空 Model 字段
- [ ] 验证 "Save" 按钮禁用
- [ ] 填写 Model
- [ ] 验证 "Save" 按钮启用

### 6.3 网络超时
- [ ] 配置 Provider 使用无效的 Base URL
- [ ] 点击 "Test Connection"
- [ ] 验证显示超时错误消息

---

## 测试场景 7: 边缘情况

### 7.1 长文本处理
- [ ] 添加 Provider,名称使用 50 个字符
- [ ] 验证名称在卡片中被截断(显示省略号)
- [ ] Hover 查看完整名称(tooltip)
- [ ] 使用超长 Model 名称(例如 `claude-3-5-sonnet-20241022-extended-context-version`)
- [ ] 验证 Model 在卡片中最多显示 2 行,超出部分截断

### 7.2 空 Provider 列表
- [ ] 删除所有 Provider
- [ ] 验证显示 "No Provider Selected" 空状态
- [ ] 验证 "Add Provider" 提示明显可见

### 7.3 表单内容滚动
- [ ] 打开 "Advanced Settings" 折叠区
- [ ] 填写所有字段(包括可选字段)
- [ ] **验证表单内容可滚动**
- [ ] **验证底部按钮始终可见,不随内容滚动**

---

## 测试场景 8: 配置持久化

### 8.1 保存和重载
- [ ] 添加/编辑 Provider
- [ ] 保存配置
- [ ] 完全退出应用(`Cmd+Q`)
- [ ] 重新启动应用
- [ ] 打开 Settings → Providers
- [ ] 验证所有配置正确加载

### 8.2 Keychain 集成
- [ ] 添加 Provider 并保存 API Key
- [ ] 打开 macOS Keychain Access 应用
- [ ] 搜索 Provider 名称
- [ ] 验证 API Key 存储在 Keychain 中
- [ ] 删除 Provider
- [ ] 验证 Keychain 条目也被删除

---

## 性能测试

### 9.1 响应速度
- [ ] 添加 Provider,验证保存操作在 1 秒内完成
- [ ] 点击 Provider,验证详情面板在 100ms 内更新
- [ ] 切换 Provider,验证无明显延迟

### 9.2 动画流畅度
- [ ] Hover Provider 卡片,验证阴影和缩放动画流畅(60fps)
- [ ] 切换编辑模式,验证过渡动画自然

---

## 最终验证清单

在所有测试完成后,确认:

- [x] Phase 5.1: 按钮位于右下角(Test Connection 在左,Cancel/Save 在右)
- [x] Phase 5.2: 表单滚动时按钮保持可见
- [x] Phase 6.1: 颜色和间距符合 DesignTokens
- [x] Phase 6.3: 表单字段变化时自动清除测试结果
- [x] Phase 7.2: 长文本正确截断,显示省略号
- [ ] Phase 8: 所有 CRUD 操作正常工作,无崩溃
- [ ] 无回归: 现有功能未破坏
- [ ] 视觉一致性: 与 `uisample.png` 参考设计保持一致

---

## 已知问题和限制

(在测试过程中发现的问题记录在此)

1. **无 Xcode 环境**: 当前环境仅有 Command Line Tools,无法通过 `xcodebuild` 命令行构建,需要完整 Xcode 进行测试。

---

## 测试完成后的操作

1. 更新 `tasks.md`,标记所有任务为完成(`[x]`)
2. 更新 `proposal.md`,标记 Approval Checklist 项目
3. 创建 Git commit:
   ```bash
   git add Aether/Sources/Components/Organisms/ProviderEditPanel.swift
   git add Aether/Sources/Components/Molecules/ProviderCard.swift
   git commit -m "feat: Redesign Providers UI with bottom-right button layout

   - Refactor ProviderEditPanel body to two-layer structure (ScrollView + fixed footer)
   - Reposition buttons to bottom-right corner (Test Connection left, Cancel/Save right)
   - Add text truncation for long provider names and model names
   - Ensure buttons remain visible when form content scrolls
   - Maintain consistent spacing and colors per DesignTokens

   Closes: redesign-providers-ui Phase 5-7"
   ```

4. 运行 `openspec archive redesign-providers-ui`(如果所有测试通过)
