# `src/canvas` 断线静态审计 / Severed-wire audit

- 基线：`d577913674e3ef2f3e2f3ec75c83f75bc53704ee`。读完 `src/canvas/{mod,assets,doc_io,store,selection,validate}.rs`，包含测试；跨生产边界用 `rg` 核对，`graphify query "how does canvas store persist and load documents" --budget 600` 只用于导航。未运行 Cargo 或修改源码。
- 结果：6 项（2 项断线、4 项其他静态缺陷）；**CONNECT 3 / CUT 0 / DECIDE 3**。置信度描述静态行为是否成立，不表示已复现，也不替代产品策略决策。

## 断线 / Severed wires

### CAN-01 - 损坏文档的删除恢复分支过不了 RPC 入口（未修，C01-4）
- Form: no-consumer
- Producer: `src/canvas/store.rs:302` - `CanvasStore::delete` 的 `Internal` 恢复分支（`:306`），持锁删除无法解析、超出读取上限或无法读取的目录；再取锁入口见 `src/canvas/doc_io.rs:107`。
- Consumer: `src/gateway/handlers/canvas.rs:250` - `handle_delete` 是正常的生产调用者，但在`:256` 先 `gate_canvas`，后者在`:104` 读同一份文档，在`:118` 把读取错误转换成找不到；因此到不了`:265` 的删除调用。**对发起请求前就已损坏的文档，没有可达的生产消费者**；不是说整个 `delete` 无人调用。
- Live caller search: `rg -n --glob '*.rs' 'gate_canvas|handle_delete|\.delete\(&params.canvas_id\)|lock_for_delete' src interfaces shared desktop plugins crates tests` 找到注册 `src/bin/aleph-server/commands/start/builder/handlers/canvas.rs:51`、唯一生产 `store.delete` 调用及 handler 测试；`rg -n --glob '*.rs' 'store\.get\(&params.canvas_id\)|canvas ID does not match|doc\.id !=|doc\.id ==|selection::get|selection::set|CanvasApi::(get|delete|list)\(' src interfaces shared desktop plugins crates tests` 证实 Panel 调用在 `interfaces/webchat/src/platform/wide/views/canvas/library.rs:221`。现有删改竞态测试 `src/canvas/store.rs:1255` 不证明 RPC 可进入这个恢复分支。
- Symptom: 已知 ID 的损坏画布经 `canvas.delete` 返回找不到，目录和资产继续留在磁盘。即便不受 owner filter 限制的请求也要先过读取闸。十月报告把存储层修复记为 fixed；该修复没有接通生产入口。不建议让受限 caller 在所有权不可读时直接删除。
- Hypothesis: DECIDE - 明确谁可以恢复性删除所有权不可读的目录，再决定接到明确授权的管理入口，还是移除这个无入口的恢复承诺。
- Confidence: HIGH

### CAN-02 - 目录 ID 和文档 ID 没有共享同一道身份校验（未修，C01-6）
- Form: name-path-drift
- Producer: `src/canvas/store.rs:152` - `CanvasStore::get` 按传入的目录 ID 读文件，`:155` 原样返回文档内的 `doc.id`；`src/canvas/store.rs:204` 只在列表入口拒绝二者不等。`src/canvas/store.rs:255` 同样按目录取写锁，`:385` 却按文档 ID 发布更新。
- Consumer: `src/gateway/handlers/canvas.rs:196` - `handle_get` 接收该文档并在`:200`、`:205` 用内嵌 ID 找选区、构建资产 base；`src/gateway/server/canvas_asset_route.rs:263` 用 URL 中的 ID 找资产目录。`interfaces/webchat/src/platform/wide/views/canvas/mod.rs:173` 则用事件 ID 匹配正在打开的 ID。
- Live caller search: `rg -n --glob '*.rs' 'doc\.id !=|doc.json id does not match|self\.emit_updated|CanvasApi::get\(|canvas::TOPIC|TOPIC\.to_string' src desktop plugins crates tests interfaces shared` 只见列表比较，并找到活的 Panel `get` 调用（`interfaces/webchat/src/platform/wide/views/canvas/library.rs:142`）、发布和 topic 映射；`rg -n --glob '*.rs' 'try_exists|asset.*not found|missing_asset|nonexistent.*asset|\.read_asset\(|sweep_assets_with|fn lock\(' src/canvas src/gateway/handlers/canvas.rs src/builtin_tools/canvas.rs src/gateway/server/canvas_asset_route.rs interfaces/webchat/src/platform/wide/views/canvas` 确认三个生产资产读取入口。
- Symptom: 针对已有目录和文档 ID 不一致的磁盘数据（不是正常 `create` 生成的数据），列表不显示该画布，但按目录 ID `get` 仍成功；生成的资产地址指向另一个目录，按原 ID 打开的客户端也不消费用内嵌 ID 发布的更新。十月报告记录为延期，当前列表的局部收紧没有统一两种身份。
- Hypothesis: CONNECT - 在从磁盘加载可操作文档的边界校验身份一致性，让查询、更新和列表对坏数据得出同一结论。
- Confidence: HIGH

