# Phase 7 I3 生产集成设计 / Production Integration Design

**状态 / Status:** APPROVED — 用户已于 2026-11-07 批准整份方案及推荐选项 A（生产 Terminal 采用真实 PTY 会话操作）；尚未实现，不是 I3 关闭或原 SPEC PASS。

**日期 / Date:** 2026-11-07。

**调查基准 / Investigated HEAD:** `c89c0cc046d0db6c82a2f41f81eb94b1f68df727`，分支 `phase7-task2-special-capabilities`。

**需求 / Request:** 完成原 I3 生产集成，再重新验收；本次先交付生产集成方案。
**可实施性检查 / Builder Check:** 已完成独立文档检查，结论 `nothing important left`；随后用户明确回复“批准”，对应整份方案与 D-T 选项 A。此检查不是源码安全审查、实现验证或完整 plan-eng-review 流程。批准时设计未提交，产品代码未改。

## 1. 目标与边界 / Intent and Boundaries

补齐原计划的三个生产集成点，而不是继续增加只在测试中使用的库接口：

1. Policy 在真实访问控制和最终调用准入时执行已配置的 activation/constraints。
2. Gateway、模型工具入口与真实 Terminal/UI effect owner 连通，具有正确 schema、lease、owner 和生命周期。
3. 真实生产 activation、invocation、denial/failure 进入 Zahir 高级事件订阅，并与审计共享同一事件源。

原计划位置：`docs/superpowers/plans/2026-11-05-phase7-advanced-capabilities.md:38,79,130–136`。上一轮验收维持 CHANGES_REQUIRED；本轮是用户重新授权的 I3 续轮，不改写旧结论，也不暗中重开旧轮的第二次最终修复。

非目标：GPUI、Core 直接调用平台 API、全局键鼠监听、通用调度/原子资源预算、durable audit/replay、external-effect exactly-once、完整 ACP 多 kind 重构。不得新增第二套权限树、把审计伪装为 metadata Invalidated，或用内存 UI stub 假挂载。未授权 merge/push、配置产物清理或工具/依赖安装。

## 2. 已核实的断点 / Source-Backed Gaps

| 断点 | 当前生产事实 | 设计要求 |
| --- | --- | --- |
| Policy/context 未安装 | `gateway/execution_engine/tool_service_builder.rs:155–233` 构造真实 service，但未安装 controller/context；`capability/policy.rs:101–109` 的 PolicyEngine 按值持有独立 tree，当前无生产安装 | 改为共享现有 tree 的 Policy 准入，不再造一个树 |
| 存在替代入口 | `gateway/handlers/tools_invoke.rs:261–264` 直接调用 raw builtin registry，不经过 ScopedToolService | 新配置约束和 special adapters 在所有相关入口同闸，或明确拒绝无可信准入上下文的入口 |
| PTY 有真实消费端，但语义不同 | Panel 已调用 `pty.*`；`pty.input` 写 child stdin，`attach` 是 merged VT snapshot | 不把 PTY 改名冒充三流；见唯一用户裁定 D-T |
| UI 无真实消费端 | `capability/ui.rs` 是内存 display/input stub；现有终端 canvas 不是该 UI consumer | 新增 scoped Panel 文本/输入 surface 与真实 transport/ack |
| Special registry 未挂载 | `capability/backend.rs:70–93` 有 descriptor+handler 单 entry，但无完整租约/生产接线；ProviderRegistry 是另一条 metadata/lifecycle 来源 | 选定一个 callable record，补齐 capture/schema/source lifecycle，禁止双权威 |
| 装饰器覆盖不了生产调用 | dispatch 实际使用 concrete `ZahirFacade::resolve_with_capture`，不是只走 AuditedZahir trait 方法 | 在真实 owner/dispatch 上产生事件 |
| 事件过滤不是授权 | EventFilter 只有 type/resource/actor；OwnerRef actor 不是 authenticated principal | 从真实调用/连接 owner 捕获 audience，再走已有 live delivery gates |

