# 性能优化指南

## 优化概览

| 优化项 | 技术 | 预期提升 | 实施Phase |
|-------|------|---------|----------|
| **初始加载** | LazyVStack虚拟滚动 | 10倍 | Phase 4 |
| **缩略图加载** | NSCache缓存 | 100倍（重复访问） | Phase 5 |
| **数据库查询** | 批量查询 | 40倍（100条消息） | Phase 5 |
| **滚动性能** | 虚拟滚动 | 2倍帧率（30fps→60fps） | Phase 4 |

---

## LazyVStack虚拟滚动

### 问题分析

**症状**:
- 初始加载100条消息耗时~5000ms
- 每条消息触发：
  - 正则表达式解析（ContentParser）
  - 数据库查询（loadStoredAttachments）
  - 异步图片加载（AsyncImage）
- 内存占用: O(n)所有视图

**根本原因**: VStack全量渲染，即使不可见的消息也会创建视图。

### 解决方案

使用`LazyVStack`仅渲染可见区域+缓冲（~10条）：

```swift
ScrollView {
    LazyVStack(spacing: 12, pinnedViews: []) {
        // Section 1: Active Parts（置顶，非Lazy）
        Section {
            ForEach(activeReasoningParts) { part in
                ReasoningPartView(part: part)
                    .id("reasoning-\(part.id)")
            }
            ForEach(activePlanParts) { part in
                PlanPartView(part: part)
                    .id("plan-\(part.id)")
            }
            ForEach(activeToolCalls) { toolCall in
                ToolCallPartView(part: toolCall)
                    .id("tool-\(toolCall.id)")
            }
        } header: {
            EmptyView()
        }

        // Section 2: Historical Messages（Lazy渲染）
        Section {
            ForEach(messages) { message in
                MessageBubbleView(message: message)
                    .id("message-\(message.id)")
            }
        } header: {
            EmptyView()
        }
    }
}
```

### 关键要点

#### 1. 稳定的`.id()`
```swift
// ✅ 正确：使用前缀避免ID冲突
.id("message-\(message.id)")
.id("reasoning-\(part.id)")
.id("tool-\(toolCall.id)")

// ❌ 错误：直接使用可能导致ID冲突
.id(message.id)
```

#### 2. Section分层
- **Section 1**: Active Parts - 始终可见，非Lazy渲染
- **Section 2**: Historical Messages - 虚拟滚动，按需渲染

#### 3. ScrollViewReader兼容
```swift
ScrollViewReader { proxy in
    LazyVStack { ... }
        .onChange(of: messages.count) {
            if let lastId = messages.last?.id {
                proxy.scrollTo("message-\(lastId)", anchor: .bottom)
            }
        }
}
```

### 效果验证

#### 性能测试
```swift
func testLazyVStackPerformance() {
    let viewModel = createViewModelWith100Messages()

    measure {
        let view = ConversationAreaView(viewModel: viewModel)
        _ = view.body
    }

    // 预期: < 100ms（VStack ~500ms）
}
```

#### 内存验证
- 使用Instruments Allocations工具
- 验证off-screen视图未创建
- 内存占用: O(可见数) vs O(n)

---

## NSCache缩略图缓存

### 问题分析

**症状**:
- 缩略图每次加载~100ms
- 滚动时重复加载同一图片
- I/O密集 + CPU解码

**根本原因**: 无缓存，每次都从磁盘读取 + 解码 + 生成缩略图。

### 解决方案

使用`NSCache`内存缓存缩略图：

```swift
// AttachmentFileManager.swift
private static let thumbnailCache: NSCache<NSString, NSImage> = {
    let cache = NSCache<NSString, NSImage>()
    cache.countLimit = 100              // 最多100个对象
    cache.totalCostLimit = 50 * 1024 * 1024  // 最多50MB内存
    cache.name = "com.aleph.thumbnailCache"
    return cache
}()

func getThumbnail(relativePath: String, maxSize: CGFloat = 64) -> NSImage? {
    let cacheKey = "\(relativePath)-\(Int(maxSize))"

    // 1. 检查缓存
    if let cached = Self.thumbnailCache.object(forKey: cacheKey as NSString) {
        return cached
    }

    // 2. 生成缩略图
    guard let thumbnail = generateThumbnail(relativePath, maxSize) else {
        return nil
    }

    // 3. 缓存（cost = 图片大小估算）
    let cost = Int(thumbnail.size.width * thumbnail.size.height * 4)  // RGBA
    Self.thumbnailCache.setObject(thumbnail, forKey: cacheKey as NSString, cost: cost)

    return thumbnail
}
```

### NSCache最佳实践

#### 1. 限制设置
```swift
cache.countLimit = 100              // 对象数量上限
cache.totalCostLimit = 50 * 1024 * 1024  // 内存上限（字节）
```

#### 2. Cost计算
```swift
// 图片: width * height * 4 (RGBA)
let cost = Int(image.size.width * image.size.height * 4)

// 数据: 字节数
let cost = data.count
```

