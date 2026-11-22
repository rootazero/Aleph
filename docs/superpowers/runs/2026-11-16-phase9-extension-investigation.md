# Phase9 extension investigation / Extension 长跑调查

**State / 状态:** bounded Core lib gate closed by the final matrix below; the earlier checkpoints retain their original failed or pending outcomes. / Core lib 有界闸门已由文末最终矩阵关闭；下文早期检查点的失败与待验状态作为历史保留。

## Observed chain / 实际因果链

The handoff JSON contains 33 long-running notices for 22 distinct tests. The current inventory includes hooks, MCP, service operations and projection tests in addition to the original cache/context nine. The investigation does not narrow the acceptance set to the old nine. / 交接记录中 33 次长跑提示对应 22 个不同测试，验收集合不限于旧九项。

A diagnostic replay of the retained Phase8 executable reached the extension group and reproduced multiple environment-guard waiters. A macOS thread sample showed those waiters acquiring the `ALEPH_HOME` mutex in `src/utils/paths.rs`, while the holder was the cache update test. Its worker was running skill companion-file regex scanning through `SkillSystem::rescan_dirs`, `parse_skill_file`, `scan_companion_files` and `scan_content`. Open-file evidence showed traversal under the account's actual `.agents/skills` tree. This is evidence of ambient filesystem work while holding the shared fixture guard, rather than evidence of a recursive mutex deadlock. / 栈采样与打开文件共同表明，持锁测试正在扫描真实账户的技能目录，其他夹具等待同一环境锁；尚无递归锁死证据。

`DirectoryScanner::new` resolves `.agents` independently of `scan_claude_dirs` and `claude_home_override` (`src/discovery/scanner.rs`). `IsolatedAlephHome` redirects only `ALEPH_HOME`; these discovery fixtures therefore still see the account HOME. The existing Claude-only census explicitly excludes `.agents` (`src/discovery/home_scan_census.rs`). Its prior green result does not assert this boundary. / `.agents` 不受 Claude 扫描开关控制，而旧夹具仅隔离 ALEPH_HOME，现有 census 的边界也明确不包括 `.agents`。

The diagnostic full-binary replay was stopped after gathering the stack/open-file evidence. It is neither a successful full-suite run nor a new source acceptance run. A single exact cache test completed in 0.18 s in the retained executable; that result is diagnostic only. / 旧二进制诊断在证据取得后停止，单项 0.18 秒通过仅用于区分路径，不充当当前源码验收。

## Repair boundary / 修复边界

Use a test-only discovery fixture that redirects both HOME and ALEPH_HOME through the existing `HomeEnvGuards` fixed lock order. Keep the narrower `IsolatedAlephHome` for tests that only need Aleph state isolation. Migrate the extension fixtures that actually construct discovery managers; spill-output and standalone parser fixtures need not acquire the second lock. Production discovery, skill admission and regex rules remain the subject of their existing contracts. / 复用既有双锁顺序，仅迁移真正构造 discovery 的测试夹具，避免让纯解析或 spill 测试无谓占用 HOME 锁。

The trade-off is lower concurrency among those discovery tests. A new behavioral regression exercises `.agents/skills` discovery with Claude scanning disabled and requires the resulting roots to remain under the fixture. RED and GREEN must be actual terminal test results. / 代价是这组测试并行度下降；回归用例验证关闭 Claude 扫描时 `.agents/skills` 仍可发现且不越出夹具，必须取得真实 RED/GREEN 终态。

## Behavioral RED / 行为回归 RED

The fresh regression `utils::paths::tests::discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled` terminated with exit 101: 0 passed, 1 failed, 22,421 filtered out. Its HOME equality assertion failed before discovery could traverse the account tree. Compilation completed in 9m 05s and the test itself in 0.62s; the command receipt elapsed 552.14s. This is an executed assertion failure, not a timeout. / fresh 回归已取得真实 RED，在扫描前即发现 HOME 仍是账户目录；不能将该失败归为环境阻塞。

Raw receipt: `phase9-test-output/20261116T053403.173012Z.json`, with matching `.stdout.log` and `.stderr.log`. The corresponding memory gate exited 0/ALLOW and its separate raw logs are preserved. / 命令、内存门、退出码及分离日志均已保存。

The repair replaces the deliberate alias with a combined-guard fixture and migrates discovery constructors in seven extension test files. The final process runner redirects HOME and ALEPH_HOME into distinct owned roots, leaves USERPROFILE untouched on Unix, and retains explicit Cargo/Rustup cache locations. Production discovery behavior is unchanged. / 最终运行器使用独立 HOME 与 ALEPH_HOME，Unix 不额外注入 USERPROFILE，生产发现行为不变。

The first repaired-fixture run (`phase9-test-output/20261116T054552.622590Z.json`) still failed: the HOME equality and `.agents` discovery checks passed, but the assertion over *all* returned roots also included legitimate project roots. The regression now explicitly disables project scanning, so its all-roots assertion covers account-scoped discovery only. This was an overly broad test assertion, not evidence that project roots must move under HOME. At that checkpoint, the corrected GREEN and complete matrix were still pending; the terminal results are recorded in Final A closure below. / 当时修正后的 GREEN 与矩阵仍待终态；最终结果见文末。