全部路径均位于 `src/`，除明确的 Panel 路径。完整源码锚点、11 项 CONNECT/DECIDE census 保留在 `.superpowers/sdd/2026-11-05-phase7-advanced-capabilities/i3-production-recon.txt`。

## 3. 推荐架构 / Recommended Architecture

选择 **现有 special registry + 共享 Policy + 薄入口适配器**：

```text
Actual startup
  +-- existing canonical ToolHandlerRegistry
  +-- existing shared Arc<OwnershipTree>
  +-- special registry: descriptor + full schema + captured handler/source
  +-- shared Policy restriction boundary + source-owned facts
  +-- one bounded advanced event hub + same-source AuditCollector
  +-- mounted concrete ZahirFacade

Agent / MCP / slash / Gateway
  -> existing authentication, permission and approval gates
  -> canonical or special capture
  -> final Policy + live lease + current owner facts admission
  -> existing PTY owner OR real Panel attachment
  -> actual effect/result
  -> one owning event fire site -> Zahir advanced stream / audit / WS
```

现有 `OwnershipTree` 是 capability owner/generation 的唯一权威；PtyManager 仍是 PTY session/provider owner 的原有权威。Special registry 只拥有它的 callable record，不复制另一个 session 权限表。Gateway RPC 和模型 Tool adapter 调用同一个 crate-private domain operation，不伪造内部 RPC 请求，也不公开 ID+JSON 的无闸 dispatch。

**备选 / Alternative:** 把 callable handler/schema 并入 ProviderRegistry，且不再同时使用 special registry 存同一目标。这也可行，但扩大目前 metadata/lifecycle public registration 契约，并迁移 Task2 注册/测试，改动面更大。只注册普通 Tool 或只套 AuditedZahir 不足以关闭 I3。

## 4. Policy、配置与当前事实 / Policy, Configuration and Live Facts

### 4.1 唯一的最终限制判据 / One Final Restriction Decision

- 给 PolicyEngine 增加使用启动期同一个 `Arc<OwnershipTree>` 的构造方式，保留现有 owned constructor 的 in-tree 兼容。
- 将现有 runtime activation/constraint 判据归入共享 Policy operation；ScopedToolService、special 调用和相关 Gateway domain operations 复用它，而非复制判断逻辑。
- 原 authentication、ownership、visibility、provider generation、lease、确认与 Memory admission 保持。Policy constraints 只是附加限制，不替代这些权威。
- 在确认等待结束后、普通 handler/Memory owned task/真实 domain effect 准入之前取最新事实。不得使用 request 构造时缓存的事实。
- Controller 绑定 exact capability ID + source incarnation，不按用户传入名称临时创建。source withdrawal 先停止新准入并 revoke 对应 controller；发现 metadata 不启动 provider。
- Provider activator 有副作用时，先通过有效 scope/权限/constraints 再启动，不能把旧 activator-before-visibility 顺序扩散到新生产路径。

### 4.2 配置与事实来源 / Configuration and Fact Ownership

拟新增 typed `policies.capability_constraints`，按 fully qualified capability ID 配置时间窗口、资源阈值和已支持的 required condition。名称是拟议配置，不表示当前已实现。

- 配置由现有 configuration boundary 校验；非法 ID/shape 拒绝，不在 dispatch 重新字符串解析。
- 配置首版 **启动时应用**，调用事实实时读取。现有配置工具若修改它，必须准确报告需重启，不得声称已热生效。
- 未配置目标保持 Phase6 原行为，不获取额外事实。配置合法但 source 尚未出现时保留配置；source 真正附着前必须安装限制。source 不支持要求的事实或读失败时，受控目标 fail-closed，不能降级为无约束。
- 时间沿用现有 engine 的 UTC 小时语义；窗口边界、跨午夜规则直接复用其构造/判据并记录。时钟读取失败拒绝，不伪造 epoch 0。
- Tool 活跃调用数由该 source attachment 的真实 operation counter 提供，guard 随真实普通/Memory-owned operation 活到完成，不随已取消的 caller 提前减数。不得用不带 scope 且 caller-lifetime 的全局 diagnostic map 冒充事实。
- 保留已确定的准入前资源语义：不含当前 candidate、包含其他占用，strict-below。它不是全局原子 reservation 或并发硬预算承诺。
- PTY 使用实际 canonical session occupancy；释放资源的 close/revoke/provider-stop 不得被“资源已满”阻止。每种资源操作写明单位，不把 session 数与 invocation 数混为一谈。
- Conditions 由代码安装的 source reader 提供当前 Ready/attachment 等实际事实；配置只能引用已支持的 reader，不能自己声明 trusted=true。缺 key/reader 失败拒绝，不建立通用规则引擎。