#### 3. 缓存键设计
```swift
// 包含关键参数，避免错误命中
func cacheKey(path: String, size: CGFloat) -> String {
    "\(path)-\(Int(size))"
}
```

#### 4. 清理接口
```swift
// 调试/测试用
static func clearThumbnailCache() {
    thumbnailCache.removeAllObjects()
}
```

### 效果验证

```swift
func testThumbnailCacheHitRate() {
    let manager = AttachmentFileManager.shared
    let attachment = createMockAttachment()

    // 第一次加载（缓存未命中）
    let start1 = Date()
    _ = manager.getThumbnail(for: attachment)
    let time1 = Date().timeIntervalSince(start1)

    // 第二次加载（缓存命中）
    let start2 = Date()
    _ = manager.getThumbnail(for: attachment)
    let time2 = Date().timeIntervalSince(start2)

    // 验证缓存命中率
    XCTAssertLessThan(time2, time1 * 0.1, "Cache hit should be 10x faster")
}
```

---

## 批量查询优化

### 问题分析

**症状**:
- 100条消息查询附件耗时~2000ms
- 数据库日志显示大量重复查询
- `SELECT * FROM attachments WHERE messageId = ?` × 100

**根本原因**: N+1查询问题 - 每条消息独立查询数据库。

### 解决方案

使用批量查询（单次JOIN查询）：

```swift
// AttachmentStore.swift

/// 批量获取多条消息的附件
func batchGetAttachments(messageIds: [String]) -> [String: [StoredAttachment]] {
    guard !messageIds.isEmpty else { return [:] }

    do {
        let attachments = try ConversationStore.shared.dbRead { db in
            try StoredAttachment
                .filter(messageIds.contains(Column("messageId")))
                .order(Column("createdAt").asc)
                .fetchAll(db)
        } ?? []

        // 按messageId分组
        var grouped: [String: [StoredAttachment]] = [:]
        for attachment in attachments {
            grouped[attachment.messageId, default: []].append(attachment)
        }

        return grouped
    } catch {
        print("[AttachmentStore] Batch query failed: \(error)")
        return [:]
    }
}

/// 获取Topic下所有消息的附件（预加载）
func getAttachmentsByTopic(topicId: String) -> [String: [StoredAttachment]] {
    do {
        // JOIN查询: messages + attachments
        let attachments = try ConversationStore.shared.dbRead { db in
            try StoredAttachment
                .joining(required: StoredAttachment.message
                    .filter(Column("topicId") == topicId))
                .order(Column("createdAt").asc)
                .fetchAll(db)
        } ?? []

        var grouped: [String: [StoredAttachment]] = [:]
        for attachment in attachments {
            grouped[attachment.messageId, default: []].append(attachment)
        }

        return grouped
    } catch {
        print("[AttachmentStore] Topic query failed: \(error)")
        return [:]
    }
}
```

### 使用方式

#### ViewModel预加载
```swift
// UnifiedConversationViewModel.swift

/// 消息附件映射（批量加载）
var messageAttachments: [String: [StoredAttachment]] = [:]

func loadTopic(_ topic: Topic) {
    self.topic = topic
    self.messages = ConversationStore.shared.getMessages(topicId: topic.id)

    // 批量预加载附件（1次查询）
    self.messageAttachments = AttachmentStore.shared.getAttachmentsByTopic(topicId: topic.id)

    print("[UnifiedViewModel] Loaded \(messages.count) messages with \(messageAttachments.values.flatMap { $0 }.count) attachments")
}

/// 获取消息附件（从预加载缓存）
func getAttachments(forMessage messageId: String) -> [StoredAttachment] {
    messageAttachments[messageId] ?? []
}
```

#### MessageBubbleView使用
```swift
struct MessageBubbleView: View {
    let message: ConversationMessage
    @EnvironmentObject var viewModel: UnifiedConversationViewModel

    @State private var storedAttachments: [StoredAttachment] = []

    var body: some View {
        // ...
    }

    // 移除onAppear中的单独查询
    .onAppear {
        // ❌ 旧方式：N+1查询
        // storedAttachments = AttachmentStore.shared.getAttachments(forMessage: message.id)

        // ✅ 新方式：从预加载数据获取
        storedAttachments = viewModel.getAttachments(forMessage: message.id)
    }
}
```

### 效果验证

```swift
func testBatchQueryPerformance() {
    let store = AttachmentStore.shared
    let messageIds = (1...100).map { "msg-\($0)" }

    measure {
        _ = store.batchGetAttachments(messageIds: messageIds)
    }

    // 预期: < 50ms (vs 100次单独查询 ~2000ms)
}
```

---

## 性能监控工具

### 1. Instruments Time Profiler
**用途**: CPU热点分析

**使用**:
1. Xcode → Product → Profile
2. 选择Time Profiler模板
3. 录制30秒滚动操作
4. 查看Call Tree找到瓶颈

