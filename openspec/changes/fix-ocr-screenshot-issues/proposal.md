# Proposal: Fix OCR Screenshot Issues

## Change ID
`fix-ocr-screenshot-issues`

## Summary

修复 OCR 截图功能的两个核心问题：

1. **闪退问题**：当前通过保留窗口和视图永不释放来避免 EXC_BAD_ACCESS 闪退，但这不是优雅的解决方案，会导致内存泄漏
2. **AI 响应丢失**：截图能够发送到 AI，但 AI 响应无法正确返回到剪贴板

## Background

### 问题 1：闪退的根本原因

当前的闪退发生在 `objc_release` 期间，原因是 AppKit 的事件分发机制：

1. 用户在 `ScreenCaptureOverlayView` 上完成选区并释放鼠标
2. `mouseUp()` 事件处理器调用 `onComplete?()` 回调
3. 回调触发 `dismissOverlay()` 来关闭窗口
4. 如果直接调用 `window.close()` 或设置 `window.contentView = nil`，AppKit 的内部数组仍然持有对视图的引用
5. 当事件循环完成时，autorelease pool 尝试释放视图，但视图已被销毁
6. 结果：EXC_BAD_ACCESS crash

**当前 workaround**（`ScreenCaptureCoordinator.swift:470-475`）:
```swift
// 永不关闭窗口，只是隐藏并保留在数组中
if let view = overlayView {
    retainedViews.append(view)
}
if let window = overlayWindow {
    retainedWindows.append(window)
}
```

这会导致每次截图都积累一个未释放的窗口/视图对。

### 问题 2：AI 响应丢失的根本原因

经过代码分析，发现问题不在于剪贴板写入逻辑（`ScreenCaptureCoordinator.swift:340-342`），而可能是：

1. **VisionService 调用失败但错误被静默处理**
2. **Provider 配置问题**：默认 provider 未配置或不支持 vision
3. **异步任务未正确等待**

当前流程：
```
captureRegion() → processCapture() → extractTextFromImage() → core.extractText() → VisionService → AI Provider
```

需要验证整个链路的错误处理和日志。

## Proposed Solution

### 解决方案 1：优雅的窗口生命周期管理

采用"窗口池"模式替代当前的"永不释放"策略：

#### 策略 A：延迟释放 + 安全 dispose（推荐）

核心思想：在事件循环完成后安全地释放资源

```swift
private func dismissOverlay() {
    guard !isDismissing else { return }
    isDismissing = true

    // Step 1: 停止所有回调
    overlayView?.prepareForDismissal()

    // Step 2: 从屏幕移除
    overlayWindow?.orderOut(nil)

    // Step 3: 保持局部引用以延长生命周期
    let viewToRelease = overlayView
    let windowToRelease = overlayWindow

    // Step 4: 清除协调器引用
    overlayView = nil
    overlayWindow = nil

    // Step 5: 延迟释放 - 在多个 runloop 周期后安全释放
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
        // 此时事件循环已完全完成
        windowToRelease?.contentView = nil
        // 让 ARC 自然释放
        _ = viewToRelease
        _ = windowToRelease
        self?.isDismissing = false
    }
}
```

#### 策略 B：窗口重用池

核心思想：创建一个窗口和视图，在整个会话中重用，只做 reset 而不销毁

```swift
final class ScreenCaptureCoordinator {
    // 单例窗口，整个 app 生命周期只创建一次
    private lazy var sharedWindow: NSWindow = createOverlayWindow()
    private lazy var sharedView: ScreenCaptureOverlayView = createOverlayView()

    private func showRegionSelector() {
        // 重置视图状态而不是创建新的
        sharedView.reset()
        sharedView.onComplete = { [weak self] rect in
            self?.captureRegion(rect)
        }
        sharedView.onCancel = { [weak self] in
            self?.cancelCapture()
        }

        sharedWindow.contentView = sharedView
        sharedWindow.makeKeyAndOrderFront(nil)
    }

    private func dismissOverlay() {
        // 只是隐藏，不销毁
        sharedWindow.orderOut(nil)
        // 清除回调防止意外触发
        sharedView.onComplete = nil
        sharedView.onCancel = nil
    }
}
```