## 非连线类静态缺陷

### CAN-03 - 强制幂等键时，Panel 五个写调用在分发前被拒绝
- Form: other-static-defect
- Producer: `interfaces/webchat/src/api/canvas.rs:117` - `CanvasApi::create` 序列化不含 `idempotency_key` 的参数；同样的发送路径用于 `apply`（`:147`）、`delete`（`:168`）、`asset_put`（`:185`）、`selection_set`（`:231`）。`interfaces/webchat/src/context.rs:1955`、`:2325` 原样转交和发送 `params`，只另生成 JSON-RPC ID。
- Consumer: `src/gateway/server/connection/mod.rs:654` - 从 `params.idempotency_key` 取值；`:692` 选 lane，`:697` 在开启 `require_idempotency_key` 时拒绝无键的写请求。`src/gateway/lane.rs:61` 将这些方法落到 `Mutate`，`:198` 要求幂等保护。
- Live caller search: `rg -n --glob '*.rs' 'CanvasApi::(list|create|get|apply|delete|asset_put|asset_get|selection_set)\(' src desktop plugins crates tests interfaces shared` 找到五个活的写调用，例见 `interfaces/webchat/src/platform/wide/views/canvas/editor.rs:219`、`:390`、`:628` 和 `interfaces/webchat/src/platform/wide/views/canvas/library.rs:194`、`:221`。`rg -n --glob '*.{rs,js,ts}' 'idempotency_key|async fn send_rpc\(' interfaces/webchat` 只找到另一个非 Canvas 的键生成点及共用发送函数；`rg -n 'req\.params|\brpc_rx\b|\brequest\.params\b' interfaces/webchat/src/context.rs` 确认线上参数未补键。
- Symptom: `[gateway] require_idempotency_key = true` 时，Panel 无法创建、编辑、删除、上传或推送选区；服务端返回 `IDEMPOTENCY_KEY_REQUIRED`（`-32030`），选区调用端丢弃该错误。列表和读取仍可用；默认配置为 false（`src/gateway/config.rs:276`）。不是 handler 缺注册，也不是配置不生效。
- Hypothesis: CONNECT - 让 Panel 写请求发送稳定的单次逻辑操作键，并让同一操作的重试复用该键。
- Confidence: HIGH

### CAN-04 - `canvas.delete` 落入通用限流桶
- Form: other-static-defect
- Producer: `src/bin/aleph-server/commands/start/builder/handlers/canvas.rs:50` - 活的 `canvas.delete` 注册；`src/gateway/handlers/canvas.rs:265` 发起实际删除。
- Consumer: `src/gateway/rate_limiter.rs:539` - `scope_for_method` 给其他破坏性动词映射 `RpcWrite`（`:549`），但 Canvas 删除落入 `RpcDefault`（`:568`）；生产远端 WS 在 `src/gateway/server/connection/mod.rs:503` 查该分类，共享限流中间件在 `src/gateway/middleware/rate_limit.rs:126` 也读取同一分类。
- Live caller search: `rg -n --glob '*.rs' 'scope_for_method\(|"canvas\.delete"|RpcWrite' src desktop plugins crates tests interfaces shared` 找到 Panel API、生产 handler 和两个限流调用点，没有 Canvas 的严格桶分支；`rg -n --glob '*.rs' 'canvas\.delete|canvas\.apply|canvas\.asset\.put|canvas\.selection\.set' src/gateway/{lane.rs,rate_limiter.rs,method_visibility.rs,method_census.rs}` 确认 `KeyChecked` 可见性分类存在，它不是速率分类。
- Symptom: 受限流约束的远端请求按通用限额而非严格写限额计数，编译默认配置分别为 100/分钟和 20/分钟（`src/gateway/rate_limiter.rs:230`、`:235`）；未绕开 owner 检查，也没有落入 Query lane。这里仅确认不可逆删除与现有破坏性写分类不一致，没有推定高频编辑也应使用同一桶。
- Hypothesis: DECIDE - 将删除与其他破坏性写归同一桶，或明确记录画布删除为何例外并锁定该分类。
- Confidence: HIGH

