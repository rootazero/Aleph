# Phase7 v26 final independent acceptance / 最终独立验收

Date / 日期：2026-11-15. Same reviewer / 同一审查者：original run `7fbf11e2-8fef-4d5f-9369-0f68ab2d26ed`, revived final follow-up. This report is the authoritative output for the revived run; the requested output override takes precedence over the other proposed final-report filename. 本次仅写此报告，不改此前报告、源码、测试、检查器或其他文档。

## Verdict / 结论

| Decision / 判定 | Result / 结果 |
|---|---|
| Current-source QUALITY | **PASS WITH RECORDED P2 LIMITS / 保留 P2 限制，通过** |
| Terminal-evidence QUALITY | **PASS WITH INDEPENDENT CORRECTIONS / 经独立更正，通过** |
| Whole approved, bounded Phase7 | **PASS WITH RECORDED LIMITS / 批准边界内通过** |
| Next stage | **May begin Capability Phase8 specification/planning, subject to its own integration/security review and human approval. / 可进入 Capability Phase8 规格与计划阶段，仍须独立集成、安全审查及人工批准。** |

No remaining source P0/P1 or failed required execution threshold was substantiated. This is an engineering acceptance of the approved bounded Phase7 work, not a merge/release authorization or a claim that every Capability kind/backend/platform is complete. 没有据本次证据成立的残余源码 P0/P1，也没有实测失守的必需执行阈值；不授权 Phase8 实施、提交、推送或发布。

Two factual corrections are essential parts of this verdict and must travel with the handoff:

