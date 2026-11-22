# 从 main 接续 Phase8 / NSH 的新会话 Prompt

> Copy the block below into the next session. / 将下方正文复制到新会话。本文与所链接计划构成完整入口；不需要旧会话上下文。

---

请从当前仓库的**本地 `main`** 接续 Aleph。先用 `git rev-parse --show-toplevel` 定位仓库，不写死工作目录。本次 Phase7 结项已经由上一会话提交并合并到本地 main；先验证实际 HEAD 和交接提交祖先关系，不 reset、不拉入无关远程提交、不清理已有分支/worktree/证据。按当前用户要求从 main 开始，不套用全局 origin/dev 或新 worktree 默认。

## 先完整阅读 / Required reading

1. 根 `AGENTS.md`、`docs/reference/SUBSYSTEM_ROUTING.md` 及相关红线/子系统入口。
2. **NSH 总纲：`docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md`，全文，尤其第六节 Phase0–6、第八节已决策/未覆盖事项和 ADR 前提。不要只读本 prompt 的摘要。**
3. `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md`——按 Task1→Task5 执行。
4. `docs/superpowers/runs/2026-11-15-phase7-acceptance-and-main-handoff.md` 及其中可随 Git 保留的 `2026-11-15-phase7-closeout/` 审查、覆盖率、NSH 审计和源码清单。
5. `docs/superpowers/plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md` 的后续阶段表、`docs/superpowers/plans/2026-11-05-phase7-advanced-capabilities.md` 与 `docs/superpowers/plans/2026-11-07-phase7-i3-production-integration.md`。
6. 按需完整读取 NSH Phase0/API/Phase1/Phase2/Phase3/UI-debt/Phase4 旧计划及 `desktop/nsh-spike/FINDINGS.md`，将历史状态与实际 main 区分。

## 当前已验收事实 / Accepted baseline

- Phase7 最终独立结论 **PASS WITH RECORDED LIMITS**；Source QUALITY **PASS WITH RECORDED P2 LIMITS**，终态证据 QUALITY **PASS WITH INDEPENDENT CORRECTIONS**。批准范围无剩余已证实 P0/P1，也无未闭合 required gate。不要重开旧 NOT_ACCEPTED 账本，也不要把它扩大成每种 capability/backend 或发布验收。
- 原基线 `d5e835d18ff6329ee31629548661224d33d579c9`；v26 manifest 178 Rust paths，SHA-256 `0aca3c9595789c8a295edc48d901daa2b4d2dc77497a787025bba2fe9164c36c`。该 SHA 是旧冻结清单，不是新会话 HEAD；当前提交号用 Git 实测。
- strict Clippy `-D warnings`、完整 workspace、正式 server build、替代验证的 fresh workspace/all-targets check 全部自然 exit0；外层测试 **27,649 passed / 0 failed / 56 ignored**，213 outer footers；3 nested probes 和9 custom harnesses 不混入总数。
- **有效生产行覆盖率为 66 个可执行生产文件、14,311/16,193 = 88.37769406533687%，missing0，原 80% 阈值不变。** 原脚本误判 Rust lifetime/`cfg(test)` 边界并计入 test-only census，旧90.724%、19,385/21,367、67生产文件标签已撤回；原 LLVM数据可用，独立 appendix 已由父会话复算。旧解析器待修，不可继续用它制造新 PASS。
- 当前 binary SHA `56427f1a9b300dc20b8dfd1e17915abfbaaffc5ea54afef5c1611d65eb52a9d8`；真实静态服务16个 gzip/Brotli HTML/JS/WASM 响应 decoded hash匹配，route-only proxy；真实 PTY attach/input→shell output/resize/close，关闭后 late resize拒绝；同 binary I3 **25/25 listed scenarios**。
- Policy/emitter 对照各为真实 baseline1pass→实际编译执行RED1fail→精确恢复GREEN1pass；renderer 是 retained built ACK mutant 重放，不是 fresh mutant build。四项交付 Activation/Constraints、Special Capabilities、Event/Audit、Performance/Optimization 已对账；112项 distinct新增当前通过用例；release benchmark warm lookup438.67ns、fresh resolve7.0745μs，不发明 speedup/release阈值。
- Node44pass/0fail/1optional glue skip；56ignored与该skip均未执行、不算通过。未证明 all-features、原生Linux/Windows、远程部署、内置同端口候选路由、所有backend、external-effect exactly-once、SafeReplay或universal scheduler。

## 不得丢失的合同与 P2 / Contracts and P2s

