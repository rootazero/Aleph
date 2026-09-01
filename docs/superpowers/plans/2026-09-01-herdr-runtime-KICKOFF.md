# herdr 运行时移植 · 下次会话启动 prompt

把下面整段贴进新会话即可。它自足——不需要你先解释任何背景。

---

```
实施 herdr 运行时移植第 1 期。

## 先读这三份（按顺序）

1. docs/superpowers/plans/2026-09-01-herdr-runtime-phase1.md   ← 11 个任务，TDD 步骤齐全，这是你要执行的
2. docs/superpowers/specs/2026-09-01-herdr-runtime-phase1-design.md  ← 契约与理由
3. 背景与投资判断（不必读，除非你要质疑某个决定）：
   https://claude.ai/code/artifact/45b5b9ba-dfba-4285-a7db-fccc4b43d069

herdr 参考检出在 /Volumes/TBU4/Github/herdr（0.8.2 · Apache-2.0）。
注意它在 Github/ 下，不在 Workspace/ 下。

## 已经裁定的，不要重新讨论

- Aleph 的核心定位是「跑别人 agent 的运行时」（CLAUDE.md:23，R3 例外条）
- 禁止引入第二个 VT 实现，herdr 的 18,245 行不搬，能力不足一律扩容
  src/gateway/pty/screen/（CLAUDE.md:69，禁用清单）
- 左栏形态 = 并存·同列分段（上「运行中的 agent」，下「会话」，可拖分割比）
- terminal 工具面第 1 期只读，没有写入动词

## 怎么执行

用 superpowers:subagent-driven-development，从 Task 1 顺序做到 Task 11，
每个任务之间停下来让我审。

计划里有三处是特意设计成会红的步骤，不许跳过、不许"看起来对就过"：
- Task 2 Step 5：搬完 herdr 测试后对账条数，差额必须能逐条解释
- Task 5 Step 5：手动剪断 osc_title 接线，那条守卫必须变红
- Task 10 Step 4：手动往 TUI 塞一句 .sort_by，grep 守卫必须非零退出

## 第 1 期做完之后

下一步不是第 2 期的代码，是 0-A：

  0-A = 在 Aleph 里 pty.spawn 一个 claude，逐条对照 herdr 支持
        而 src/gateway/pty/screen/ 不支持的能力，产出一张缺口清单。
        （现有 2,686 行，用 vte crate 做解析器，只手写语义；
         已知缺口：不实现 hook/put/unhook ⇒ 无 DCS；osc_dispatch
         认 OSC 0/2 但不认 OSC 9;4 ⇒ 无 progress）

**第 2 期及以后的 spec 和 plan 都还没写，它们等 0-A 的结果。**
0-A 出了清单再写，顺序是：0-A → 第 2 期 spec → 第 2 期 plan → 实施。
不要在 0-A 之前给第 2 期任何工期数字——VT 扩容是整条路线上唯一
没有价格的格子。

第 2 期的范围（spec §8 已列）：VT 扩容 · 多会话 tab/split · tiling 布局 ·
workspace/worktree 模型 · PTY 写入动词 · manifest 远端热更新 ·
OSC 9;4 progress 接入。
```

---

## 给我自己的备注（不必贴给新会话）

- 三份交付物已 commit，工作区干净
- 记忆条目：`project-herdr-runtime-port`
- 第 1 期**不依赖** 0-A；第 2 期依赖。这是当初把第 1 期切出来的唯一理由
- 仍未做：herdr `tests/`（15 个目录）从没清点过——计划 Task 2 Step 1 是第一次真的数
