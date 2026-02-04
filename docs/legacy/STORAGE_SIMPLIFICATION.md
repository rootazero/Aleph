# 文件存储架构简化实施总结

## ✅ 完成状态
所有修改已完成并通过编译。

## 修改内容

### 1. AttachmentFileManager.swift
**删除了：**
- `generatedDirectory` 静态属性
- `saveGeneratedFile()` 方法
- 从 `createDirectories()` 中删除 generated 目录创建
- 从 `deleteFilesForMessage()` 中删除 generated 目录清理
- 从 `cleanupEmptyDirectories()` 中删除 generated 目录清理

**更新文档注释：**
```swift
/// ~/.aleph/
/// ├── conversations.db
/// ├── output/{topicId}/             # AI-generated files (referenced, not copied)
/// └── attachments/
///     ├── user/{messageId}/         # User uploaded files
///     └── cached/{messageId}/       # Remote URL cache
```

### 2. UnifiedConversationViewModel.swift
**修改文件处理逻辑：**
```swift
// 旧代码：复制文件到 attachments/generated/
if let localPath = AttachmentFileManager.shared.saveGeneratedFile(
    from: url,
    toolName: "tool",
    messageId: messageId
) { ... }

// 新代码：直接引用 output/ 中的文件
let stored = StoredAttachment.forToolOutput(
    messageId: messageId,
    toolName: "ai_generated",
    sourceURL: url,
    localPath: url.path  // 直接使用绝对路径
)
```

### 3. AttachmentModels.swift
**增强路径处理：**
- 更新 `localPath` 注释，说明支持相对路径和绝对路径
- 修改 `fileURL` 计算属性，支持绝对路径（以/开头）
- 修改 `getFileSize()` 方法，支持绝对路径

```swift
var fileURL: URL? {
    guard let localPath = localPath else { return nil }

    // 如果是绝对路径，直接使用
    if localPath.hasPrefix("/") {
        return URL(fileURLWithPath: localPath)
    }

    // 否则，作为相对路径处理
    return AttachmentFileManager.attachmentsDirectory
        .appendingPathComponent(localPath)
}
```

## 新架构

### 目录结构
```
~/.aleph/
├── output/
│   ├── {topic_id_1}/          # 会话1的AI生成文件
│   │   ├── dielianhua_final.txt
│   │   └── imagery_notes.txt
│   └── {topic_id_2}/          # 会话2的AI生成文件
│       └── report.pdf
├── attachments/
│   ├── user/{messageId}/      # 用户上传文件
│   │   └── screenshot.png
│   └── cached/{messageId}/    # 远程URL缓存
│       └── downloaded_image.jpg
└── tool_output/               # 大输出截断
    └── bash_12345.txt
```

### 职责分离
| 目录 | 用途 | 管理方式 |
|------|------|---------|
| `output/` | AI生成的所有文件 | 直接引用（不复制） |
| `attachments/user/` | 用户上传文件 | 复制到attachments |
| `attachments/cached/` | 远程URL缓存 | 下载到attachments |
| `tool_output/` | 大输出截断 | Rust core管理 |

### StoredAttachment.localPath 字段规则
- **AI生成文件**：绝对路径（如：`/Users/.../output/xxx/file.txt`）
- **用户上传**：相对路径（如：`user/msg123/file.png`）
- **远程缓存**：相对路径（如：`cached/msg123/image.jpg`）

## 优势

### 1. 消除重复存储
- ❌ 旧设计：同一文件在 `output/` 和 `attachments/generated/` 各一份
- ✅ 新设计：文件只存在 `output/`，数据库保存引用

### 2. 简化代码
- 删除了 `saveGeneratedFile()` 方法（40行代码）
- 删除了目录创建和清理逻辑
- 减少了维护负担

### 3. 提高性能
- 不需要复制文件（特别是大文件）
- 减少磁盘I/O操作

### 4. 更清晰的语义
- `output/` - AI的工作空间
- `attachments/` - 仅用于用户交互和缓存

## 向后兼容性

### 数据库
- 现有用户上传和缓存文件仍然使用相对路径
- 新的AI生成文件使用绝对路径
- `fileURL` 和 `getFileSize` 方法同时支持两种格式

### 旧数据
如果有旧的 `attachments/generated/` 文件：
1. 仍然可以正常访问（相对路径逻辑保留）
2. 新生成的文件不会再复制到这里
3. 可以选择清理旧的 generated 目录：
   ```bash
   rm -rf ~/.aleph/attachments/generated
   ```

## 测试验证

### 构建状态
- ✅ Rust core 编译成功
- ✅ macOS 应用构建成功
- ✅ 无编译警告或错误

### 功能测试建议
1. 运行 `/classical-poetry` skill，确认生成的文件显示在附件中
2. 上传用户文件，确认仍然正常工作
3. 检查远程URL图片缓存是否正常
4. 删除会话，确认附件清理逻辑正常

### 日志关键字
```
[UnifiedViewModel] Saved reference to AI-generated file: /Users/.../output/.../file.txt
```

## 后续清理（可选）

### 删除空目录
```bash
# 如果 generated 目录是空的，可以删除
rmdir ~/.aleph/attachments/generated 2>/dev/null || true
```

### 数据库清理（可选）
如果有旧的 generated 类型附件记录，可以迁移或清理：
```sql
-- 查看旧的 generated 附件
SELECT * FROM attachments
WHERE localPath LIKE 'generated/%';

-- 可选：删除旧记录（如果文件已不存在）
DELETE FROM attachments
WHERE localPath LIKE 'generated/%'
  AND NOT EXISTS (SELECT 1 FROM messages WHERE id = messageId);
```

## 相关文档
- [附件显示机制修复](./ATTACHMENT_FIX_IMPLEMENTATION.md)
- [目录结构说明](./DIRECTORY_STRUCTURE.md)
- [架构文档](./ARCHITECTURE.md)
