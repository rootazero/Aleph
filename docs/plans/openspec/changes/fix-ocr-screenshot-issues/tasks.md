# Tasks: Fix OCR Screenshot Issues

## Phase 1: 诊断 AI 响应问题（优先级最高）

在修改窗口生命周期之前，先诊断 AI 响应丢失的具体原因。

- [ ] **1.1** 在 `extractTextFromImage()` 添加详细日志，记录：
  - 入口时的 pngData 大小
  - core.extractText() 调用前后
  - 返回结果或异常详情
  - 依赖：无

- [ ] **1.2** 在 Rust `VisionService.extract_text()` 添加 tracing 日志：
  - 检查 config 加载是否成功
  - 验证 default_provider 是否配置
  - 验证 provider 是否支持 vision
  - 依赖：无

- [ ] **1.3** 手动测试并收集日志：
  - 执行一次 OCR 截图
  - 检查 `~/Desktop/aleph_debug.log`
  - 检查 Console.app 中的 NSLog 输出
  - 验证：确定问题是 provider 配置、网络、还是代码逻辑
  - 依赖：1.1, 1.2

- [ ] **1.4** 根据诊断结果修复 AI 响应问题：
  - 如果是 provider 配置问题：改进错误提示
  - 如果是代码逻辑问题：修复具体 bug
  - 依赖：1.3

## Phase 2: 窗口生命周期重构

- [ ] **2.1** 在 `ScreenCaptureOverlayView` 添加 `reset()` 方法：
  - 重置 `selectionRect` 为 nil
  - 重置 `startPoint` 为 nil
  - 重置 `isDragging` 为 false
  - 移除并重新添加 tracking area
  - 调用 `setNeedsDisplay(bounds)` 刷新视图
  - 依赖：无

- [ ] **2.2** 重构 `ScreenCaptureCoordinator` 采用窗口重用模式：
  - 将 `overlayWindow` 和 `overlayView` 改为 lazy 单例
  - 修改 `showRegionSelector()` 调用 `reset()` 而非创建新实例
  - 修改 `dismissOverlay()` 只调用 `orderOut()` 和清除回调
  - 删除 `retainedViews` 和 `retainedWindows` 数组
  - 依赖：2.1

- [ ] **2.3** 验证窗口重用的内存行为：
  - 使用 Instruments 的 Allocations 工具
  - 执行 10 次连续截图
  - 确认无内存增长
  - 验证：无 NSWindow/NSView 泄漏
  - 依赖：2.2

- [ ] **2.4** 压力测试闪退修复：
  - 快速连续触发 50 次截图热键
  - 确认无 EXC_BAD_ACCESS
  - 依赖：2.2

## Phase 3: 恢复 HaloWindow 反馈

- [ ] **3.1** 恢复 `processCapture()` 中被禁用的 HaloWindow 调用：
  - 取消注释 `showHaloProcessing()`
  - 取消注释 `showHaloSuccess()`
  - 取消注释 `showHaloError()`
  - 依赖：2.4（确保闪退已修复）

- [ ] **3.2** 端到端测试完整流程：
  - Cmd+Option+O 触发截图
  - 验证 Halo 显示处理状态
  - 验证 OCR 完成后 Halo 显示成功/错误
  - 验证结果正确写入剪贴板（Cmd+V 粘贴验证）
  - 依赖：3.1

## Phase 4: 清理

- [ ] **4.1** 移除调试日志文件写入：
  - 删除或注释 `debugLog()` 函数中的文件写入逻辑
  - 保留 NSLog 用于 Console.app 调试
  - 依赖：3.2

- [ ] **4.2** 更新 PERMISSIONS.md 或相关文档（如有需要）
  - 依赖：3.2

## 验证检查清单

- [ ] 连续 100 次截图无闪退
- [ ] 内存使用稳定（无泄漏）
- [ ] AI OCR 响应正确返回
- [ ] 结果正确写入剪贴板
- [ ] HaloWindow 正确显示反馈状态
- [ ] 权限拒绝时显示正确的错误提示

## 依赖关系图

```
1.1 ─┬─→ 1.3 ─→ 1.4
1.2 ─┘

2.1 ─→ 2.2 ─→ 2.3
         │
         └─→ 2.4 ─→ 3.1 ─→ 3.2 ─→ 4.1
                               └─→ 4.2
```
