# 审计工具 / Audit tooling

本目录是断线审计使用的两个脚本的快照 / This directory snapshots the two scripts the severed-wire audit used.

- `sweep2.py` — 仓库级「已定义 − 被消费」扫描器。逐文件剥离 `#[cfg(test)]` 块，建立 *标识符 → 文件* 倒排索引，报告模块树中「除定义文件外没有任何生产引用」的 `pub` / `pub(crate)` 项。 / A repository-wide "defined − consumed" sweeper: it strips `#[cfg(test)]` blocks per file, builds an inverted *identifier → files* index, and reports every `pub` / `pub(crate)` item of the module trees that no production code mentions outside its own definition file.
  - 用法 / Usage: `python3 sweep2.py --root <repo> --modules src/generation,src/goal,src/group_chat`
- `batch1c.js` — 审计子代理的工作流脚本（`pi-subagents`，agent `sev-auditor`，model `minimax-cn/MiniMax-M3`）。每条切片一个只读子代理；子代理被要求分批 grep、增量写报告、并且**不要**调用压缩工具（压缩失败曾烧掉一个子代理的整轮预算）。 / The auditor workflow script (`pi-subagents`, agent `sev-auditor`, model `minimax-cn/MiniMax-M3`): one read-only child per slice. The children are told to batch their greps, write the report incrementally, and **not** to call the compress tool — three failed compress calls once burned a whole child's budget and timed it out.

第一个版本的扫描器有缺陷：定义正则少了 `re.MULTILINE`，`^` 只匹配文件开头，于是它一个定义都没找到 —— 一道「绿灯但看不见任何东西」的守卫。修正版是 `sweep2.py`。 / The first sweeper was defective: its definition regex lacked `re.MULTILINE`, so `^` matched only the start of the file and it found zero definitions — a green-but-blind guard. `sweep2.py` is the corrected version.
