# Phase 3 启动Prompt

**使用场景：** 在新会话中开始Phase 3实施  
**前置条件：** 确认main分支HEAD在edb23b67f或之后  
**预期结果：** 实现NSH Desktop核心UI组件

---

## 复制给Claude的Prompt

```
我要开始实施NSH Desktop Phase 3：Core UI Implementation。

## 当前状态

- 仓库：/Volumes/TBU4/Workspace/Aleph
- 分支：main
- 预期HEAD：edb23b67f (docs: add Web Target architecture amendment)
- Phase 2已完成：连接层已集成，18/18测试通过

请先确认：
1. 当前HEAD commit hash和消息
2. `cargo test -p aleph-nsh-shell` 是否通过
3. Phase 2文件是否存在：
   - desktop/nsh-shell/src/connection.rs
   - desktop/nsh-shell/src/runtime.rs
   - desktop/nsh-shell/src/supervisor.rs

## 实施计划

完整计划文档：docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md

**Phase 3目标：** 将当前的文本占位符替换为功能性UI

**5个核心任务：**
1. Task 6: Session State Management (session.rs)
2. Task 7: Input Field Component (components/input.rs)
3. Task 8: Message List Component (components/message_list.rs)
4. Task 9: Streaming Token Display (streaming.rs)
5. Task 10: Integration into App (main.rs修改)

## 架构约束

**必读文档：**
1. ADR-001~009 (docs/research/NSH_ADR-001~009.md)
   - Mixed Runtime：Rust Native Components + TSX
   - Web Target：每个Native Component需声明Web fallback
   
2. PROTOCOL_V1 (docs/research/NSH_PROTOCOL_V1.md)
   - § 6.2: session.* RPC方法
   - § 6.3: message.* RPC方法
   - § 8: Event Envelope格式
   - § 9: Snapshot + Replay机制

3. Phase 2 Plan (docs/superpowers/plans/2026-11-05-nsh-desktop-phase2-connection.md)
   - 连接层实现参考
   - signal + spawn模式

## 关键设计决策

**组件选择（来自ADR-001）：**
- Input field, message list, streaming text → Rust Native Components (高频、复杂生命周期)
- Message container, layout → TSX (页面组合)
- Tool call cards → TSX (结构化数据展示)

**依赖锁定：**
- NSH版本：`rev = "2ab3b6bc27"` (已在Cargo.toml)
- Phase 1验证：signal、spawn、text、component宏可用

**State Management：**
```rust
// Phase 2已有
enum ShellStatus {
    Connecting,
    Connected { server_version: String, session_count: usize },
    Error(String),
}

// Phase 3新增
struct SessionState {
    session_id: Option<String>,
    messages: Vec<Message>,
    input_text: String,
    is_generating: bool,
}
```

## 实施策略

**使用SDD（Subagent-Driven Development）：**
1. 阅读完整plan：docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md
2. 使用superpowers:writing-plans生成详细的Task 6-10实施计划
3. 按任务顺序实施，每个任务：
   - 写实现
   - 写测试
   - 运行`cargo test -p aleph-nsh-shell`
   - 运行`cargo clippy -- -D warnings`
   - 提交（格式：`feat(nsh): implement Task N - <description>`）

**关键检查点：**
- Task 6完成后：SessionManager可创建session、订阅事件
- Task 7完成后：InputField组件可输入、提交
- Task 8完成后：MessageList可渲染消息列表
- Task 9完成后：Streaming可实时显示token
- Task 10完成后：完整UI可用，端到端测试通过

## 风险预案

**如果Server RPC未实现：**
- 先用Mock Client开发UI（见plan § Risk 2）
- Server RPC实现后再集成真实client

**如果NSH API不匹配：**
- 验证NSH rev 2ab3b6bc27的API（见plan § Risk 1）
- 如有breaking change，更新Cargo.toml依赖或调整代码

**如果性能问题：**
- 参考plan § Risk 3的优化建议
- 批量delta、虚拟滚动、flamegraph分析

## 验收标准

Phase 3完成 = 所有条件满足：
- [ ] `cargo test -p aleph-nsh-shell` 全部通过
- [ ] `cargo build -p aleph-nsh-shell` 无警告
- [ ] 手动QA：可输入消息、提交、看到响应
- [ ] 手动QA：流式token实时显示
- [ ] 手动QA：重连后对话恢复
- [ ] 代码审查：Native Components文档标注Web fallback
- [ ] 提交消息遵循惯例：`feat(nsh): implement Phase 3 core UI`

## 参考资料

**代码参考：**
- Phase 2连接层模式：desktop/nsh-shell/src/connection.rs
- Phase 1 spike示例：desktop/nsh-spike/src/main.rs
- Server RPC handlers：server/src/rpc/ (如果存在)

**外部资源：**
- NSH文档：仓库 rev 2ab3b6bc27
- JSON-RPC 2.0: https://www.jsonrpc.org/specification
- Tokio教程：https://tokio.rs/tokio/tutorial

## 立即开始

请执行：
1. 确认当前状态（HEAD、测试、文件存在性）
2. 阅读完整plan：docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md
3. 询问我是否有任何偏好或约束
4. 开始Task 6实施

有任何问题随时问我。Let's build Phase 3! 🚀
```

---

## 使用说明

1. **启动新会话**：在Claude Code中开始新对话
2. **复制prompt**：将上面"复制给Claude的Prompt"部分全文复制
3. **粘贴执行**：粘贴到新会话，Claude会自动检查状态并开始实施
4. **监控进度**：Claude会按Task 6→7→8→9→10顺序实施，每个任务独立提交

## 预期时间线

- Task 6 (Session Manager): 1-2小时
- Task 7 (Input Field): 1小时
- Task 8 (Message List): 1-2小时  
- Task 9 (Streaming): 2-3小时
- Task 10 (Integration): 1-2小时
- **Total**: 6-10小时（根据Server RPC可用性调整）

## 故障排查

**如果Claude迷失方向：**
```
请重新阅读plan文档：
docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md

当前应该在哪个任务？下一步是什么？
```

**如果测试失败：**
```
Phase 2的18个测试必须保持通过。
如果新代码破坏了旧测试，先修复再继续。
```

**如果需要暂停：**
```
Phase 3进度：Task [6/7/8/9/10]已完成
下次继续：从Task [N+1]开始
当前HEAD：[commit hash]
```

## 成功标志

当Claude报告"Phase 3 complete"时，验证：
1. 所有测试通过（包括Phase 2的18个）
2. 可以运行`cargo run -p aleph-nsh-shell`看到UI
3. progress.md已更新记录Task 6-10完成状态
4. 所有5个任务的commit已推送到main

---

**准备好了吗？复制上面的prompt开始Phase 3！** 🎯
