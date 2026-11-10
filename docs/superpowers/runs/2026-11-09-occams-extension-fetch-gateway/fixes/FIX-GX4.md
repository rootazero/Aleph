# FIX-GX4 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- GX4-01: FIXED
- GX4-03: FIXED
- GX4-02: FIXED
- GX4-05: FIXED
- GX4-04: FIXED
- GX4-06: FIXED
- GX4-10: FIXED
- GX4-09: FIXED
- GX4-11: FIXED
- GX4-12: FIXED


---

# 第二轮修复复核 / second fix wave (appended)

# GX4 — second fix wave / 第二轮修复

## GX4-01 — FIXED — unattended response/reasoning fragments are buffered through a message boundary, coalesced without byte/character offsets, then masked before inner/Instant delivery; attended wiring is unchanged. / unattended 文本完整缓冲后再脱敏，删除字节/字符混用；新增分片凭据、中文前缀及 Instant 组合回归。
## GX4-02 — REVERTED — restored the writer fields/construction and write_input exactly to 79dacfa33; the attempted queue could not independently cancel an already-blocked write when descendants retain the slave FD, so it was removed under the requested rollback rule. / 已将输入相关改动还原到指定基线；原 admission 锁内阻塞写问题仍存在，需要完整的可取消平台 I/O 方案。
## GX4-03 — FIXED — FIFO eviction removes only ended entries, never consumes live entries, and new sessions are inserted before owner retention runs. / 活跃 owner 在 FIFO 和 map 中同时保留；新增长期会话经历超过保留上限 churn 后仍可授权及关闭的回归。
## GX4-06 — FIXED — close and capacity eviction require an admission guard, retain failed kills, and record killed only after successful removal; managed waiter settlement takes the same admission lock, and shutdown no longer records failed kills as successful. / 关闭及淘汰失败保留管理权，成功移除后才记 killed；新增失败关闭/淘汰与并发关闭只 kill 一次回归。
## GX4-10 — FIXED — reader/waiter startup returns Result, workers wait behind startup gates, and child ownership transfers to the waiter only after both threads start; failed startup kills/reaps locally and aborts the parked workers. / 两个工作线程全部建立后才启动读取、移交 child；新增 reader/waiter 分别启动失败均返回错误的注入回归。

## Verification / 验证

- `rustfmt --edition 2021 --emit stdout` parsed all three edited Rust files successfully; `git diff --check` passed. / 三个文件通过 rustfmt 语法解析及 diff 空白检查。
- A small standalone rustc MIR probe passed for the buffering borrow pattern and admission-guard closure signature. This does not type-check the project. / 独立 rustc 探针仅验证借用模式及 guard 签名，不等同于项目类型检查。
- No cargo check/test/clippy/build was run, as instructed; the new regressions have not been executed. Full formatting was not applied by the parser-only check. / 按要求未运行 cargo；新增回归未执行，语法检查未写回格式化结果。
- Graph queries were attempted before source reads, but this worktree has no `graphify-out/graph.json`; direct source/API inspection was used instead. / 工作树缺少 graphify 图，已尝试查询，随后直接核对源码及真实接口。

## Trade-offs and unverified limits / 取舍及未验证边界

- Unattended text and interleaved events wait until a complete message/tool/terminal boundary; arbitrary regexes therefore see the whole text. Attended streaming is untouched. / unattended 首次文本可见时间变晚，attended 实时流不变。
- GX4-02 is deliberately unresolved after rollback: a stalled child can still block write_input while admission is held, affecting other session operations and provider stop. / GX4-02 回退后仍未解决：子进程不读输入时仍可阻塞全局 admission，影响其他会话及 provider stop。
- Capacity is reclaimed before starting a replacement child. If replacement startup subsequently fails, the successfully closed old session is not restored. / 满容量时先淘汰；随后新会话启动失败不会恢复已关闭的旧会话。
- Cancellable platform I/O was not implemented, and cross-platform PTY stress tests were not run. / 未实现平台可取消 I/O，也未做跨平台 PTY 压力测试。
- No git history operations or unrelated source edits were performed. / 未提交、stash、改写历史或编辑无关源码。