- 已批准合同 A：真实 `chat.send`/`agent.run` 每个 addressed-key 成功响应前提交 canonical metadata；合法 no-run、后续queue/spend拒绝、取消/timeout/caller断线可留行；接受一次存储延迟。只用既有 SessionStore/request_scope/AgentInstance 返回值，没有 ready map/latch/第二store；不再重问 A。
- `SimpleExecutionEngine` **真实 server 可达**：`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569`构造、`:1572`擦除为ExecutionAdapter、`:1618`装入无provider fallback GatewayContext。`src/gateway/execution_engine/simple.rs` 忽略 installed SessionService 的 user/assistant emit失败仍为P2、未修。旧“无server构造点”说法错误已撤回；合同A不是模拟 transcript持久性承诺。
- metadata 文件写入的 mutex 仅 per-store instance/process；无跨实例/跨进程保证。PTY错误呈现及 coverage parser保留P2。若下一阶段修，先定具体失败合同及真实终态/资源清理测试。
- `capabilities.ui.display/ack/input` 是 renderer ACK 合同；PTY attach/input/resize/close没有额外已批准UI-visible ACK schema。D-T=A不等于独立stdin/stdout/stderr streaming API。
- configured raw target必须 canonical fail-closed；unconfigured legacy raw行为按原批准保持。不得为“修P1”顺手扩大合同或做第二registry。
- 已批准并合入的公开字段变化：`PolicyDenial::PrincipalMismatch.actual: Box<OwnerRef>`；`GrantChange::Created.grant: Arc<Grant>`。不是新的待批准项。

## 下一步目标与 NSH 总纲 / Next phase and NSH master

Capability Phase8 的真实名称是 **Integration & Security**。尚无已批准细案：先按新计划形成当前源码驱动的 threat/owner/ingress/consumer/teardown矩阵、真实spec与具体实施plan，完成独立SPEC/QUALITY审查，再将可审查的设计交人批准。不要编造 Phase9 名称、工期或未授权接口。已选串行 native实施+独立审查方式继续沿用，不重复询问执行方式。

NSH为**独立轨道**，不与Capability Phase8共用一个“已完成”标志：

- 总纲仍是**草稿、0.4 API历史**；新版目标冻结为 **v0.5.0**，release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`，tag object `729ea4c6810112160e890684e8a8d34528ec6da9`。旧0.4 spike不能当0.5本地编译/宿主运行证明。官方0.3/0.4/0.5均未进入永久兼容记录。
- 新版替换窗口API为 `WindowRef`/`<Window>`/window kinds；核实同版CLI/crates/runtime/platform AppView/JS与TS类型、host channel/optional Tokio handle、次级窗口/退出、media grants和activation policy。Rust1.97，macOS14+，Windows10 1809+，LinuxGTK4.14+/WebKitGTK6.0(2.40+)。
- 交接时 `desktop/nsh-shell` 已存在，`interfaces/nsh-desktop` 不存在；先核对实际main和历史合入工作，不从Phase0盲目重做，也不凭目录存在说全链路完成。
- Phase0=0.5兼容门槛；Phase1=shared client/protocol+hello/events/session list/supervisor；Phase2=window/tray/hotkey/deeplink/update/daemon/notify/single-instance；Phase3=canonical真实聊天；Phase4=Canvas/Settings/Browser/Panel/PhoneTablet parity；Phase5=独立PluginRuntime；Phase6=移除Tauri。
- 完全替换方向已确定，不走Tauri混合兜底；但**Phase4 parity完成+两周双运行之前不得删除Tauri/WebChat/workspace members**。Swift截图/语音/摄像头与i18n按总纲独立，不伪报本阶段验收。GPUI替代轨道继续deferred。
- 查实际ADR索引核实状态，特别006 WASM/008信任签名，以及总纲列出的003/004/005/007/009；不能推定Accepted。核实NSH原生UI与当前R2/Leptos宪法如何通过明确ADR衔接，不静默改宪法。

## 操作纪律 / Execution discipline

- 所有源码探索先Graphify；对子agent原样传递：MANDATORY: graphify-out/graph.json exists. You MUST run graphify before reading source files. Use: `graphify query "<question>"` (scoped subgraph), `graphify explain "<concept>"`, or `graphify path "<A>" "<B>"`. Only read raw files after graphify has oriented you, or to modify/debug specific lines. This rule applies to subagents too — include it in every subagent prompt involving code exploration.
- 串行独占checkout/source writer/Cargo；每次Cargo前即时运行 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`，只接受exit0。显式独立CARGO_TARGET_DIR，offline/locked/jobs1，bounded owned PID/PG和终态收据。不要轮询/重启长任务冒充完成，不安装全局工具、依赖、修改trust或降低阈值。
- 不读取/传出/暂存/改写 `.claude/settings.json`、根 `config.lock`、根 `config.toml`；不发布sandbox私有凭据。旧branches/worktrees/artifacts/targets保留。
- `/verify` branded工具缺失可沿用已批准的repo-local替代方法；不得装工具或改checker/阈值来制造PASS。代码有变化时按实际影响重跑，不因会话重启重复昂贵且已验证的原树测试。
- 真实UI证据用loopback Gateway、真实Panel/PTY、身份和controlled deterministic provider；不以Simulated、付费外部LLM、mock DOM或代理自产gzip/br替代。
- 自动推进已授权核查与文档工作。只在**具体可审查的新设计/公开API/依赖或不可逆动作**需要且尚无授权时询问，说明批准对象和规则来源；不反复问“继续吗”。上一会话授权的是本次Phase7提交并合入本地main，不是未来任意push/release。

先报告main/HEAD/工作树与文档读取结果，完成新计划Task1–4，交付Phase8与NSH分轨的具体设计/计划和独立审查；Task5通过明确设计批准后才进行相应生产实施。结尾列出未做、未测、P2和各轨道真实状态。
