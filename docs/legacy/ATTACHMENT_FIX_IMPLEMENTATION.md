# 附件显示机制修复 - 实现记录

## 问题描述
classical-poetry等skill通过bash命令或Python脚本创建的文件没有被Aether的文件追踪系统记录，导致UI无法显示这些附件。

## 根本原因
1. `record_written_file()` 只在 FileOpsTool 中被调用
2. bash命令或Python脚本直接创建的文件不会被追踪
3. Agent Loop结束时，只返回已追踪的文件列表

## 解决方案
在Agent Loop和Skill执行完成后，扫描工作目录（`~/.aether/output/{topic_id}/`），找出所有在session期间新创建的文件，并将它们添加到 `[GENERATED_FILES]` 块中。

## 实现细节

### 1. 文件扫描功能（core/src/rig_tools/file_ops/state.rs）
添加三个新函数：
- `mark_session_start()`: 记录session开始时间
- `scan_new_files_in_working_dir()`: 扫描工作目录中在session期间创建的文件
- 添加全局变量 `SESSION_START_TIME` 用于时间基准

扫描逻辑：
1. 遍历工作目录中的所有文件
2. 检查文件修改时间是否在session开始之后
3. 排除已经被追踪的文件（避免重复）
4. 返回新发现的文件列表

### 2. 集成到Agent Loop（core/src/ffi/processing/agent_loop.rs）
- 在 `clear_written_files()` 后调用 `mark_session_start()`
- 在 `take_written_files()` 后调用 `scan_new_files_in_working_dir()`
- 合并两个文件列表，一起放入 `[GENERATED_FILES]` 块

### 3. 集成到Skill执行（core/src/ffi/processing/skill.rs）
同样的修改应用到skill执行流程。

### 4. 依赖项（core/Cargo.toml）
添加 `walkdir = "2.5"` 用于目录遍历。

## 文件修改清单
1. ✅ core/src/rig_tools/file_ops/state.rs - 添加文件扫描功能
2. ✅ core/src/rig_tools/file_ops/mod.rs - 导出新函数
3. ✅ core/src/ffi/processing/agent_loop.rs - 集成扫描功能
4. ✅ core/src/ffi/processing/skill.rs - 集成扫描功能
5. ✅ core/Cargo.toml - 添加walkdir依赖

## 编译状态
- ✅ Rust core编译成功
- ✅ macOS应用构建成功
- ⏳ 单元测试运行中

## 测试验证

### 方法1：使用classical-poetry skill重新测试
```bash
# 在Aether中运行
/classical-poetry 使用蝶恋花词牌创作一首词，主题是用闺怨表达政治失意。
```

预期结果：
- UI应该显示4个附件文件：
  - dielianhua_final.txt
  - validation_summary.txt
  - imagery_notes.txt
  - revision_log.txt

### 方法2：手动验证日志
检查日志中是否出现类似信息：
```
Found additional files in working directory not tracked by tools
scanned_count = 4
Appending generated files to agent loop response
file_count = 4
```

### 方法3：检查数据库
```sql
sqlite3 ~/.aether/conversations.db "
SELECT content
FROM messages
WHERE role='assistant'
  AND content LIKE '%[GENERATED_FILES]%'
ORDER BY createdAt DESC
LIMIT 1;
"
```

## 日志追踪
关键日志标记：
- `"Marked session start time for file tracking"` - session开始
- `"Scanning working directory for new files"` - 开始扫描
- `"Found new file created during session"` - 发现新文件
- `"Completed working directory scan"` - 扫描完成
- `"Found additional files in working directory"` - 发现未追踪文件

## 性能考虑
- 文件扫描只在Agent Loop/Skill结束时执行一次
- 使用修改时间过滤，避免扫描所有历史文件
- 排除已追踪文件，避免重复
- 对于包含大量文件的目录，性能影响应该很小

## 边界情况
1. ✅ 工作目录不存在 - 跳过扫描
2. ✅ 没有新文件 - 返回空列表
3. ✅ session开始时间未设置 - 跳过扫描
4. ✅ 文件修改时间无法读取 - 记录警告，继续处理其他文件

## 下一步
1. 等待单元测试完成
2. 实际运行classical-poetry测试
3. 验证UI是否正确显示附件
4. 如有问题，查看日志进行调试