### CAN-05 - 画布根目录枚举失败却返回空库或不完整列表
- Form: other-static-defect
- Producer: `src/canvas/store.rs:164` - `list_entries` 在根 `read_dir` 的非 `NotFound` 错误上 warn 后返回空向量（`:170`），迭代中途错误则返回已收集部分（`:180`）。这是整次枚举出错，不是按设计跳过一个坏文件（`:228`）。
- Consumer: `src/gateway/handlers/canvas.rs:175` - 包装成成功的 `CanvasList`（`:186`）；`src/builtin_tools/canvas.rs:819` 同样返回成功的 `count`/`canvases`（`:830`）。Panel 的活列表入口见 `interfaces/webchat/src/platform/wide/views/canvas/mod.rs:128`。
- Live caller search: `rg -n --glob '*.rs' 'CanvasApi::(get|delete|list)\(|\.list_entries\(|cannot enumerate canvas root|directory enumeration failed mid-way' src interfaces shared desktop plugins crates tests` 找到两个生产存储调用、两个活 Panel 列表调用，以及存储测试；没有可供两面消费的失败或部分完成标记。
- Symptom: 已挂载后，根目录因权限、类型错误或目录读取 I/O 故障不可枚举，Panel 和模型仍看到成功的空库；迭代中断则把部分结果当完整库。日志是唯一会报告该故障的面。不存在的根本来就是该函数允许的空库情况，不计入缺陷。
- Hypothesis: CONNECT - 将根枚举失败或不完整结果的信号传到工具和 RPC，同时保留逐条跳过损坏文档的策略。
- Confidence: HIGH

### CAN-06 - 去重素材续期失败后仍返回可用 ID
- Form: other-static-defect
- Producer: `src/canvas/assets.rs:165` - `CanvasStore::put_asset` 在去重时用 `set_modified` 重置宽限；打开、续期或阻塞任务失败只 warn（`:189`），`:208` 仍返回 `Ok(asset_id)`。同文件`:313` 的清扫按旧 mtime 判定宽限是否已过。
- Consumer: `src/gateway/handlers/canvas.rs:295` - `handle_asset_put` 将该 ID 包为成功（`:298`）；`interfaces/webchat/src/platform/wide/views/canvas/editor.rs:397` 接收后用于建图。中间任一次成功 apply 会触发清扫（`src/canvas/store.rs:293`）；后续图形 upsert 仅校验 ID 格式（`src/canvas/validate.rs:278`）。
- Live caller search: `rg -n --glob '*.rs' '\.put_asset\(|\bsweep_assets_with\b|renew asset grace|ORPHAN_GRACE|CanvasApi::asset_put\(' src desktop plugins crates tests interfaces shared` 找到 Panel 实际上传、RPC、模型工具调用及唯一生产清扫触发；`rg -n --glob '*.rs' 'asset_ids\(|ImageSource::|CanvasOp::AddShape|read_asset\(|\.apply\(' src/canvas/validate.rs src/canvas/store.rs src/gateway/handlers/canvas.rs src/builtin_tools/canvas.rs shared/protocol/src/canvas.rs interfaces/webchat/src/platform/wide/views/canvas/shape_view.rs` 核对格式校验、成功提交后清扫及实际回读路径。
- Symptom: 条件性竞态：一份已过 1h、仍未被引用的文件命中去重但无法续期；上传成功后、引用该文件的 apply 之前，另一批不引用它的 apply 可清掉它；随后引用仍可能提交，但读图时已找不到素材。此路径不删已有引用的文件。这是源码明确采用的 best-effort 降级（`:168`），不是每次上传都会发生。
- Hypothesis: DECIDE - 明确成功回执是否保证短期可引用；如保证，应让续期失败可见或让提交验证引用，不依赖日志兜底。
- Confidence: HIGH