1. **Do not quote the archived coverage result as accurate production-only coverage.** Its `90.72401366593346%`, `19,385/21,367`, and `67 production files` are not independently valid descriptions. The retained LLVM coverage data does establish the unchanged 80% requirement after independent source/test separation; see the reproducible analysis below. 不采信错误的生产覆盖率数字；保留原收据不改，以独立重算闭合原 80% 门槛。
2. **Withdraw “no current server constructor” for `SimpleExecutionEngine`.** A real no-provider server branch constructs and mounts it at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569`. The simulation transcript-write issue remains P2, with actual server reachability explicitly recorded. 撤回无 server constructor 的错误判断；模拟模式的写失败问题没有修复。

These are corrections to evidence accounting and reachability, not new product exceptions or threshold changes. They do not make the original reports disappear. 此处没有为通过而添加产品例外或降低阈值。

## Scope and identity / 范围与身份

Read-only review: graphify ran before source reads. First query oriented Phase7 activation/constraints/special capabilities/events/audit/resolver seams; a later focused query confirmed `lazy_load.rs`, `resolver_tests.rs`, `ToolHandlerRegistry`, and `ZahirFacade` relationships. Graphify warned that skill 0.9.59 is newer than executable 0.9.29; no upgrade/install was performed. 查询只作导航，不作为行为证明。

Directly rechecked:

- Branch `fix/session-ui-readiness`; HEAD `d5e835d18ff6329ee31629548661224d33d579c9`.
- `artifacts-20261112-a/p7-strict-lint-workspace-v26-source.json`: **178 entries**, SHA-256 `0aca3c9595789c8a295edc48d901daa2b4d2dc77497a787025bba2fe9164c36c`; **178/178 current hashes match, missing 0**.
- **176 changed/untracked Rust paths**, all in the manifest; unaccounted changed Rust paths **0**. The manifest's 178 entries are not 178 changed Rust files.
- The three untracked integration files remain covered and hash-matched: `tests/session_ui_readiness.rs`, `tests/session_ui_readiness/support.rs`, `tests/session_ui_spend.rs`. The prior alleged coverage gap remains withdrawn.
- Supplemental JS: `interfaces/webchat/scripts/build-panel-tabs.mjs` = `8737677cf2fc1f568c845edb5202fbe78c10018b5a155cb56430338b27b30fde`; `interfaces/webchat/tests/panel-build.test.mjs` = `4d08a89dfeb766b34077fd0b7f9625211f73ac8c952527b3918d8283e993679d`. Both match source-validation v2.
- Runtime binary current SHA-256 directly matches `56427f1a9b300dc20b8dfd1e17915abfbaaffc5ea54afef5c1611d65eb52a9d8`; runtime receipt links to the actual server-build receipt and source-validation result also match. Browser driver and bounded-run driver hashes match their recorded identities.
- Original reports preserved: `p7-final-current-source-quality-v3.md` SHA-256 `129b826120ead5e122aa640e9549d2338d3e989d5a6458482838a328b8ed02ac`; `p7-final-current-source-quality-v3.7fbf11e2.md` SHA-256 `6a42172159745cd8d80d25b282b681808b2e445beddce5693df7772c5f7d073b`.

This follow-up carries forward the earlier source review only under the reverified exact source identity; it adds fresh targeted source checks and independent terminal-data analysis. It does not pretend every source line was freshly re-reviewed. 本次以精确源码身份承接此前语义审查，并新查关键接缝；不声称重新逐行审完所有文件。

## Four approved deliverables / 四项批准交付

The original `docs/superpowers/plans/2026-11-05-phase7-advanced-capabilities.md:57` through the four task sections, its success criteria at `:122`, the I3 design/plan, and the approved continuation plan define the bounded engineering acceptance. Source/runtime evidence supports all four:

| Deliverable / 交付 | Source and execution evidence / 源码与执行证据 |
|---|---|
| Task1 Activation & Constraints | Activation/constraints, lifecycle/admission and live policy facts are integrated. Current exact-source workspace passes; the real UI policy behavioral control executes baseline pass, RED failure, exact restoration, GREEN pass. Existing owner/generation authority remains canonical. |
| Task2 Special Capabilities | Ambient/custom handlers and UI/terminal ingress are integrated at the approved sources. The owned Close regression is fixed; new terminal work denial still permits owner-fenced cleanup. Real production-browser PTY attach/input/shell-output/resize/close succeeds. Renderer display ACK has its own behavioral control, separate from PTY RPC replies. |
| Task3 Event & Audit | Source event publication, audience/audit separation and lifecycle withdrawal have real integration evidence. The actual emitter sever executes RED and restored GREEN. Same-binary I3 verifies private delivery/withdrawal, audit body exclusion, subscription selection and stream sequencing for its listed cases. |
| Task4 Performance & Optimization | `src/capability/cache_policy.rs:1` implements bounded metadata retention with LRU/FIFO and exact visibility/revision keys; `src/capability/lazy_load.rs:15` captures the invalidation epoch before source calls. `src/capability/resolver.rs:75` always delegates lease validation to live Zahir; `:114` takes a nonwaiting work permit for lookup. Eighteen actual `resolver::optimization_tests` cases pass; the final release benchmark completes naturally. Metadata retention is not a second permission authority. |

The conservative new-test criterion was independently checked against the retained full workspace log and pre-Task1 baseline `d54cedf65255c51b140d392f5807fbe88cbff9ae`: **112 distinct passing case IDs**, each function name absent from baseline `src/capability`, no ignored case counted. Module distribution is activation 21, constraints 16, special tests 10, UI surface 6, events 29, audit 11, tool adapters 1, resolver optimization 18. This supports the original 100+ count; names are not a substitute for behavioral assertions. 原始 94 项窄清单保留；18 项真实嵌套 Task4 测试补足遗漏，不混入无关测试凑数。

## Terminal checks / 终态检查

All entries below are review of already completed commands, not commands rerun by this reviewer. 全部为既有终态执行的独立审阅，本 reviewer 未运行 Cargo、build、test、probe、server 或 browser。

| Gate / 门槛 | Independently checked evidence / 独立检查结果 |
|---|---|
| Strict lint | `p7-strict-lint-workspace-v26-receipt.json`: workspace/all-targets Clippy `-D warnings`, exit 0, 392.507 s, memory ALLOW, signals `[]`, final group `[]`. |
| Complete default-feature workspace | `p7-workspace-current-source-v6.log` was read in full by the analysis script. Splitting actual Cargo `Running` blocks and taking the last libtest footer per block gives **213 outer footers, 27,649 passed, 0 failed, 56 ignored, 0 measured, 0 filtered**. Three nested child footers (1 pass, 1 pass, empty) and nine custom harnesses are excluded. Receipt exit 0, 2200.686 s, memory ALLOW, signals/group empty. All 56 ignored are unexecuted, not passed. |
| Server build | `p7-final-server-build-v2-receipt.json`: exit 0, 385.601 s, memory ALLOW, signals/group empty; runtime binds the same resulting binary. |
| Fresh replacement verification | `p7-final-direct-verify-v3-result.json`: seven recorded checks true, source before/after equal. Fresh workspace/all-targets `cargo check` exit 0, 489.268 s, memory ALLOW, signals/group empty; its full log ends `Finished ... in 8m 08s`. Exact receipt/log identities were checked where stored. The coverage interpretation is independently corrected below; the old checker percentage is not adopted. |
| Panel build/tests | Final package v2, build ID `a0550c3d0d2b4c69`, identity tied to final build inputs. Node suite 44 passed, 0 failed, 1 optional generated-glue skip; that skip is not a pass. No branded verifier install, trust change, or threshold adjustment. |
| Release performance | `p7-performance-capability-final-v2-receipt.json`: actual `cargo bench --bench capability_resolver --offline --locked --jobs 1`, exit 0, 1072.319 s, memory ALLOW, signals/group empty. Full log contains six Criterion cases for 1024 tools/32 fields. |

Benchmark point estimates: warm lookup **438.67 ns**; invalidated cold lookup **7.3915 μs**; namespace describe reference **7.5071 ms**; fresh resolve **7.0745 μs**; LRU eviction/reload **7.5600 μs**; FIFO **7.5273 μs**. These differ in workload; no speedup ratio or invented release threshold is claimed. 结果为不同工作量的实测耗时，不推导虚构加速或门槛。

### Behavioral controls / 行为对照

- `p7-policy-behavior-final-v1/result.json` and all three actual logs: baseline 1 pass, RED **0 pass/1 executed failure**, GREEN 1 pass. RED has no compiler error; panic is `same real surface ingress must be reachable without the deny` at `src/capability/ui_surface_tests.rs:216`. Each process is joined with empty group; original/restored source hashes match. It proves the declared real ingress control, not every policy combination.
- `p7-emitter-behavior-final-v1/result.json` and logs: baseline 1 pass, RED **0 pass/1 executed failure**, GREEN 1 pass; no compiler error. RED assertion is `Non-ResponseChunk events should be emitted immediately in instant mode` at `src/gateway/event_emitter/tests.rs:217`. All phase signals/groups are empty and source restoration is exact. This supersedes the old emitter process-group evidence gap; the old invalid Policy compile-only RED remains historical.
- `p7-renderer-retained-replay-v1-result.json`: final-package baseline 1 pass → retained built mutant 1 fail with `each rendered frame must be acknowledged` → final-package GREEN 1 pass. Package WASM hashes were independently recomputed; only the mutant contains `capabilities.ui.ack.mutated`. All groups/signals empty. This is transparent retained-built-mutant replay, not a fresh mutant compilation or production compression proof.

### Actual production resources and PTY / 真实生产资源与 PTY

`p7-final-runtime-v1-result.json`, `p7-production-browser-final-v1/result.json`, the complete browser driver, browser receipt/log, and I3 receipt were inspected together.

The driver launches the candidate binary with `--webchat-dir` and a separate production static listener. Its HTTP proxy forwards to that listener and its WebSocket proxy forwards to the actual Gateway; it does not generate compressed QA responses. The resource requests themselves go directly to the static listener. Sixteen gzip/Brotli responses cover HTML, host/tab JS, snippet JS and host/tab WASM; every returned encoding and decoded hash matches disk bytes. This closes the old sidecar/Accept-Encoding evidence gap for the declared real-listener path. 不使用旧 QA 压缩代理 v6 冒充生产服务证据。

Real PTY evidence includes attach, typed octal `printf`, actual shell sentinel output, a **new successful resize after viewport change**, then Close; late post-Close resize is rejected with `-32099`. Browser result has no page/console errors, browser root exit 0 in 61.015 s, empty group; server exits code 0 and leaves no group. PTY cleanup is not inferred solely from successful UI clicks.

Same-binary I3 actual receipt hash `d54891e75851326d0857e6848e61f6b456bc613def1700c54a274d43015d70c6` matches runtime linkage: **25/25 listed checks** pass, root exit 0 in 48.673 s, server exit 0 and provider exit -15 both waited, both final groups empty. Cases include actual private source commit/withdraw once, guest/other-principal isolation, same-human-other-connection audit not functional display, body-free audit, Memory/PTY withdrawal, subscription separation and six connection sequence checks. Repeated named checks are not misrepresented as 25 different feature families.

Its explicit limits remain: loopback, controlled provider, no remote audit-read endpoint, no proof of whole `start_server` force-drop, bounded scrubbed output retention, no universal secret detector. Cached WebSocket import is process-local; no global installation is claimed.

## Coverage correction and original 80% gate / 覆盖率更正与原 80% 门槛

**Finding E1, P2 evidence-tool correctness: the archived v6 production-only percentage is inaccurate.** This does not conceal a production threshold failure, but quoting that number as verified would be incorrect. 原 checker 存在测量错误；本报告拒绝沿用原百分比。

Concrete mechanisms in `artifacts-20261112-a/p7-coverage-production-final-v6.py`:

- `source_ranges` at lines 132–190 treats every single quote as opening a character literal, so Rust lifetimes can hide later real `#[cfg(test)]` attributes. For example `src/capability/audit.rs:208` begins an embedded test module that the recorded exclusion map misses. Other misses include `src/capability/provider.rs:413` and the large test module at `src/capability/projection_host.rs:1839`.
- It always searches for a following `{` and does not stop at an out-of-line `mod ...;` or a field comma. Recorded ranges include `src/capability/mod.rs:46–291` and `src/capability/tool_adapters.rs:48–549`, excluding production bodies that are not test-only.
- The 67-file list contains `src/capability/census.rs`, which is declared under `#[cfg(test)]` at `src/capability/mod.rs:76–77`. Its recorded 1,475/1,565 covered/mapped lines are test-only, not production coverage.

