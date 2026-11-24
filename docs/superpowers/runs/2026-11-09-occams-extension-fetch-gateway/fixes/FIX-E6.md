# FIX-E6 — second-wave corrections / 第二轮修正

The first-wave fixes were applied by the orchestrator after a provider outage. This note describes the current diff; the second wave changes only the two rejected marketplace hunks and their focused tests. / 首轮由协调者在供应商中断后补齐；本轮仅修改被复审指出的两处及针对性测试。

## E6-01 — FIXED — Existing first-wave fix retained: `reject_unsafe_segment` rejects `.` so marketplace removal cannot target the cache root. / 保留首轮目录名校验修复。
## E6-02 — FIXED — Existing first-wave fix retained: directory digests use length-prefixed path/content records. / 保留首轮摘要记录分隔修复。
## E6-03 — FIXED — Existing first-wave fix retained: canonical plugin paths must remain under the marketplace root. / 保留首轮规范路径边界修复。
## E6-04 — FIXED — Unique staging paths retained; backup paths now use deterministic `.bak..-<plugin>` names, forbidden by the existing plugin-name validator. / 保留唯一暂存路径，备份改用不能作为合法插件名的确定性路径。
## E6-05 — FIXED — Valid requested slugs still require a matching cached origin; legacy invalid slugs may pull an existing cache, while fresh clones remain validated by `clone_github_repo`. / 有效来源仍检查 origin，旧无效 slug 可复用缓存，新克隆仍校验。
## E6-05-R1 — FIXED — Removed unconditional early slug rejection; the new local-Git regression test proves updated file contents arrive through cached pull and uncached invalid slugs still fail. Existing changed-origin rejection test retained. / 修复缓存兼容性，测试断言实际更新到达且无缓存时仍拒绝。
## E6-04-R1 — FIXED — Stable backup naming makes the existing stale-backup cleanup reachable again without deleting valid `.bak-<plugin>` installs; a focused test seeds a stale nonempty backup and verifies successful replacement with no residue. Existing update cleanup assertion now counts all entries instead of checking obsolete deterministic names. / 恢复旧备份清理并测试非空残留和最终目录数量。

## Validation / 验证

- Both changed Rust files parse successfully through `rustfmt --edition 2021 --emit stdout`; targeted `git diff --check` passes. / 两个修改文件通过 rustfmt 解析，定向 diff 空白检查通过。
- No cargo check/test/clippy/build was run, as requested; the new Rust regression tests have not been compiled or executed. No commits, stash, checkout, or unrelated source edits. / 按要求未运行 cargo，新增测试未编译执行，未提交或操作他人改动。

## Limits / 未覆盖

- Legacy randomly suffixed backups are not glob-deleted: their names may also identify legitimate plugins. The deterministic reserved name prevents recurrence, but older leftovers require operator inspection. Concurrent updates and crash recovery semantics were not expanded. / 不批量删除可能与合法插件同名的旧随机备份；新命名避免后续积累，历史残留需人工辨别；未扩展并发更新和崩溃恢复。
- Static inspection found two existing digest tests (`test_verify_plugin_integrity_match`, `directory_digest_uses_portable_path_separators`) still compute pre-length-prefix expected hashes. They were not changed because the second-wave prompt names only cache ordering and backup cleanup. / 静态发现两个原有摘要测试仍按旧格式计算期望值，本轮未越界修改，后续统一测试需处理。
