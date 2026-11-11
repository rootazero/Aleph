# Fix brief — GX4

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX4.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX4.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX4.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX4.md

## FIX THESE (10)

### GX4-01 [P0] 逐 delta 独立脱敏会把跨 chunk 凭据完整送出
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/event_emitter/redacting.rs:94` (`RedactingEmitter::emit`, `ResponseChunk` / `Reasoning`)
- **Failure mode**: unattended run 输出两个 delta：`sk-` 和 `ABCDEFGHIJKLMNOPQRSTUVWX` → 每段单独都不匹配 API-key 规则，故两段原样进入事件总线。第二帧的 `full_text` 即使已脱敏，也不能收回第一帧，且该帧自己的 `delta` 仍是明文；按 delta/content 拼接的客户端能恢复完整密钥。Instant 模式也未避开此问题：内层 planner 累积的是这些已经逐段“脱敏”的 delta，而不是脱敏后的 `full_text`，最终会重新组成完整明文。`Reasoning.content` 同样按片段独立处理。跨批次核对：`src/exec/masker.rs:94-121` 的 masker 无跨调用状态；`src/exec/secret_patterns.rs:65` 的规则是 `\bsk-[a-zA-Z0-9]{20,}`；`src/gateway/execution_engine/event_drain.rs:59-89` 逐 delta 发帧；`src/gateway/execution_engine/run_loop/inner.rs:698-701` 将 redactor 放在整个输出链外；`interfaces/cli/src/commands/run_follow.rs:201-207` 确实拼接 content。批内 `src/gateway/events/frame.rs` 将 delta 复制到 wire content。
- **Evidence**:
  > `delta: self.mask(&delta),`
  > `full_text: self.mask(&full_text),`
  > `content: self.mask(&content),`
- **Suggested fix**: 在 unattended 的流式文本边界持有尚不能安全发布的文本，形成完整 response/reasoning 后统一脱敏再交给内层 emitter；对于任意 operator regex，最小可靠策略是完整消息缓冲，而不是仅从 masked full_text 推导新 delta。保留 attended 的实时输出、事件 wire 形状及生命周期顺序；代价是 unattended 文本首次可见时间变晚。回归至少覆盖凭据在前缀处分片，以及 redactor 外包 Instant emitter 的组合。
- **Confidence**: 1.0

### GX4-02 [P1] 阻塞 PTY 写在全局 admission 锁内执行，输入堵塞会卡住整个 provider
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/pty/session.rs:306` (`PtySession::write_input`); `src/gateway/pty/manager.rs:1009` (`PtyManager::with_generation`)
- **Failure mode**: 子进程进入 raw 模式但停止读取 stdin，客户端粘贴超过 PTY 输入队列容量的数据 → `write_all` 阻塞，且调用栈仍持有 manager 的全局 admission mutex。该请求既占住 Tokio worker，又使其他 session 的 input、close、flush 和 provider stop 等待同一把锁；正常 close 无法进来终止这个写入。跨批次核对：`src/gateway/handlers/pty.rs:279-301` 的 async handler 直接调用 `write_at`，没有转交阻塞工作线程。批内 `write_at` 经 `with_session_at` 进入此 admission 临界区。
- **Evidence**:
  > `let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());`
  > `w.write_all(data)`
  > `let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());`
- **Suggested fix**: admission 下只做 generation 校验和有界写入交接，将实际阻塞写交给可独立停止的单 session writer；使用有界队列，满时返回可见错误。不要只在现有调用外包 spawn_blocking 而继续长期持有 admission——那只能释放 worker，仍会堵死 close/stop。保留逐 session 的输入顺序、generation 隔离及 stop 后拒绝新输入语义。
- **Confidence**: 1.0