**关注指标**:
- MessageBubbleView初始化时间
- ContentParser解析耗时
- Database query频率

### 2. Instruments Allocations
**用途**: 内存泄漏检测

**使用**:
1. Xcode → Product → Profile
2. 选择Allocations模板
3. 录制滚动操作
4. 查看Heap增长曲线

**关注指标**:
- NSImage对象数量（应被NSCache限制）
- MessageBubbleView实例数（LazyVStack应限制）
- 内存峰值（< 200MB正常）

### 3. Xcode View Debugger
**用途**: 视图层级检查

**使用**:
1. 运行应用
2. Xcode → Debug → View Debugging → Capture View Hierarchy
3. 检查视图层级深度

**关注指标**:
- LazyVStack是否仅渲染可见视图
- 视图层级深度（< 10层正常）
- 重叠视图（避免透明度叠加）

---

## 性能基准测试

### 初始加载测试
```swift
func testInitialLoadPerformance() {
    let viewModel = createViewModelWith100Messages()

    measure {
        let view = ConversationAreaView(viewModel: viewModel)
        _ = view.body
    }

    // 目标: < 500ms
}
```

### 滚动性能测试
```swift
func testScrollingPerformance() {
    let viewModel = createViewModelWith100Messages()

    // 使用Instruments Time Profiler监控
    // 目标帧率: ≥ 55fps
}
```

### 缓存命中率测试
```swift
func testCacheHitRate() {
    let manager = AttachmentFileManager.shared
    let paths = (1...50).map { "path/\($0).png" }

    // 第一轮：填充缓存
    paths.forEach { _ = manager.getThumbnail(relativePath: $0) }

    // 第二轮：测量缓存命中
    measure {
        paths.forEach { _ = manager.getThumbnail(relativePath: $0) }
    }

    // 预期: < 10ms (缓存命中率 > 80%)
}
```

---

## 常见性能问题

### Q1: LazyVStack滚动卡顿
**症状**: 快速滚动时掉帧

**排查**:
1. 检查`.id()`是否稳定
2. 验证视图是否过度嵌套
3. 使用Time Profiler定位耗时操作

**解决**:
```swift
// ❌ 错误：动态ID导致重建
.id(UUID())

// ✅ 正确：稳定ID
.id("message-\(message.id)")
```

### Q2: NSCache未生效
**症状**: 内存持续增长，未见缓存命中

**排查**:
1. 检查`cacheKey`生成逻辑
2. 验证`setObject`调用
3. 检查`totalCostLimit`是否过小

**解决**:
```swift
// 增加日志验证缓存
if let cached = cache.object(forKey: key) {
    print("Cache HIT: \(key)")
    return cached
} else {
    print("Cache MISS: \(key)")
}
```

### Q3: 批量查询未应用
**症状**: 数据库日志仍显示N+1查询

**排查**:
1. 检查ViewModel是否调用`getAttachmentsByTopic`
2. 验证MessageBubbleView是否使用预加载数据
3. 查看日志：`[AttachmentStore] Batch query returned`

**解决**:
```swift
// 确保在loadTopic中预加载
func loadTopic(_ topic: Topic) {
    self.messageAttachments = AttachmentStore.shared.getAttachmentsByTopic(topicId: topic.id)
}
```

---

## 性能优化检查清单

### LazyVStack
- [ ] 使用`LazyVStack`替换`VStack`
- [ ] 所有视图有稳定`.id()`
- [ ] Active Parts使用Section 1（非Lazy）
- [ ] Historical Messages使用Section 2（Lazy）
- [ ] ScrollViewReader使用新ID格式

### NSCache
- [ ] 添加`thumbnailCache`静态属性
- [ ] `getThumbnail`检查缓存
- [ ] `setObject`设置cost
- [ ] 添加`clearThumbnailCache`调试接口
- [ ] 验证`countLimit`和`totalCostLimit`

### 批量查询
- [ ] `batchGetAttachments`方法实现
- [ ] `getAttachmentsByTopic`方法实现
- [ ] ViewModel预加载逻辑
- [ ] MessageBubbleView使用预加载数据
- [ ] 移除旧的单独查询调用

### 测试验证
- [ ] 初始加载 < 500ms
- [ ] 滚动帧率 ≥ 55fps
- [ ] 缩略图缓存命中率 > 80%
- [ ] 批量查询 < 50ms
- [ ] 无内存泄漏

---

## 进一步优化方向

1. **图片懒加载**: 使用`AsyncImage`延迟加载图片
2. **预渲染**: 后台线程预生成缩略图
3. **分页加载**: 首次加载最近50条，滚动到顶部时加载更多
4. **虚拟化优化**: 调整LazyVStack缓冲区大小
5. **数据库索引**: 为`messageId`和`topicId`添加索引

---

**最后更新**: 2026-01-28
**实施版本**: Phase 4-5 完成
**测试覆盖**: 基准测试待补充（Phase 7）
