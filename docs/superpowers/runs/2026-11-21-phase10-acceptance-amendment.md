# Phase10 acceptance amendment / Phase10 验收修订

## Decision / 决定

On 2026-11-21 the operator explicitly approved **Quit (`close_ms`) median ≤ 1.30×** and then separately approved **Quit p95 ≤ 1.30×**. All other latency gates remain median ≤ 1.10× and p95 ≤ 1.20×; RSS median remains ≤ 1.10× and thread median delta ≤ 2. / 操作员先明确批准退出中位数上限 1.30×，随后单独批准退出 p95 上限 1.30×；其余门槛不变。

This is a performance trade-off, **not an optimization** and not proof of the cause of Quit latency. Earlier subagent statements attributing the gap to JS VM cleanup, assigning 80% confidence, or asserting a 200–400 ms blocking close animation were unverified hypotheses. A sequential call chain does not establish animation duration; observed diagnostic exits were much shorter. No such claim is required for this approval. / 这是性能取舍，不是提速。此前关于 JS 清理主因、80% 置信度或关闭动画阻塞 200–400 ms 的表述没有测量支持，予以撤回；同步调用顺序不能证明动画耗时。

## Evidence / 证据

- Re-evaluate the **same retained candidate and all 80 rounds**, not the reverted tray candidate or a best-of selection. Original [matrix](../../../.phase10-evidence/g7-official-matrix.Ji0duOcq/matrix.json) remains `VALID / NOT_ACCEPTED` under its original contract. / 使用原保留候选的完整 80 轮，不选择最佳样本或已撤回候选，原始失败判定不变。
- New [amended assessment](2026-11-21-phase10-acceptance-amendment.json): `ACCEPTED_UNDER_AMENDED_CONTRACT`. Quit median `41.298 → 51.794 ms`, ratio `1.2541527434742639`; p95 `57.849 → 72.014 ms`, ratio `1.2448616224999398`. Both satisfy only the newly approved Quit limits. All other original gates already pass. / 新评估仅应用新退出门槛，其余指标按原门槛通过。
- Original sampling is unchanged: Tauri 40 then NSH 40, first 10 warmups excluded per surface, 30 measured, p95 rank 29, real 100 ms resource sampling, original native Quit and observed settlement boundary. No fresh native timing run was performed for this amendment. / 采集顺序、热身剔除、分位数、采样与退出边界均未改；本次不是重新采样。
- Read-only identity audit rechecked 23 source files, three binaries, 80 distinct functional passes/natural exits/observed settlements/unforced Gateway fixture cleanups and 320 byte-exact HTTP asset files. The new audit equals the retained [restored-candidate audit](../../../.phase10-evidence/g7-tray-candidate.IThQBKIR/restored-candidate-audit.json). / 重新核验源码、二进制、终态与实际资产，结果与恢复候选审计一致。
- Reproducer: `python3 .phase10-evidence/phase10-amended-acceptance.py`. It calls the existing audit and the unchanged `qa/nsh_panel_parity.py` functions, requires exact original-summary/failure-set replay, then overrides **only** the two Quit comparisons. Boundary checks reject non-Quit failure waiver and ratios above 1.30. Script SHA-256 and original reducer SHA-256 are recorded in the assessment. / 重放器复用原统计实现，并检查边界；不是修改旧 reducer 的全局门槛。

## Limits / 限界

Physical multi-display topology remains user-deferred and **NOT TESTED**. Windows/Linux, release signing/notarization/distribution, switching the default desktop, upstream changes, and Tauri removal are not accepted by this amendment. The retained corpus is local QA evidence; it is not a release package. / 多屏仍未测试，其他平台、发布、默认切换、上游改动和移除 Tauri 均不在本次授权内。