### 4.3 所有相关入口 / All Relevant Ingresses

覆盖真实 agent run/resume、MCP face、slash service、新 special Gateway/Tool adapter、关联的 legacy `pty.*`，以及受新配置影响的 `tools.invoke`。最后者如无法安全提供原确认/可信上下文，则对该受控目标明确拒绝；不借本轮放宽既有硬门，也不重写所有 raw 工具。`ALEPH_GATEWAY_TOOLS_ALLOW` 不得绕过新 capability 限制。

## 5. Special Source、Schema 与租约 / Callable Records and Leases

- 启动期创建一份 special registry，传入实际 ProjectionHost/concrete facade；不把同一 special callable 再复制注册为独立 ProviderRegistry 权威。
- 一条 special entry 包含 descriptor、完整 schema、handler/transport reference、entry incarnation/revision；schema fingerprint 与对外完整 JSON schema 来自同一份规范化内容。
- 捕获证据绑定 exact entry incarnation、descriptor revision、OwnershipTree generation 与真实 source/provider generation。remove/replace/close 后旧 capture 拒绝；descriptor lookup 不授予调用权。
- Zahir 的 describe/lookup/resolve/validate_lease 支持真实 Terminal/Ui source。保留现有 Tool/Memory/generic compatibility；特殊来源没有强 freshness 证明时不缓存，不借 Tool revision。
- Gateway discovery 和模型薄 Tool adapters 实际拿到完整 operation schema，而不是只见 hash。Tool ingress capability 与被调用的 Terminal/Ui target 是不同入口层，effect owner 仍只有一个。
- 保持 registry dispatch crate-private，接收内部生成的 captured/admitted call；需要 Panel ack 的生产路径采用可等待、可取消的 async seam，不在异步 runtime 内用 blocking stdio 冒充 transport。
- 允许必要的 in-tree Phase7 handler async 演进；明确可能影响新 public trait 的外部实现者，不承诺未验证的生态兼容。不新增双轨泛型兼容框架。

## 6. 终端：唯一需要用户裁定的语义 / Terminal Decision D-T

原计划 `:38` 明确写了 stdin/stdout/stderr；Task2 实现 application-facing `read_stdin`、`write_stdout`、`write_stderr`。现有 PTY 却只有 master 端：写入的是 child stdin，读取进入 merged VT；无法恢复独立 stdout/stderr，也不能把 attach snapshot 称为 raw stdin。

### A. 推荐：明确采用 PTY session 作为生产 Terminal 契约

- 每个真实 PTY session incarnation 对应一个 Terminal capability。
- Production operations 为 `input / attach / resize / close`；spawn/list 由现有 config-gated manager ingress 提供。
- owner、retained exit attribution、Ready generation、screen/exit 输出、disable/reenable 全部复用现有 PtyManager/Panel 链。capability 会话标识不冒充 ACP SessionKey；只有真实已验证的 conversation binding 才带相应 scope。
- Task2 三流库接口保持原名与行为，但不把它列为已挂载的生产三流 transport。
- 这需要用户明确接受 **终端生产契约调整**。它满足真实 PTY 集成，但不能标成“原三流要求无变化且全部交付”。其他 I3 要求没有豁免。

优点：复用真实终端、权限、provider 和用户已用的 Panel，不增加第二套终端 runtime。代价：生产接口不承诺独立三流或 raw output；将来要该功能仍需另做真实 channel。

