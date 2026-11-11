# FIX-H4 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- H4-03: FIXED
- H4-02: FIXED
- H4-04: FIXED
- H4-05: FIXED
- H4-01: FIXED
- <ID>: SKIP(no location)
- H4-06: FIXED
- H4-07: FIXED

## H4-NEW-01 — SKIPPED — Current source already persists before runtime creation/registration; added an ordering regression test, not an unregister call. / 当前源码已先持久化、后创建并注册运行实例；新增顺序回归测试，不添加 unregister。

Evidence / 依据: `handle_create` returns `INTERNAL_ERROR` and removes the inserted config on save failure before reaching `create_channel_from_config` or `registry.register`. The reported pre-save registration is absent in the current source. / `handle_create` 保存失败时移除插入的配置并返回 `INTERNAL_ERROR`，尚未到达运行实例创建与注册；当前源码不存在复审所述的保存前注册。

Validation / 验证: `git diff --check` and a standalone source-order assertion; cargo check/test/clippy/build were not run as instructed. The added test is structural, not a runtime disk-failure test. / 执行差异空白检查及独立源码顺序断言；遵照要求不运行 cargo。新增测试为结构断言，未验证真实磁盘故障。
