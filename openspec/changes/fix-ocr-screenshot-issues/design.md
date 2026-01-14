# Design: Fix OCR Screenshot Issues

## 1. 问题根因分析

### 1.1 闪退问题 (EXC_BAD_ACCESS)

#### 崩溃堆栈分析

典型的崩溃发生在 `objc_release`，调用链：

```
objc_release
NSWindow dealloc
ScreenCaptureOverlayView dealloc (此时 AppKit 内部还有引用)
autorelease pool drain
```

#### 根本原因

AppKit 的事件分发机制在 `mouseUp:` 事件处理期间会持有视图引用。当我们在事件处理器的回调中销毁视图/窗口时：

1. `mouseUp:` 触发 `onComplete` 回调
2. 回调执行 `dismissOverlay()`
3. `dismissOverlay()` 设置 `contentView = nil` 或调用 `close()`
4. 视图被 ARC 释放
5. 但 AppKit 的事件分发代码仍持有 autoreleased 引用
6. 当 runloop 结束时，autorelease pool 尝试释放已销毁的对象
7. **CRASH: EXC_BAD_ACCESS**

#### 时序图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Current (Broken) Flow                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  User Release Mouse                                                         │
│         │                                                                   │
│         ▼                                                                   │
│  ┌─────────────────┐                                                        │
│  │  NSEvent        │                                                        │
│  │  mouseUp:       │─────autorelease──────┐                                 │
│  └────────┬────────┘                      │                                 │
│           │                               ▼                                 │
│           │                        ┌──────────────┐                         │
│           ▼                        │ Autorelease  │                         │
│  ┌─────────────────┐               │    Pool      │                         │
│  │ OverlayView     │               └──────────────┘                         │
│  │ mouseUp()       │                      │                                 │
│  └────────┬────────┘                      │                                 │
│           │                               │                                 │
│           ▼                               │                                 │
│  ┌─────────────────┐                      │                                 │
│  │ onComplete()    │                      │                                 │
│  │  callback       │                      │                                 │
│  └────────┬────────┘                      │                                 │
│           │                               │                                 │
│           ▼                               │                                 │
│  ┌─────────────────┐                      │                                 │
│  │ dismissOverlay()│                      │                                 │
│  │ close() / nil   │◀─── View FREED ──────┼─── 💥 CRASH                     │
│  └────────┬────────┘                      │     when pool                   │
│           │                               │     drains!                     │
│           ▼                               ▼                                 │
│      Return to                       Pool Drain                             │
│      Event Loop                                                             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 AI 响应丢失问题

#### 可能的原因分析

通过代码审查，识别出以下可能的故障点：

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         OCR Request Flow                                     │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────┐                                                         │
│  │ captureRegion() │                                                         │
│  │ SCScreenshot    │                                                         │
│  └────────┬────────┘                                                         │
│           │ CGImage                                                          │
│           ▼                                                                  │
│  ┌─────────────────┐                                                         │
│  │ processCapture()│                                                         │
│  │ → PNG Data      │ ◀─── ❓ PNG conversion failure?                         │
│  └────────┬────────┘                                                         │
│           │ Data                                                             │
│           ▼                                                                  │
│  ┌─────────────────────┐                                                     │
│  │ extractTextFromImage│                                                     │
│  │ async throws        │ ◀─── ❓ core == nil?                                │
│  └────────┬────────────┘                                                     │
│           │ [UInt8]                                                          │
│           ▼                                                                  │
│  ┌─────────────────────┐                                                     │
│  │ core.extractText()  │                                                     │
│  │ UniFFI bridge       │ ◀─── ❓ UniFFI marshaling error?                    │
│  └────────┬────────────┘                                                     │
│           │                                                                  │
│           ▼ Rust                                                             │
│  ┌─────────────────────┐                                                     │
│  │ VisionService       │                                                     │
│  │ .extract_text()     │                                                     │
│  └────────┬────────────┘                                                     │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────┐                                                     │
│  │ get_vision_provider │ ◀─── ⚠️ default_provider not set?                   │
│  │                     │ ◀─── ⚠️ provider not vision-capable?                │
│  └────────┬────────────┘                                                     │
│           │                                                                  │
│           ▼                                                                  │
│  ┌─────────────────────┐                                                     │
│  │ provider            │                                                     │
│  │ .process_with_image │ ◀─── ⚠️ API error (auth, rate limit)?               │
│  └────────┬────────────┘                                                     │
│           │ Response                                                         │
│           ▼                                                                  │
│  ┌─────────────────────┐                                                     │
│  │ Clipboard Write     │ ◀─── ❓ Empty result check?                         │
│  │ NSPasteboard        │                                                     │
│  └─────────────────────┘                                                     │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

**最可能的原因**（按概率排序）：

