# Spec: OCR Error Handling

## Capability Overview

确保 OCR 请求链路中的错误被正确处理、记录和反馈给用户。

## ADDED Requirements

### Requirement: OCR-ERR-001 - Provider Vision Check

系统 MUST 在发起 OCR 请求前验证 AI provider 支持 vision 能力。

#### Scenario: Provider supports vision

**Given** 用户配置的 `default_provider` 是 "claude" 或 "openai"（使用 gpt-4o）
**When** VisionService 初始化 provider
**Then** `provider.supports_vision()` 返回 true
**And** OCR 请求正常继续

#### Scenario: Provider does not support vision

**Given** 用户配置的 `default_provider` 是 "ollama"（本地模型）
**And** 该模型不支持 vision
**When** VisionService 尝试创建 provider
**Then** 返回 `AlephError::InvalidConfig` 错误
**And** 错误消息包含 "does not support vision"
**And** 错误消息列出支持 vision 的 provider 选项

#### Scenario: No default provider configured

**Given** config.toml 中未设置 `[general] default_provider`
**When** 用户触发 OCR 截图
**Then** 返回 `AlephError::InvalidConfig` 错误
**And** 错误消息为 "No default provider configured"

### Requirement: OCR-ERR-002 - Error Logging

系统 MUST 在 OCR 请求链路的关键节点记录日志。

#### Scenario: Swift layer logging

**Given** 用户触发 OCR 截图
**When** `extractTextFromImage()` 被调用
**Then** 记录入口日志：data size
**And** 记录 core 调用前日志
**And** 成功时记录：字符数、耗时
**And** 失败时记录：具体错误信息

#### Scenario: Rust layer logging

**Given** Swift 层调用 `core.extractText()`
**When** Rust VisionService 处理请求
**Then** 使用 tracing 记录：image_size、provider_name、supports_vision
**And** 成功时记录：result_length
**And** 失败时记录：error 详情和类型

### Requirement: OCR-ERR-003 - User Feedback via HaloWindow

系统 MUST 通过 HaloWindow 向用户反馈 OCR 处理状态。

#### Scenario: Processing state shown

**Given** OCR 截图已捕获图像
**When** 开始调用 AI provider
**Then** HaloWindow 显示 processing 状态
**And** 显示本地化文本 "正在识别..."

#### Scenario: Success state shown

**Given** AI provider 返回 OCR 结果
**And** 结果不为空
**When** 结果写入剪贴板后
**Then** HaloWindow 显示 success toast
**And** toast 包含识别的字符数

#### Scenario: Error state shown

**Given** OCR 处理过程中发生错误
**When** 错误被捕获
**Then** HaloWindow 显示 error toast
**And** toast 显示具体错误信息
**And** toast 不自动消失（用户可以阅读完整错误）

#### Scenario: Permission error opens Settings

**Given** 用户未授予 Screen Recording 权限
**When** 用户触发 OCR 截图
**Then** HaloWindow 显示权限错误提示
**And** 自动打开系统设置的 Screen Recording 页面

### Requirement: OCR-ERR-004 - Clipboard Write Verification

系统 MUST 验证 OCR 结果正确写入剪贴板。

#### Scenario: Non-empty result written

**Given** AI provider 返回有效的 OCR 文本
**When** 处理完成
**Then** 剪贴板被清空（clearContents）
**And** OCR 文本写入剪贴板（setString:forType:.string）
**And** `lastResult` 属性更新为 OCR 文本

#### Scenario: Empty result handled

**Given** AI provider 返回空字符串或纯空白字符
**When** 检查 `trimmedResult`
**Then** 显示 "未识别到文本" 错误
**And** 不修改剪贴板内容
**And** `lastError` 属性更新为错误信息

---

## Cross-References

- `claude-provider` - Claude API vision 支持
- `openai-provider` - OpenAI API vision 支持
- `event-handler` - HaloWindow 状态更新
- `permission-gating` - Screen Recording 权限检查
