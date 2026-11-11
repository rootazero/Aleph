# FIX-G4 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- G4-01: FIXED
- G4-03: FIXED
- G4-04: FIXED
- G4-02: FIXED
- G4-08: FIXED
- G4-06: FIXED
- G4-07: FIXED
- G4-10: FIXED
- G4-09: FIXED

---

# G4 — Second fix wave / 第二轮修复

## G4-03 — FIXED — Slot 保留 acquisition-time watch sender；complete 与 discard 在 DashMap 原子操作中仅作用于 same_channel 的 generation，并增加 stale complete/discard/drop 回归测试。 / Fence completion and removal with acquisition identity; preserve pruning and completed-result TTL.
## G4-03-INCOMPLETE — FIXED — 比较 entry sender 与 slot 自己保存的 sender，而非当前 entry 自比；复用 stale_slots_cannot_complete_or_discard_a_replacement。 / Compare against the original slot sender, including cancellation/drop.
## G4-05 — FIXED — SessionStore 新增 reserve_next_epoch；SQLite 用 Immediate transaction 和独立 reservation 表，文件后端复用 atomic_io 文件锁与原子 sidecar 写入；router 调用 owner 预留并移除无效读锁。增加两个真实后端的并发路由测试，覆盖显式 key、已有 epoch、不生成空 conversation。 / Reserve at the existing state owner without a router map or retiring old sessions.
## G4-06 — FIXED — Terminal notice、fallback、footer 统一组装 suffix，附到实际 controller 当前 tail；instant/voice 使用相同 suffix，不回填 overflow 全文。增加未达 threshold 与 overflow tail 的 notice 回归测试。 / Decorate only the live stream tail, preserving previously delivered pages.
## G4-07 — FIXED — 先根据正文是否为空选择 chunk body 或 summary.final_response，再添加 suffix；增加空正文 fallback 与已有正文优先的 instant 回归测试。 / Resolve the answer body before terminal metadata.
## G4-09 — FIXED — Final 与 iteration boundary 的 Done 分支也按现有 message id 去 cursor；仅 final settle 应用 side-answer 标记。增加两个 boundary 的 send/edit 测试，并更新普通流式答案的集成断言。 / Settle presentation even when no body bytes changed, without an extra send.
## G4-01-EDGE — FIXED — Sanitizer 在 code span 内消费完整 backtick run，并显式关闭匹配状态，不再回退 index；增加 fenced/inline embedded-backtick 回归测试。 / Ensure forward progress and resume tag stripping after the closing delimiter.
## G4-01-SPLIT-EDGE — FIXED — Reasoning splitter 同样一次消费完整 run 并关闭匹配状态；增加 reasoning_split_closes_code_spans_after_embedded_backticks。 / Preserve code text while resuming reasoning extraction after the closing delimiter.

## Verification / 验证

- `git diff --check` passed / 通过。
- Standalone `rustfmt --check --edition 2021 --config skip_children=true` passed for the ten other touched Rust files. The shared `reply_emitter/tests.rs` has formatting differences only in the concurrently added, out-of-scope GX6 voice test; G4 test blocks have no formatter differences. / 其余十个 Rust 文件格式检查通过；共享测试文件仅有其他批次新增 GX6 voice 测试的格式差异，未改动该测试。
- No cargo check/test/clippy/build, compilation, or runtime tests were run, as requested. Added regressions are unexecuted; FIXED records source implementation, not a verified runtime result. / 按要求未运行 cargo、编译或运行时测试；FIXED 表示源码修复已落实，不代表测试已通过。
- Graphify was queried before raw-read phases but unavailable: `graph file not found`; no graph/config changes were made. / 读取前已尝试 graphify，图不存在，未修改图或配置。
- No commit, stash, checkout, or history changes. / 未修改 git 历史。

## Trade-offs and scope / 取舍与边界

- Unused reservations can leave epoch gaps and durable reservation metadata; neither backend creates an empty conversation row. / 未使用的预留会留下 epoch 间隙及持久元数据，但不会创建空会话。
- Existing router behavior on store-reservation errors remains its warning/base-key fallback; this patch does not redesign error propagation. / owner 预留失败时保留原有告警及 base-key fallback，未扩展错误传播契约。
- Native-stream fast paths and unrelated batches were not redesigned or validated; no full build, hardware/channel QA, or cross-process stress test was performed. / 未重构 native-stream fast path 或其他批次，未做全量构建、真机通道 QA、跨进程压力验证。