The retained LCOV export is usable; mapped data and source identity were not lost. The read-only independent analysis in the appendix separates lifetime/character/raw-string/comment syntax, stops test-only declarations and fields at their actual delimiters, excludes actual test item bodies, and removes test-only census. No existing checker, receipt, threshold or source was edited. 这是对既有数据的独立算术审查，不是新建、重编译或重新运行覆盖率测试。

Independent result / 独立结果：**66 executable production files, 14,311 covered / 16,193 mapped production lines = 88.37769406533687%**, above the unchanged **80%** requirement; missing executable production files **0**. Additional checks found no `#[test]`/`#[tokio::test]` attribute outside the recomputed excluded ranges, and census was the only inventory file reached through an explicit parent `#[cfg(test)] mod ...;` declaration. The 67th mapped file was test-only census, not a missing production file. The declaration-only `src/tools/scoped/traits.rs` and unlinked/incompatible orphan `src/capability/session.rs` retain their explicit scope dispositions; neither is counted as an executable coverage pass.

Thus the **coverage requirement passes on the independently corrected retained data**, while the original checker's advertised percentage and inventory labeling remain a P2 tooling defect. Future automated acceptance should repair the parser/inventory and regenerate the summary; this report does not certify that checker as correct. 覆盖率门槛以独立更正的数据通过；后续自动验收应修复测量器并更新摘要，不能继续传播 90.724% 或 67 production 的错误说法。