### B. 保留严格三流生产契约

新增每用户显式附着的 console/pipe transport，具有真正 recipient、独立 stdout/stderr 与可取消输入，并提供真实 UI/CLI 消费端。不能占用 daemon stdio、偷现有 PTY reader，或把两次 master write 叫 stdout/stderr。

优点：按严格解释兑现原 Task2 三流。代价：新 transport、recipient/channel、cancel/close 及消费端协议，工作量与维护面明显大于 A；仅加 PTY tee 仍解决不了 stderr 分离。

**已批准 / Approved:** D-T = A。选项 B 留作未选择的备选，不在本轮实施；terminal task 的 schema/验证按真实 PTY 契约冻结。批准不豁免其他 I3 接线、安全边界或独立验收门槛。

无论 A/B，既有 PTY tab unmount/RPC cancellation 不能关闭他人的 session；仅显式 close/provider-stop 触发原 owner 的回收。cancel-before-admission 零 effect；commit 后不承诺回滚。停止 provider 后旧 generation/capture 必须拒绝。

## 7. UI：真实 Scoped Panel Surface / Real UI Consumer

首版聚焦用户明确附着的文本/输入 surface，不是桌面全局监听。DOM/渲染/键鼠 listener 在现有 Panel/Leptos；Core 只做类型化 I/O 与 owner/lifecycle。

拟议 wire，名字可由 builder 顺应现有命名，但字段及约束需保留：

1. **attach**：选择已授权 conversation/session；服务端从真实 caller/connection/session 权威派生 owner，返回独立 `surface_id + mount_generation`。不接受 client OwnerRef/role/visibility 作为权威。
2. **display**：`surface_id, mount_generation, request_id, text` 定向到 exact attachment。Panel 把 text 渲染为 text node 后发送对应 render ack，服务端才返回 displayed=true。没有 Panel、关闭、超时或错误不是成功；publish receiver count 不是 ack。
3. **submit_input**：限定 focused surface 的 `key_down(key)` 或 `mouse_down(x,y,button)`，坐标为 surface-local CSS pixels。服务端绑定连接和 mount generation；按 source-owned 有界队列提供 input()，空队列返回 null，满队列明确报错。
4. **detach/disconnect**：撤销这一 incarnation、释放 listener/subscription、结束 pending waits 并拒绝旧 ack/input；不全局 dispose(Session)，不关闭 PTY，不把旧 surface 重定向到最新 tab。

特殊 entry 的 async adapter 引用这条真实 attachment，不再维护另一个独立有权威的内存 UiCapability。wire ingress 一次性限制文本/事件大小；内部信任 typed values，Panel 安全渲染。超时或取消不能撤回已经渲染的文本，不承诺 exactly-once。Shared protocol types 放 Core-free 的 shared/protocol，避免 WASM 引用 Core 类型。

## 8. 高级事件与审计 / Advanced Events and Audit

### 8.1 API 与单一事件源 / API and Source

- 新增独立 `Zahir::subscribe_events(trusted_scope, filter)`，旧实现默认 Unsupported；具体参数遵循现有类型边界，不暴露任意用户自报 scope。
- 旧 `subscribe(Scope, Cursor) -> CapabilityChangeStream` 和 metadata cursor 语义不变，不混用 Tool cursor 与 advanced sequence，不把 audit 伪装为 Invalidated。
- 一个 bounded hub、sequence/retention source，AuditCollector 消费同一批 committed records。真实 owner 在 commit/denial/result 处各产生一次事件，覆盖 concrete resolve_with_capture、Policy、dispatch 与特殊 effect；不由 wrapper 和 source 重复发送。
- Operational failure 仍是 ExecutionFailed，不是 PolicyViolation。emitter 失败不能改变准入/handler 结果，也不能阻止 revoke/cleanup。

### 8.2 Audience 与撤销 / Audience and Withdrawal