## 已核对但未列为候选

- **导出与注册：** `src/lib.rs:63`、`src/canvas/mod.rs:14`、`:18` 的模块和重导出都有生产引用；`CanvasListing` 的类型名很少出现，但其 `owner_user_id` / `row` 在 `src/gateway/handlers/canvas.rs:175`、`:180`、`:184` 和 `src/builtin_tools/canvas.rs:819`、`:824`、`:828` 被实际使用。`CanvasError` 四个分支映射见 `src/gateway/handlers/canvas_error.rs:40`。构造与总线注入在 `src/bin/aleph-server/commands/start/mod.rs:1517`；逐个核对 `create/get/list_entries/apply/delete/put_asset/read_asset` 对应 `src/gateway/handlers/canvas.rs:148`、`:104`、`:175`、`:237`、`:265`、`:295`、`:313`。`selection::set/get` 的非测试路径见 `src/gateway/handlers/canvas.rs:342`、`:200` 和 `src/builtin_tools/canvas.rs:865`。
- **调用与路由：** 八个 Panel RPC 均有活调用和匹配的生产注册（`interfaces/webchat/src/api/canvas.rs:100` 至 `:238`；`src/bin/aleph-server/commands/start/builder/handlers/canvas.rs:24` 至 `:70`）。`src/gateway/handlers/mod.rs:926` 的占位由启动流程替换；根无法创建时保留 `SERVICE_UNAVAILABLE` 是显式降级（`src/bin/aleph-server/commands/start/mod.rs:1515`）。模型工具同名 `canvas`，构造在 `src/executor/builtin_registry/builder/constructor/mod.rs:249`，名称与 action 分发在 `src/builtin_tools/canvas.rs:793`、`:814`。正常数据下资产 URL 的 producer、路由挂载和路径消费者一致（`src/gateway/handlers/canvas.rs:206`、`src/gateway/server/mod.rs:1084`、`src/gateway/server/canvas_asset_route.rs:158`、`:263`）。
- **事件与配置：** `src/canvas/store.rs:384`、`:395` 发布，`src/gateway/events/frame.rs:825` 映射到共享 `TOPIC`，`src/gateway/event_visibility.rs:682` 分类，Panel 在 `interfaces/webchat/src/platform/wide/views/canvas/mod.rs:158`、`:167` 订阅解析。未发现已声明但无发送的 Canvas 生命周期事件；已定义的是 apply 更新事件。Slice 没有 schema 中解析后不读的画布专用配置；运行路径消费根目录、共享 store 和总线。`require_idempotency_key` 是活配置，问题在客户端（CAN-03）。
- **存根、特性与旧报告：** Slice 无未声明 feature 限制生产 impl；`src/canvas/doc_io.rs:120` 的 `slot_count` 明确只为测试。八月 `sweep_orphan_assets` 和旧 `CanvasStore::list` 已不存在，实际清扫与列表分别在 `src/canvas/store.rs:293` 和 `src/gateway/handlers/canvas.rs:175`，不重报。十月 `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md:68` 至 `:72` 的其余输入限制、去重文件类型闸仍在；`:119` 的读取字节上限、选区形状/字节上限和残留目录删除仍在。完整 op 事件是现行契约而非断线；最近一次选区推送的语义也不要求跨会话持久化。

## 未覆盖的接缝

- 静态审计没有运行 `cargo`、生产服务、浏览器/WASM、真实 RPC 请求或故障注入。未实测权限和磁盘故障、续期竞态、远端限流计数和强制幂等配置；这里只报告能从代码跟出的条件和结果。
- 未穷尽安装产物的 feature/target 组合、仓外 RPC 客户端或反向代理路径行为；调用覆盖限于当前 checkout 的 Rust 源码与必要的相邻发送链。未扩展到网关独立方法、Panel 全部交互逻辑或 Canvas 之外的同名功能。