## First matrix and fixture compatibility / 首轮矩阵与夹具兼容边界

All 22 slow filters and six positive controls reached terminal outcomes: the controls passed 6/6; the slow filters had 11 PASS and 11 FAIL. The failures are assertion/panic evidence, not BLOCKED. The full serial suite subsequently made steady progress to about 19,000 cases and recorded 33 FAILED names before its 600s window ended; it did not emit a suite footer. A `PermissionError: [Errno 1] Operation not permitted` in the runner's final process-group probe prevented that invocation's normal JSON receipt. The original failed matrix and raw logs remain preserved. / 28 个定点调用均有终态；全量运行既有明确失败，也缺少整体终态，不能用六项正向控制掩盖。

The first full run's raw output is `phase9-test-output/20261116T060403.530064Z.stdout.log` and matching stderr/memory logs. Its supervisor error is retained in the local execution workspace as `matrix-28.stderr.log`; `2026-11-16-phase9-test-matrix.json` records the missing terminal JSON. A subsequent process census found no running target test command and no process holding that stdout file; this later observation does not turn the failed cleanup probe into a successful owned-cleanup receipt. / 进程后查未见遗留不等同于原清理步骤成功。

Four independent diagnostic reruns of the already-built binary established the remaining fixture boundaries:

- The Seatbelt witness rejects HOME beneath the profile's temporary-directory grant. Its protection must remain active. / HOME 不可放在测试本应拒绝访问的 temp 授权树中。
- The runtime ledger test clears HOME and ALEPH_HOME; injecting USERPROFILE on Unix accidentally supplies a fallback and defeats that fixture's intended absence. / Unix 不应新增 USERPROFILE 兜底。
- The existing save guard rejects HOME/.aleph/config.toml even for a moved HOME; put ALEPH_HOME in a distinct owned directory without weakening the guard. / 使用独立目录，保留防误写断言。
- The Unix socket test failed with `path must be shorter than SUN_LEN`; the runner's long TMPDIR made its nested socket path exceed the macOS limit. / 使用短的独占 TMPDIR。

Those diagnostics are stored as `failure-families.json` and `failure-family-*.stdout.log` in the execution workspace and do not count as Cargo acceptance. One earlier diagnostic attempt was interrupted by the outer tool's 60s limit while Cargo was recompiling; it ran no test body and is not a verdict. No owned test/Cargo process remained after that interruption. / 诊断与实际验收分列，工具超时不充当源码结论。

The revised runner creates an owned HOME child in the account directory, a separate short temporary root for ALEPH_HOME/TMPDIR, and removes both. Its trade-off is requiring a writable account directory. It retains the 600s window for each individual filter and uses a documented 1200s supervised window for the full suite, whose first attempt demonstrated continuous progress beyond the prior extension stall. All prior failed/partial evidence remains historical. A real full-suite terminal summary and passing assertions are still required. / 新夹具与窗口尚待 fresh 验证，不能追认旧运行通过。

## Local raw evidence / 本地原始证据

The execution workspace `.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/` retains `baseline-full.json`, `baseline-full.stdout.log`, `baseline-full.stderr.log` and `baseline-full-sample.txt`. These are diagnostic artifacts, not independent reviewer verdicts. Fresh Cargo runs and their classifications are recorded separately in `2026-11-15-phase9-test-closure-receipt.md` and `phase9-test-output/`. / 本地 workspace 保留诊断原件；fresh Cargo 回执另列，不混充独立审查或最终通过证据。


## Final A closure / A 最终闭合

The fourth matrix reached a terminal PASS on the repaired fixture and runner source: `2026-11-16-phase9-test-matrix-verified.json`, 53 focused calls PASS plus one full lib call PASS. The outer lib footer is **22,402 passed; 0 failed; 20 ignored; 0 filtered out**, 656.84 seconds. Nested subprocess test footers are not added to that count. No new test skips were introduced. All six Phase8 positive controls and the complete latest 22-name slow inventory are included. / 53 个定点调用与全量 lib 均取得终态，旧失败、超时和中断证据保留。

A later test-only tee repair initially failed compilation (`E0382`, moved `cli`); the interrupted closure matrix is retained separately. Its cancellation also exposed nested-owner teardown: an outer SIGKILL orphaned the inner Cargo process group. That specific verified group was terminated, and a real nested subprocess RED/GREEN test now requires cancellation to let the inner owner drain first. Runner checks: 12/12. The final verified matrix used the corrected runner and records its SHA-256 and retained lock SHA-256. / 编译失败与 runner 取消缺陷均有独立证据，不由旧绿灯覆盖。

This closes the bounded alephcore lib test gate only. It does not validate the new NSH host, live Gateway, debug parity, release performance, or the entire workspace. Existing ignored cases remain recorded limits. / 该结果仅关闭 Core lib 测试闸门。