### 解决方案 2：AI 响应链路修复

#### 2.1 增强错误处理和日志

在 `extractTextFromImage()` 中添加详细日志：

```swift
private func extractTextFromImage(pngData: Data) async throws -> String {
    debugLog("extractTextFromImage: starting, data size = \(pngData.count) bytes")

    guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
          let core = appDelegate.core else {
        let error = "AlephCore not initialized"
        debugLog("extractTextFromImage: FAILED - \(error)")
        throw NSError(domain: "ScreenCapture", code: -1,
                      userInfo: [NSLocalizedDescriptionKey: error])
    }

    do {
        let imageBytes = Array(pngData)
        debugLog("extractTextFromImage: calling core.extractText...")
        let result = try await core.extractText(imageData: imageBytes)
        debugLog("extractTextFromImage: SUCCESS - received \(result.count) chars")
        return result
    } catch {
        debugLog("extractTextFromImage: EXCEPTION - \(error)")
        throw error
    }
}
```

#### 2.2 验证 Provider Vision 支持

在 Rust `VisionService.get_vision_provider()` 中添加明确的 vision 支持检查：

```rust
fn get_vision_provider(&self, config: &Config) -> Result<Arc<dyn AiProvider>> {
    let provider = /* ... create provider ... */;

    if !provider.supports_vision() {
        return Err(AlephError::invalid_config(format!(
            "Provider '{}' does not support vision. Vision-capable providers: claude, openai (gpt-4o), gemini",
            provider.name()
        )));
    }

    Ok(provider)
}
```

#### 2.3 HaloWindow 反馈恢复

当前 HaloWindow 反馈被禁用用于调试。修复闪退后应恢复：

```swift
private func processCapture(_ cgImage: CGImage) {
    // ...
    showHaloProcessing()  // 恢复

    Task {
        do {
            let result = try await extractTextFromImage(pngData: pngData)
            // ...
            showHaloSuccess(characterCount: trimmedResult.count)  // 恢复
        } catch {
            showHaloError(message: error.localizedDescription)  // 恢复
        }
    }
}
```

## Impact Analysis

### 文件影响

| File | Changes |
|------|---------|
| `Aleph/Sources/Vision/ScreenCaptureCoordinator.swift` | 主要修改：窗口生命周期管理、错误处理增强 |
| `Aleph/Sources/Vision/ScreenCaptureOverlayView.swift` | 添加 `reset()` 方法用于视图重用 |
| `Aleph/core/src/vision/service.rs` | 增强 provider 检查和错误消息 |

### 风险评估

- **低风险**：错误处理增强、日志添加
- **中风险**：窗口生命周期重构（需要充分测试）
- **无破坏性变更**：不影响公开 API

## Alternatives Considered

### Alternative 1: 使用 NSWindowController

使用 `NSWindowController` 管理窗口生命周期，让 Cocoa 框架处理内存管理。

**缺点**：需要更大的重构，引入额外的复杂性

### Alternative 2: Objective-C 桥接

使用 `objc_setAssociatedObject` 来手动管理对象生命周期。

**缺点**：引入不安全的 ObjC 运行时操作，不推荐

### Alternative 3: 维持现状 + 定期清理

每 10 次截图后批量释放累积的窗口。

**缺点**：仍然是 hack，只是减少了内存泄漏

## Success Criteria

1. **闪退修复**：连续 100 次截图无 EXC_BAD_ACCESS
2. **内存稳定**：每次截图后内存使用恢复到基线
3. **AI 响应**：OCR 结果正确写入剪贴板
4. **用户反馈**：HaloWindow 正确显示处理/成功/错误状态

## Open Questions

1. **策略选择**：优先采用 A（延迟释放）还是 B（窗口重用）？
   - 推荐 B：更符合 Apple 的最佳实践，且完全避免创建/销毁的开销

2. **调试日志**：是否保留 Desktop debug log 文件，还是仅使用 NSLog？
   - 建议：Release 版本移除文件日志，保留 NSLog

3. **Provider 检测**：用户配置的默认 provider 不支持 vision 时，应该：
   - a) 报错退出
   - b) 自动选择支持 vision 的 provider（如果有）
   - 推荐 a：明确的错误优于意外的行为