- 所有事件 variant 的 envelope 都有从可信 owner/caller 边界捕获的 audience/context：principal、真实 session/workspace（如存在）、source incarnation/correlation。OwnerRef 保持 actor，不冒充用户身份。
- EventFilter 只缩小选择，不是 ACL。订阅 scope 来自 authenticated request；实时投递走现有 login/role、event scope、audience、subscription selection、EventVisibility 链。
- 新 capability topic 缺必要 audience/store 时 fail-closed，不继承已有“无 SessionStore 则跳过过滤”的宽松 fallback。没有 user/session 的全局事件默认 operator-only。
- 区分 **subscriber 失权** 与 **被观察 target 已撤销**：后者仍应把最后 revoke/deny 事件交给合法的 recorded audience；不能用当前 resolve(target) 是否成功来决定历史事件可见性。旧 incarnation 的事件不交给新 owner/新 surface。
- 复用现有 identity/session/owner 权威，不新增通用权限数据库。即使两个用户同为 operator，也不因通过同一 admin topic gate 就默认共享另一创建者的 PTY 事件；保留 source-recorded owner/audience 限制。两个 tab 的功能 payload 和 ack 也不得互收。UI display 的功能 payload 与 audit 分开；audit 不记录正文、terminal bytes、按键、secrets 或任意 raw error string。

### 8.3 消费与关闭 / Delivery and Close

Gateway forwarding 先订阅再启 producer，使用可 await 的 async recv，不轮询/sleep。显式传播 lag/loss；snapshot/audit 查询使用相同 audience narrowing，丢失历史时明确 incomplete。首版 live-only，无 durable replay/cursor 恢复保证。

增加 owned close：停止新准入，提交最终 owner observations，明确停止/等待 bridge worker，关闭 subscription 并拒绝后续 emit，而非等待所有 Arc 恰巧 drop。精确 drain/close 时序、正在执行的 UI waits/PTY stop/Memory→ProjectionHost 顺序须由实现任务测试确定；不得创建无法 join 的 detached forwarding task。

## 9. 实施顺序 / Sequential Implementation

一棵树每次仅一个 implementer/reviewer。任务之间明确交回所有权；每项实现有 SPEC/QUALITY 双审，不并行编辑共享文件。

| 步骤 | 交付 | 依赖 |
| --- | --- | --- |
| T0 契约冻结 | D-T、UI surface/config 默认值、操作 schema/资源单位、保留旧验收证据 | 用户批准本方案 |
| T1 共享准入 | shared-tree Policy、真实配置/facts、post-approval guard、替代入口防绕过 | T0 |
| T2 Special source | 单 entry schema/capture/incarnation、Zahir/mount/notification、admitted async seam 与 discovery/model 接线 | T1 |
| T3 Terminal | 所选真实 transport adapter、既有 owner helper、session/provider register/withdraw/reenable | T2 + D-T |
| T4 UI | shared protocol、实际 Panel consumer、定向 display/ack/input/detach、visibility/permission registration | T2 |
| T5 事件 | 单源 scoped records、真实 fire sites、Zahir+WS+audit、lag/close/cleanup | T1–T4；types 可先定义但串行实现 |
| T6 复审与重验 | 新完整集成范围审查、生产链路效果测试、相邻回归和如实 gate 报告 | T1–T5 |

这是跨 Core、Gateway、shared protocol、Panel 的生产集成，不是一次 registry registration。主要成本在捕获租约、UI 异步 ack/关闭、事件 audience，以及所有真实入口同闸。不提供未经测量的小时/成本保证；A 比 B 少一套真实 console/channel 生命周期。

## 10. 重新验收 / Re-Acceptance

先以真正 startup composition/request builder 的测试证明接线，再用隔离的本机 disposable server、当前构建 Panel 与本机浏览器验证真实链路。仅构造 fake Tool 叫 terminal 或手动组装测试 controller，不算生产集成。

