48 个测试全部通过。

# Module: scheduler

## Summary
- Files reviewed: 6
- Issues found: 2
- Issues fixed: 2

## Fixes

1. **`lane_state.rs:124`** 负数整数回绕 + i8 溢出
   - `(current_time - enqueued_at) as u64` → 时钟回拨时回绕为巨大 u64 值。加 `.max(0)` 防护。
   - `((wait_ms - threshold_ms) / 10_000) as i8` → 等待时间极长时（>1.27M ms），`as i8` 截断溢出为负值，`.min(10)` 无法修正。改为先 `.min(10)` 再 `as i8`。

2. **`lane_scheduler.rs:163-168`** 信号量膨胀 bug
   - `on_run_complete` 在 `guard=None` 时盲目调用 `add_permits(1)`。当 run 仅存在于递归追踪中（从未被调度，无 permit 被获取）时，会凭空增加信号量容量，逐步突破 `global_max_concurrent` 和 per-lane 并发限制。
   - 修复：加 `was_running` 检查，仅在 run 确实处于 running 状态时才手动释放 permit。

## Notes
- 代码整体质量较高，无 UTF-8 切片、lock poison、SQL 注入、`static mut` 等问题
- `try_schedule_next()` 中的 `std::mem::forget(permit)` + `ScheduleGuard` RAII 模式是正确的设计，但较脆弱——如果未来有人在 `forget` 和 guard 构造之间加入 early return，permit 会泄漏。可考虑添加注释标记此 invariant
- `sweep_anti_starvation()` 对整个 lane 设置 boost 而非 per-run boost，可能导致同 lane 内新入队的 run 也享受到不应有的优先级提升。当前是已知的设计简化，不属于 bug