## Preserved source findings and corrected reachability / 残余问题与可达性更正

**P2: installed-service write failure in the simulated engine is real and reachable in server simulation mode.** `src/gateway/execution_engine/simple.rs:147` and `:235` ignore `emit_event` errors; `:265` can emit RunComplete. Unlike the previous report's search conclusion, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569` constructs it in the explicit no-provider branch, `:1572` erases it into `ExecutionAdapter`, and `:1618` supplies the adapter to the actual fallback `GatewayContext`. The old “public library only/no server constructor” reachability claim is withdrawn.

This changes the factual boundary, not the approved metadata contract. `simple.rs:89–101` checks canonical session metadata preparation before state transition. The continuation plan at `docs/superpowers/plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md:155–157` requires checked metadata preparation in both real/simple engines; it does not establish a durable transcript guarantee for simulated model output. The server explicitly advertises Simulated mode; these final production runtime proofs use the full engine with a controlled provider. Therefore the actual installed-service transcript-error concern remains **P2**, not a demonstrated breach of this Phase7 production admission/metadata gate. 不把缺失服务的兼容路径与已安装服务的写失败混为一谈；不声称此问题无害或已修复。

Before advertising durable simulated transcripts or promoting this path to such a guarantee, handle failed UserMessage/AssistantMessage writes through terminal failure and settled run/agent cleanup, with owned-service failure tests that do not poison the global singleton. This review neither implements that change nor creates a silent fallback policy.