1. **Policy 与所有入口**：实际配置目标，允许请求产生一次 sentinel effect；false/missing condition、source failure、UTC 边界、usage=max-1/max、revoke/stale/stop 产生零 effect。审批期间事实改变后不能用旧事实通过。覆盖普通及 Memory-owned operation、MCP/slash、tools.invoke 或明确拒绝，env override 无法扩大权限。
2. **Terminal**：A 经真实 Gateway spawn 测试自己的 PTY、输入唯一 sentinel，观察真实 screen/attach 与外部 marker；验证 owner 隔离、resize、close/exit、capture→effect 间 provider-stop、disable/reenable 和 tab unmount 不关闭 session。B 必须改为真实三流 recipient 的精确 bytes/通道/取消证明，不能拿 screen snapshot 替代。
3. **UI**：两个 principal/session/surface 的真实 DOM；display 成功依赖正确 renderer ack，目标组件键鼠到正确 input()，陌生人不可收取；验证缺 Panel、queue-full、late/forged/wrong-generation ack、disconnect/unmount 与 listener cleanup。
4. **Events/audit**：真实 mounted Zahir API 与 WS 订阅收到 activation、effect completion、denial/failure；审计是同一个 sequence/context。验证两用户、降权、session withdrawal、最后 revoke 可见性、替换 incarnation、lag/retention/incomplete/close；不得包含 raw content。旧 metadata cursor/cache/audited-source 回归继续通过。
5. **Schema/生命周期**：Gateway/模型实际看见完整匹配 schema；替换/ABA、withdraw、provider generation 与 shutdown worker join 有效果证据，不只断言 callback 调过。
6. **断线证伪**：在单 owner、明确授权的受控变异中，断开真实 Policy fire site、Panel renderer/ack、advanced emitter 后，对应正向/负向探针必须失败；前后记录身份，恢复精确 bytes 后重新通过。变异不在本方案阶段执行。

本机装置采用独立 ALEPH_HOME/config、唯一 loopback ports、两个 disposable identities/session；用 gstack /browse 并确认浏览器本机 locality/source IP，Panel 产物包含当前压缩 assets。只清理探针创建的资源，不碰用户 session 或既有 root config.lock/config.toml。没有适合的授权装置时标 BLOCKED，不拿 host fixture 假称 live 验证。

Gate 单独报告：focused Cargo/shared/Panel tests、build/default/strict lint、适当有界全库/全 workspace、数值 coverage。上一轮 grant 失败、Memory ignore、runtime_manage 失败、全库 timeout、strict lint、未测 >=80% 不因 I3 集成完成而自动消失。没有 actual BASE run 就不把失败豁免为已证明 baseline。I3 修复通过也不自动等于 Phase7 全部绿。

## 11. 工程默认值与待批准事项 / Calls and Approval

**本方案的工程默认值（非用户既有承诺）：**
- 共享 special registry 路径 A，而非把所有 provider 改造成 callable。
- constraints 配置首版 startup-applied、facts live，不承诺 hot reload。
- UI 是 scoped、用户显式附着且 focused 的 Panel text/input surface，不做全局输入。
- advanced stream 有界内存、live-only，与审计同源，不增加 durable replay/恢复。
- 保持既定非全局原子预算语义；所有 owner facts、provider generation 和 source incarnation 不由请求自报。

**用户已批准整体方案及 D-T 的 A（生产 PTY、明确调整终端契约）。** 上述工程默认值随整份方案批准生效；B 不实施，不宣称严格三流生产交付。下一步编写精确实施计划并沿用 SDD；批准不扩展为 merge/push、清理、依赖安装或生产服务操作授权。

## 12. 未做与证据 / Not Done and Evidence

本次只完成源码调查与拟议设计，未实现或执行新测试/build/服务，未改配置/权限/阈值，未提交/合并/push/发布/清理。原 I3 仍 OPEN，原验收仍 CHANGES_REQUIRED；GPUI 延后。

调查：`.superpowers/sdd/2026-11-05-phase7-advanced-capabilities/i3-production-recon.txt`。历史验收：同目录 `phase7-final-acceptance-report.txt`、`phase7-verification-report.txt`。本设计不把任何拟议 endpoint/config/type 冒称已存在。