1. **Provider 配置问题** - `default_provider` 未设置或 provider 不支持 vision
2. **API 认证/网络错误** - 但错误被静默处理
3. **异步任务未正确完成** - Task 未等待完成

## 2. 解决方案设计

### 2.1 窗口生命周期：窗口重用模式

#### 核心理念

**永不销毁，只做重置**

```swift
final class ScreenCaptureCoordinator {
    // Singleton pattern for window/view
    private var _overlayWindow: NSWindow?
    private var _overlayView: ScreenCaptureOverlayView?

    // Lazy initialization, created once, reused forever
    private var overlayWindow: NSWindow {
        if let existing = _overlayWindow {
            return existing
        }
        let window = createOverlayWindow()
        _overlayWindow = window
        return window
    }

    private var overlayView: ScreenCaptureOverlayView {
        if let existing = _overlayView {
            return existing
        }
        let view = createOverlayView()
        _overlayView = view
        return view
    }
}
```

#### 状态机

```
┌─────────────────────────────────────────────────────────────────┐
│                    Window Lifecycle States                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│                          ┌────────────┐                          │
│                          │   HIDDEN   │ ◀──────────────────┐     │
│                          │ (orderOut) │                    │     │
│                          └─────┬──────┘                    │     │
│                                │                           │     │
│                     startCapture()                  dismissOverlay()
│                     reset() + show                         │     │
│                                │                           │     │
│                                ▼                           │     │
│                          ┌────────────┐                    │     │
│                          │  VISIBLE   │                    │     │
│                          │ (orderFront)                    │     │
│                          └─────┬──────┘                    │     │
│                                │                           │     │
│                       User selects region                  │     │
│                                │                           │     │
│                                ▼                           │     │
│                          ┌────────────┐                    │     │
│                          │ COMPLETING │────────────────────┘     │
│                          │ (callback) │                          │
│                          └────────────┘                          │
│                                                                  │
│  Window/View NEVER destroyed during app lifecycle                │
│  Only created ONCE, reused for ALL captures                      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

#### ScreenCaptureOverlayView.reset() 设计

```swift
/// Reset view state for reuse
/// Call this BEFORE showing the window for a new capture
func reset() {
    // 1. Clear selection state
    selectionRect = nil
    startPoint = nil
    isDragging = false

    // 2. Reset dismissed flag (important!)
    isDismissed = false

    // 3. Recreate tracking area (safe since window is hidden)
    if let existingArea = trackingArea {
        removeTrackingArea(existingArea)
    }
    let newArea = NSTrackingArea(
        rect: bounds,
        options: [.activeAlways, .mouseMoved, .mouseEnteredAndExited],
        owner: self,
        userInfo: nil
    )
    addTrackingArea(newArea)
    trackingArea = newArea

    // 4. Request redraw
    setNeedsDisplay(bounds)
}
```

### 2.2 错误处理增强

#### Swift 层日志

```swift
private func extractTextFromImage(pngData: Data) async throws -> String {
    let logPrefix = "[OCR]"

    debugLog("\(logPrefix) START: dataSize=\(pngData.count)")

    guard let appDelegate = NSApplication.shared.delegate as? AppDelegate else {
        debugLog("\(logPrefix) ERROR: AppDelegate not found")
        throw OCRError.notInitialized("AppDelegate not found")
    }

    guard let core = appDelegate.core else {
        debugLog("\(logPrefix) ERROR: AetherCore not initialized")
        throw OCRError.notInitialized("AetherCore not initialized")
    }

    do {
        debugLog("\(logPrefix) Calling core.extractText...")
        let startTime = CFAbsoluteTimeGetCurrent()

        let imageBytes = Array(pngData)
        let result = try await core.extractText(imageData: imageBytes)

        let elapsed = CFAbsoluteTimeGetCurrent() - startTime
        debugLog("\(logPrefix) SUCCESS: \(result.count) chars in \(elapsed)s")

        return result
    } catch {
        debugLog("\(logPrefix) EXCEPTION: \(error)")
        throw error
    }
}
```

#### Rust 层日志 (`vision_ops.rs`)

```rust
pub async fn extract_text(&self, image_data: Vec<u8>) -> Result<String> {
    tracing::info!(
        image_size = image_data.len(),
        "Starting OCR text extraction"
    );

    // Check config
    let config = {
        let guard = self.config.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };

    // Log provider check
    let default_provider = config.general.default_provider.as_ref();
    tracing::info!(
        default_provider = ?default_provider,
        "Config loaded"
    );

    if default_provider.is_none() {
        tracing::error!("No default provider configured");
        return Err(AetherError::invalid_config(
            "No default provider configured for OCR"
        ));
    }

    let vision_service = VisionService::with_defaults();

    match self.runtime.spawn(async move {
        vision_service.extract_text(image_data, &config).await
    }).await {
        Ok(Ok(text)) => {
            tracing::info!(result_length = text.len(), "OCR completed successfully");
            Ok(text)
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "OCR failed");
            Err(e)
        }
        Err(e) => {
            tracing::error!(error = %e, "Task join error");
            Err(AetherError::other(format!("Task join error: {}", e)))
        }
    }
}
```

### 2.3 Provider Vision 检查

```rust
// In VisionService::get_vision_provider()
fn get_vision_provider(&self, config: &Config) -> Result<Arc<dyn AiProvider>> {
    let provider_name = config.general.default_provider.as_ref()
        .ok_or_else(|| AetherError::invalid_config(
            "No default provider configured. Set [general] default_provider in config.toml"
        ))?;

    tracing::info!(provider = %provider_name, "Creating vision provider");

    let provider_config = config.providers.get(provider_name)
        .ok_or_else(|| AetherError::invalid_config(format!(
            "Provider '{}' not found in [providers] section",
            provider_name
        )))?;

    let provider = create_provider(provider_name, provider_config.clone())?;

    // Explicit vision capability check
    if !provider.supports_vision() {
        tracing::error!(
            provider = %provider_name,
            "Provider does not support vision"
        );
        return Err(AetherError::invalid_config(format!(
            "Provider '{}' does not support vision/image input. \
             Use a vision-capable provider: claude, openai (gpt-4o), gemini",
            provider_name
        )));
    }

    tracing::info!(
        provider = %provider_name,
        supports_vision = true,
        "Vision provider ready"
    );

    Ok(provider)
}
```

## 3. 内存管理对比

### 当前方案 vs 新方案

```
┌───────────────────────────────────────────────────────────────────┐
│                     Current: Accumulating Leak                    │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Capture 1:  [Window1] [View1]  → retained                        │
│  Capture 2:  [Window2] [View2]  → retained                        │
│  Capture 3:  [Window3] [View3]  → retained                        │
│  ...                                                              │
│  Capture N:  [WindowN] [ViewN]  → retained                        │
│                                                                   │
│  Memory: O(n) - grows with each capture                           │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────────┐
│                      New: Single Reusable                         │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Capture 1:  [Window] [View]  → reset + show                      │
│  Capture 2:  [Window] [View]  → reset + show (same objects)       │
│  Capture 3:  [Window] [View]  → reset + show (same objects)       │
│  ...                                                              │
│  Capture N:  [Window] [View]  → reset + show (same objects)       │
│                                                                   │
│  Memory: O(1) - constant, only one window/view pair               │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