Other preserved limitations:

- Markdown metadata writes are serialized per store instance/process (`src/memory/store/markdown/store_impl.rs:69` and write sites); there is no cross-instance/cross-process lock. The real existing `test_append_transcript` at `:1178` covers concurrent transcript/tool append, counts, SQLite/list and FTS, so the old missing-regression P2 remains withdrawn.
- PTY operation error presentation remains P2; its operation replies are distinct from renderer display ACK. The SPEC follow-up's withdrawal of the old Task5 ACK P1 remains justified at this approved boundary.
- Configured raw targets retain canonical admission. Unconfigured legacy raw behavior is unchanged under the approved contract; this report does not invent a requirement to block it.

## Phase8 entry and exclusions / Phase8 入口与不承诺事项

The approved four tasks, 100+ distinct new passing tests, unchanged-source regression suite, semantic controls, actual bounded production integration, measured coverage above 80%, release benchmark and user-authorized replacement verification support bounded Phase7 completion. The historical SPEC/QUALITY documents' runtime PENDING statements describe their earlier evidence state; this final review supersedes those pending execution dispositions only for the current linked terminal receipts. 原失败与纠正报告继续保留，未被覆盖或篡改。

Capability Phase8 may start specification/planning with integration/security review and human approval before implementation. The continuation plan's NSH trajectory is separate: `p7-nsh05-compatibility-audit.md` freezes official v0.5 evidence; ADR/parity/two-week dual-run remain prerequisites to native NSH migration/Tauri removal, not hidden Capability Phase7 gates. No native NSH migration, Tauri removal or Phase8 implementation is accepted here.

Not done / 未做与未覆盖：no new build/test/probe/server/browser execution; no source/test/checker/config edits; no commit/push/publish/install; no global trust change; no nested agents. No all-feature/all-platform claim, no Linux/Windows native verification, no ignored-test execution, no optional generated-glue pass, no remote deployment or same-port candidate routing verification, no every-kind/backend completion, no external-effect exactly-once/Safe Replay/universal scheduler claim. Original “four clean commits to main” timeline is not represented as completed; this is uncommitted engineering acceptance and no merge authorization.

## Reviewer validation record / 本次审查验证记录

Executed only graph navigation and read-only analysis: `graphify query`; Python SHA-256/current-git inventory checks; full-log parsing of workspace, behavioral controls, renderer, Panel, benchmark and fresh-check evidence; runtime receipt/package/binary linkage; baseline `git grep` new-name verification; and the appendix's retained-LCOV calculation. The appendix was extracted from this report and executed successfully, reproducing its exact totals. 所有 Python 执行均为读取已有文件并输出审查结果。

Final checks: original two report hashes unchanged; all 16 recorded decoded resource hashes still match final disk assets; staged files **0**. `git diff --no-index --check /dev/null <this-report>` emitted no whitespace diagnostics (exit 1 denotes the added-file diff). No tests were added or modified. An initial relative renderer-package lookup used repository root instead of artifact root and failed; resolving the receipt's relative path against `artifacts-20261112-a` then verified all three WASM hashes successfully. No execution gate was rerun or changed. 最终仅新增本报告，原报告与收据保持原样。

## Reproducible read-only coverage analysis / 可复算的只读覆盖率分析