### GX4-03 [P1] owner 历史 FIFO 会淘汰仍存活 session 的授权事实
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/pty/manager.rs:140` (`Inner::remember_owner`)
- **Failure mode**: 保留一个长期运行的、带 creator 的 session，再顺序创建并关闭 1024 个短 session（同时存活数始终远小于 64）→ 最老的长期 session owner stamp 被无条件移除。`owner_of` 此后返回 Unknown，`SessionOwner::admits(Some(owner))` 拒绝真正的 owner；该 session 仍在 list 中，但 input/resize/attach/close 和面向该身份的事件投递都失效。并发数量上限不能保护一个活了很久的 session 免受累计创建次数淘汰。跨批次核对：`src/gateway/handlers/pty.rs:455-465` 的 addressed 方法授权确实消费 `owner_of` 的结果；journal fallback 不会重新授予 live session 操作权限。
- **Evidence**:
  > `while self.owner_order.len() > OWNER_RETENTION {`
  > `if let Some(old) = self.owner_order.pop_front() {`
  > `self.owners.remove(&old);`
- **Suggested fix**: 仅淘汰已经结束的 owner 历史，或在 `owner_of` 中先读取 live session 的不可变 created_by，再回退到有界历史。保留 Unknown 的 fail-closed 判据和 exit 后的有限历史保留，不以扩大常量掩盖 churn 问题。
- **Confidence**: 1.0

### GX4-04 [P1] 纯光标移动被 take_patch 判成无变化
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/pty/screen/perform/mod.rs:273` (`Screen::take_patch`)
- **Failure mode**: feed `abc` 并消费第一帧，再 feed `ESC [ D` 或 BS → Grid 的 cursor 已左移，但没有行内容或其他 metadata 变化；`take_patch` 返回 None，`feed_and_take_frame` 因而不发帧。客户端光标停在原位，直到下一次字符写入或其他 metadata 改变，交互式行编辑的光标显示错误。Grid 的 `move_cursor` / `goto` 只改变坐标，不把行标脏；当前 Screen 仅追踪 last_sent_cursor_visible，没有追踪 last_sent_cursor 位置。
- **Evidence**:
  > `cursor: Some(self.grid.cursor()),`
  > `if patch.rows.is_empty()`
  > `return None;`
- **Suggested fix**: 记录最后发布的 cursor 坐标，将坐标变化纳入 patch 的 news 判据；发布后同步更新。保留完全静止终端不发帧，以及无须整行重绘即可移动光标的现有 wire 结构。
- **Confidence**: 1.0

### GX4-05 [P1] Instant 的终结补帧以较新 seq 先发，随后发出较旧 RunComplete
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/event_emitter/instant_buffer.rs:185` (`plan_instant`), `src/gateway/event_emitter/instant_buffer.rs:233` (`InstantBufferingEmitter::emit`); `src/gateway/event_emitter/impls.rs:84` (`GatewayEventEmitter::emit`)
- **Failure mode**: 已缓冲一个 seq=0 的非 final delta，随后 producer 为 RunComplete 领取 seq=1 → planner 再领取 seq=2 生成 Prepend final chunk，两个 emitter 都先发 seq=2 的 chunk，随后原样发 seq=1 的 RunComplete。summary fallback 也走同一条路径。因此单 run、单线程、无发布失败时即可稳定看到终结阶段序号倒退；事件排序/去重消费者不能把 seq 当作流的单调顺序。`EventEmitter::emit_run_complete` 在进入 planner 前就领取 seq，trait 对 next_seq 明确要求单调递增，生产方不能补救内层此次重排。没有假设必须跨 run 复用 emitter 才能触发。
- **Evidence**:
  > `InstantOutcome::Prepend(vec![final_chunk(run_id.clone(), next_seq(), text)])`
  > `self.inner.emit(e).await?;`
  > `self.inner.emit(event).await`
- **Suggested fix**: planner 同时安排补出的 chunk 和 terminal event 的序号，例如补帧使用原 RunComplete seq，RunComplete 再领取更大的 seq；让两个调用方都消费这个完整计划，而非 prepend 后原样转发旧 terminal。保留 exactly-once RunComplete、final 文本去重及既有 wire 字段。
- **Confidence**: 0.99

### GX4-06 [P1] close 和容量淘汰先丢掉会话，再吞掉 kill 错误并报成功
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/pty/manager.rs:779` (`PtyManager::close_admitted`), `src/gateway/pty/manager.rs:680` (`PtyManager::spawn_admitted`)
- **Failure mode**: ChildKiller 返回错误（例如进程改变凭据后 signal 被拒绝；批内也已有 RefuseKill 注入场景）→ close 已从 sessions/order/viewports 移除目标，写入 killed verdict，再经不返回 Result 的 `s.kill()` 吞掉错误并返回 Ok。容量淘汰同样丢失失败目标。子进程和 reader/waiter 仍在，但下一次 close 或 shutdown 已找不到它，用户得到的成功结果与实际状态相反。批内 `close_all_checked` 已有“try_kill 失败则保留 session 并返回 residual”的正确模式，说明错误并非不可表达。
- **Evidence**:
  > `inner.sessions.remove(session_id)`
  > `s.kill();`
  > `Ok(())`
- **Suggested fix**: close/eviction 使用 `try_kill` 并在失败时保留或恢复管理权，向调用方返回具体错误；容量淘汰失败不能悄悄接受新 session 并遗失旧进程。仅在成功清理边界记录 killed verdict，保留失败时可重试、owner 不变及 registry 临界区内不做阻塞 I/O 的约束。
- **Confidence**: 1.0

### GX4-09 [P2] 子进程已经启动后，reader/writer 获取失败会绕过所有 reap 路径
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/pty/session.rs:271` (`PtySession::spawn_inner`)
- **Failure mode**: `spawn_command` 成功后，`try_clone_reader` 或 `take_writer` 因 fd 耗尽等原因返回错误 → `?` 提前退出，尚未调用 spawn_reader，也没有 kill/wait 清理 child。Unix 下 child 是 boxed `std::process::Child`，drop 不会 wait：即便关闭 master 让它立即收到 HUP 退出，也可能留下 zombie；忽略 HUP 的子进程还会继续运行，而 manager 只看到 spawn 失败。跨批次依赖核对：portable-pty 0.9.0 `src/unix.rs:228-297,334-339` 返回 boxed std child，`src/unix.rs:352-363` 的两个句柄获取步骤均可能因 fd clone 失败而报错。
- **Evidence**:
  > `let child = slave`
  > `.map_err(|e| format!("clone_reader failed: {e}"))?;`
  > `.map_err(|e| format!("take_writer failed: {e}"))?;`
- **Suggested fix**: 优先把 master reader/writer 的 fallible 获取移到创建 child 之前；若平台必须保持现有顺序，则用持有 child 的回滚清理保证所有后续错误都终止并 reap。保留原始 spawn/clone/writer 错误上下文，不能仅 drop Child 视作回滚。
- **Confidence**: 0.99

### GX4-10 [P2] 丢弃 OS 线程创建错误会让 spawn 成功但没有 reader 或 reaper
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/pty/session.rs:578` (`spawn_reader`), `src/gateway/pty/session.rs:600` (`spawn_reader`)
- **Failure mode**: thread/pid 额度或内存不足导致任一 Builder::spawn 返回 Err → 两处错误都被 `.ok()` 丢弃，外层仍返回 Ok(session)。reader 未启动时，正常输出永远不会被读取，子进程可能卡在满输出队列；waiter 未启动时，闭包中的 child 被 drop 而未 wait，既不会执行 settle_exit，也不会发布 exit/移除会话，并可留下 zombie。线程资源耗尽正是该路径应当返回失败的边界，不能把降级隐瞒成成功。
- **Evidence**:
  > `.name(format!("pty-reader-{}", session.id))`
  > `.name(format!("pty-waiter-{}", session.id))`
  > `.ok();`
- **Suggested fix**: 让工作线程启动返回 Result，必须确认 reader 和 waiter 都已建立后才报告 session 启动成功；失败时回滚已启动的一半并终止/reap child，尤其保证 waiter closure 因创建失败被 drop 时仍有 child 清理所有者。保留仅 waiter 做一次 settlement 的设计，不让 reader 再竞争发布 exit。
- **Confidence**: 0.99

### GX4-11 [P2] 新建 alternate grid 会把已配置 scrollback ceiling 重置到 1000
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/pty/screen/perform/csi.rs:93` (`Performer::toggle_alt_screen`)
- **Failure mode**: session spawn 时设置 scrollback_lines=7，随后程序第一次进入 `?47h`/`?1047h`，或每次进入 `?1049h` → 新建 Grid 使用默认 1000，而不是当前 7。全高度滚动随后可保留 1000 行，`scrollback_limit()` 也读回 1000；operator 配置的保留/内存上限被程序自身的正常 escape 切换绕过。设置高于 1000 时又会意外缩短历史。`set_scrollback_limit` 虽更新 visible 和已存在的 parked grids，但无法覆盖此后创建的 grid。
- **Evidence**:
  > `.unwrap_or_else(|| Grid::new(rows, cols)),`
  > `AltBuffer::Cleared => Grid::new(rows, cols),`
- **Suggested fix**: 创建新 alternate grid 时继承当前 grid 的 scrollback_limit，再执行现有 swap；复用 retained alternate 时保留其内容。保持 legacy buffer 保留内容、1049 清空内容及 nested enter no-op 三个既有语义。
- **Confidence**: 1.0

### GX4-12 [P2] 下调 max_sessions 后每次只淘汰一个，活跃数量永远不降到新上限
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/pty/manager.rs:671` (`PtyManager::spawn_admitted`)
- **Failure mode**: 已有 4 个存活 session，live config 将 max_sessions 从 4 改为 1，再创建新 session → 当前代码只删除最老的一个，然后立即插入新的一个，结果仍有 4 个。后续每次 spawn 都是减一加一，永远不能把 map 降到配置的新上限。`set_max_sessions` 只 store 新值，没有其他收缩路径。跨批次核对：`src/gateway/handlers/pty.rs:219-224` 每次 spawn 都应用当前 terminal config 的 max_sessions，所以下调配置后的 admission 确实进入这里。
- **Evidence**:
  > `if inner.sessions.len() >= cap {`
  > `evicted = inner.sessions.remove(&old_id);`
  > `inner.sessions.insert(id, session);`
- **Suggested fix**: admission 时按 FIFO 收集足够多的淘汰目标，使插入前 live 数量小于当前 cap；在 inner map 锁外逐一清理，并与 kill 失败时保留管理权的策略一致，失败不能静默漏掉 residual。保留最少一个 session 的配置归一化和既有 FIFO 策略。
- **Confidence**: 1.0

## Clean files

以下为本批次未发现符合本轮证据门槛缺陷的文件；不表示已做运行时验证。

- `src/gateway/pty/screen/perform/tests.rs`
- `src/gateway/events/frame.rs`
- `src/gateway/event_emitter/team_fanout.rs`
- `src/gateway/event_emitter/origin_fanout.rs`
- `src/gateway/pty/screen/grid/mod.rs`
- `src/gateway/event_emitter/types.rs`
- `src/gateway/events/frame_census.rs`
- `src/gateway/pty/screen/grid/tests.rs`
- `src/gateway/pty/mod.rs`
- `src/gateway/event_emitter/tests.rs`
- `src/gateway/pty/screen/diff.rs`
- `src/gateway/pty/screen/grid/edit.rs`
- `src/gateway/event_emitter/mod.rs`
- `src/gateway/pty/jail.rs`
- `src/gateway/pty/screen/perform/osc.rs`
- `src/gateway/pty/screen/grid/write.rs`
- `src/gateway/pty/screen/grid/scroll.rs`
- `src/gateway/event_emitter/artifact_ping.rs`
- `src/gateway/pty/screen/mod.rs`
- `src/gateway/pty/screen/perform/esc.rs`
- `src/gateway/pty/screen/grid/resize.rs`
- `src/gateway/pty/screen/convert.rs`
- `src/gateway/pty/screen/text.rs`
- `src/gateway/events/mod.rs`


## DO NOT FIX (2) — refuted / deferred by adjudication or routing

- GX4-07: ### GX4-07 [P2] clone_killer 在 Unix 只发 SIGHUP，没有保证终止或升级
- GX4-08: ### GX4-08 [P2] Unix 上只 drop 原 master 不能中断仍持有克隆 fd 的 reader