## 4. 测试策略

### 4.1 闪退回归测试

```swift
// Pseudo-test: Stress test for crash regression
func testRapidCaptureNoCrash() async {
    for i in 1...100 {
        // Simulate hotkey
        ScreenCaptureCoordinator.shared.startCapture(mode: .region)

        // Wait for overlay to appear
        try await Task.sleep(nanoseconds: 100_000_000) // 100ms

        // Simulate selection complete
        ScreenCaptureCoordinator.shared.simulateSelectionComplete(
            rect: CGRect(x: 100, y: 100, width: 200, height: 200)
        )

        // Brief pause
        try await Task.sleep(nanoseconds: 50_000_000) // 50ms
    }

    // If we reach here, no crash occurred
    XCTAssert(true, "Completed 100 captures without crash")
}
```

### 4.2 内存泄漏测试

使用 Instruments:
1. 选择 "Allocations" 模板
2. 执行 10 次截图
3. 标记 Generation
4. 再执行 10 次截图
5. 比较两代 NSWindow/NSView 实例数
6. **预期结果**: 实例数保持不变（应该只有 1 个）

### 4.3 AI 响应端到端测试

```swift
func testOCREndToEnd() async throws {
    // Clear clipboard
    NSPasteboard.general.clearContents()

    // Capture a region with known text
    let testImage = createTestImageWithText("Hello OCR Test")

    // Process through vision pipeline
    let result = try await ScreenCaptureCoordinator.shared.processTestImage(testImage)

    // Verify result
    XCTAssertTrue(result.contains("Hello"))
    XCTAssertTrue(result.contains("OCR"))
    XCTAssertTrue(result.contains("Test"))

    // Verify clipboard
    let clipboardContent = NSPasteboard.general.string(forType: .string)
    XCTAssertEqual(clipboardContent, result)
}
```

## 5. 回滚计划

如果新方案出现问题：

1. **保留旧代码**：在 `#if DEBUG` 块中保留当前的 `retainedViews/retainedWindows` 逻辑
2. **Feature Flag**：添加 `Config.experimental.reuseOverlayWindow` 开关
3. **分阶段发布**：先在 Debug 版本验证，再合并到 Release