Run from repository root using the retained data; the code below reads only and prints totals. It changes no checker or artifact. 从仓库根目录读取原 LCOV 与当前哈希匹配源码；不运行 Cargo、不生成 coverage、不写文件。

```python
from pathlib import Path
import json, re
root = Path.cwd()
base = root / '.superpowers/sdd/2026-11-12-session-ui-readiness-and-phase7-continuation/artifacts-20261112-a'
data = json.loads((base / 'p7-coverage-production-v6-result.json').read_text())

def mask(text):
    out = list(text)
    pos = 0
    def blank(start, end):
        for index in range(start, min(end, len(text))):
            if out[index] != '\n':
                out[index] = ' '
    while pos < len(text):
        start = pos
        if text.startswith('//', pos):
            end = text.find('\n', pos)
            pos = len(text) if end < 0 else end
            blank(start, pos)
        elif text.startswith('/*', pos):
            depth = 1
            pos += 2
            while pos < len(text) and depth:
                if text.startswith('/*', pos): depth += 1; pos += 2
                elif text.startswith('*/', pos): depth -= 1; pos += 2
                else: pos += 1
            blank(start, pos)
        else:
            raw = re.match(r'(?:br|cr|r)(#{0,20})"', text[pos:])
            if raw:
                end_token = '"' + raw.group(1)
                end = text.find(end_token, pos + raw.end())
                pos = len(text) if end < 0 else end + len(end_token)
                blank(start, pos)
            elif text[pos] == '"':
                pos += 1
                while pos < len(text):
                    if text[pos] == '\\': pos += 2
                    elif text[pos] == '"': pos += 1; break
                    else: pos += 1
                blank(start, pos)
            elif text[pos] == "'":
                char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", text[pos:])
                if char: pos += char.end(); blank(start, pos)
                else: pos += 1  # Lifetime, not a character literal.
            else:
                pos += 1
    return ''.join(out)

records = {}
current = None
for line in Path(data['export']).read_text().splitlines():
    if line.startswith('SF:'):
        current = str(Path(line[3:]).resolve().relative_to(root))
        records[current] = {}
    elif line.startswith('DA:') and current:
        number, count, *_ = line[3:].split(',')
        records[current][int(number)] = int(count)
    elif line == 'end_of_record':
        current = None

covered = total = files = 0
for name in data['expected_production_files']:
    if name == 'src/capability/census.rs':
        continue  # Parent module is explicitly cfg(test).
    text = (root / name).read_text()
    clean = mask(text)
    ranges = []
    pattern = r'#\[\s*cfg\s*\(\s*(?:test|all\s*\(\s*test\s*,\s*unix\s*\))\s*\)\s*\]'
    for match in re.finditer(pattern, clean):
        start = match.end()
        while True:
            attr = re.match(r'\s*#\[[^\]]*\]', clean[start:])
            if not attr: break
            start += attr.end()
        field = bool(re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?[A-Za-z_][A-Za-z_0-9]*\s*:', clean[start:]))
        terminator = re.search(r'[;{,]' if field else r'[;{]', clean[start:])
        if not terminator: continue
        end = start + terminator.start()
        if clean[end] == '{':
            depth = 1
            end += 1
            while end < len(clean) and depth:
                if clean[end] == '{': depth += 1
                elif clean[end] == '}': depth -= 1
                end += 1
        else:
            end += 1
        ranges.append((text.count('\n', 0, match.start()) + 1,
                       text.count('\n', 0, end - 1) + 1))
    assert name in records, name
    used = {line: count for line, count in records[name].items()
            if not any(start <= line <= end for start, end in ranges)}
    files += 1
    total += len(used)
    covered += sum(count > 0 for count in used.values())
print({'files': files, 'covered': covered, 'mapped': total,
       'percent': 100 * covered / total, 'threshold80pass': covered / total >= .8})
```

Expected measured output / 本次实算输出：`{'files': 66, 'covered': 14311, 'mapped': 16193, 'percent': 88.37769406533687, 'threshold80pass': True}`. This is a review calculation for these frozen sources, not a new general-purpose Rust parser or a certified future checker. 此算法用于本批冻结源码的独立审查，不声称通用 Rust parser 完备性。
