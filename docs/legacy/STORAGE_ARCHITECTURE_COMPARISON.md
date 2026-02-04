# 文件存储架构对比

## 旧架构（冗余）

```
~/.aleph/
├── output/
│   └── {topic_id}/
│       ├── file1.txt      ← AI生成（原始）
│       └── file2.pdf      ← AI生成（原始）
│
├── attachments/
│   ├── user/{messageId}/
│   │   └── upload.png     ← 用户上传
│   ├── generated/{messageId}/  ❌ 重复！
│   │   ├── file1.txt      ← 从output复制
│   │   └── file2.pdf      ← 从output复制
│   └── cached/{messageId}/
│       └── remote.jpg     ← URL缓存
│
└── tool_output/
    └── bash_123.txt       ← 大输出截断
```

**问题：**
- 🔴 同一文件存储两次（output + attachments/generated）
- 🔴 需要维护同步逻辑
- 🔴 浪费磁盘空间
- 🔴 增加复制开销（特别是大文件）

---

## 新架构（简化）

```
~/.aleph/
├── output/
│   └── {topic_id}/
│       ├── file1.txt      ← AI生成（唯一存储）
│       └── file2.pdf      ← AI生成（唯一存储）
│           ↑                   ↑
│           └───────────────────┘
│                   直接引用（不复制）
│
├── attachments/
│   ├── user/{messageId}/
│   │   └── upload.png     ← 用户上传
│   └── cached/{messageId}/
│       └── remote.jpg     ← URL缓存
│
└── tool_output/
    └── bash_123.txt       ← 大输出截断
```

**优势：**
- ✅ 单一数据源（output是AI的工作空间）
- ✅ 无需复制（节省磁盘和I/O）
- ✅ 代码简化（删除40+行复制逻辑）
- ✅ 职责清晰（attachments仅用户交互）

---

## 附件数据库字段变化

### StoredAttachment.localPath

**旧设计：**
```swift
// 所有附件都是相对路径
localPath: "generated/msg123/file.txt"  // 相对于 ~/.aleph/attachments/
localPath: "user/msg123/upload.png"      // 相对于 ~/.aleph/attachments/
```

**新设计：**
```swift
// AI生成：绝对路径
localPath: "/Users/.../output/topic123/file.txt"  // 绝对路径

// 用户上传：相对路径（不变）
localPath: "user/msg123/upload.png"  // 相对于 ~/.aleph/attachments/

// 远程缓存：相对路径（不变）
localPath: "cached/msg123/remote.jpg"  // 相对于 ~/.aleph/attachments/
```

### 文件URL解析

```swift
var fileURL: URL? {
    guard let localPath = localPath else { return nil }

    // 绝对路径：直接使用
    if localPath.hasPrefix("/") {
        return URL(fileURLWithPath: localPath)
    }

    // 相对路径：相对于attachments目录
    return AttachmentFileManager.attachmentsDirectory
        .appendingPathComponent(localPath)
}
```

---

## 代码简化对比

### AttachmentFileManager.swift

**删除前（99行）：**
```swift
static var generatedDirectory: URL { ... }

func saveGeneratedFile(from sourceURL: URL, ...) -> String? {
    // 1. 创建目录
    // 2. 生成文件名
    // 3. 复制文件
    // 4. 返回相对路径
    // ~40行代码
}

func deleteFilesForMessage(_ messageId: String) -> Int {
    let subDirs = ["user", "generated", "cached"]  // 需要清理3个目录
    ...
}
```

**删除后（63行）：**
```swift
// ❌ generatedDirectory - 已删除
// ❌ saveGeneratedFile - 已删除

func deleteFilesForMessage(_ messageId: String) -> Int {
    let subDirs = ["user", "cached"]  // 只需清理2个目录
    ...
}
```

**减少：36行代码 + 1个方法 + 1个属性**

---

## 性能对比

### 生成4个文件（如 classical-poetry）

| 操作 | 旧架构 | 新架构 | 改进 |
|------|--------|--------|------|
| 磁盘写入 | 8次（4原始+4复制） | 4次（仅原始） | -50% |
| 磁盘使用 | ~20KB × 2 = 40KB | ~20KB | -50% |
| 文件复制 | 4次 | 0次 | -100% |
| 代码执行 | saveGeneratedFile × 4 | 直接引用 | 快得多 |

### 大文件场景（如生成10MB PDF）

| 操作 | 旧架构 | 新架构 | 改进 |
|------|--------|--------|------|
| 磁盘使用 | 20MB | 10MB | -50% |
| 复制耗时 | ~100ms | 0ms | -100% |

---

## 迁移指南

### 对新安装用户
无需操作，直接使用新架构。

### 对现有用户

**自动兼容：**
- 旧的 `attachments/generated/` 文件仍可访问（相对路径逻辑保留）
- 新生成的文件使用绝对路径引用
- 无需数据迁移

**可选清理：**
```bash
# 1. 删除空的 generated 目录
rmdir ~/.aleph/attachments/generated 2>/dev/null || true

# 2. 查看是否有旧的 generated 文件
ls -la ~/.aleph/attachments/generated/*/

# 3. 如果确认不需要，可以删除
# rm -rf ~/.aleph/attachments/generated
```

---

## 总结

| 方面 | 旧架构 | 新架构 |
|------|--------|--------|
| **目录数量** | 4个（output + 3个attachments子目录） | 3个（output + 2个attachments子目录） |
| **文件复制** | ✅ 需要 | ❌ 不需要 |
| **磁盘空间** | 重复存储 | 单一存储 |
| **代码行数** | 99行 | 63行（-36行） |
| **维护成本** | 高（需同步） | 低（直接引用） |
| **性能** | 较慢（复制开销） | 快（零复制） |
| **语义清晰度** | 混淆 | 清晰 |

**结论：新架构在所有维度上都优于旧架构。**
