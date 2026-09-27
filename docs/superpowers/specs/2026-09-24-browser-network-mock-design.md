# C1 网络拦截（mock 优先）设计 spec — browser-native Round 2

- **日期**：2026-09-24
- **状态**：已批准（四节设计逐节确认）
- **前序**：Round 1（`docs/superpowers/specs/2026-09-24-browser-native-tools-r1-design.md`，已全量交付合入 main）
- **侦察依据**：C1 只读侦察报告（aleph-cdp Fetch 域零覆盖；SSRF 咽喉 `src/browser/network_policy.rs:193-216/273-278`；pi-agent-browser-native 两份副本零实现——**无抄本，自创**；obscura `server.rs:1482-1540` 有真 `Fetch.continue/fulfill/fail` 实现但 `Fetch.enable` 启用路径未确认）

## 1. 意图与成功判据

**意图**：让驱动浏览器的 AI 模型能在自动化会话中做请求级管控——**mock 为主**（确定性页面状态、API 替身、注入测试数据）、**阻断为 mock 的特例**（`abort`）、**HAR 裁出本轮**（需 Network 观测面：obscura 上是 ack-noop，且存储/PII/生命周期需独立 spec）。

**成功判据**：模型能用工具动词完成「这个接口返回我给的 JSON」「挡住这个域」，并能用 `mock_list` 自查「我注册的规则真的命中了吗」（命中计数）。obscura 上要么实测可用、要么诚实的 `UnsupportedByEngine` + recovery 拖车点名 `switch_engine`。

**硬约束**（宪法/判据）：

- 唯一 CDP 真源 `crates/aleph-cdp`，手写包装（crate 明示拒绝 codegen），禁第二份 CDP 客户端
- **SSRF 否决优先于一切 mock/放行决策**（顺序红线，不可调换）
- 按引擎能力矩阵 fail-closed 降级；R39 砍掉的 `network_interception` 行恢复并点名新动词（`capability.rs:523-545` 与 `:1065-1085` 两处防护测试同步更新）
- R3 零新重型依赖；R7 nextActions 是数据；R9 字节纪律（catalog 描述 ≤80 字符，语义走 JsonSchema doc comment）；R8 匹配用 substring 不用 regex
- 判据 §2（每条守卫先证伪）/§5（能力行点名动词）/§6（写者普查）/§8（不把未知当许可花掉）/§9（API 无客户端不算交付）/§18（验证谓词+数字）

## 2. 架构与组件（第 1 节已批）

```
browser_network 工具 ──→ BrowserBackend::route_* 默认 no-op（老后端 UnsupportedByDriver）
                              │
                    cdp_backend 覆盖 ──→ RouteTable（tab 级 + profile 级规则，内存态）
                              │
                    per-tab InterceptLoop（tokio task，随 tab session 生死）
                              │
                    aleph-cdp::methods::fetch（手写包装：enable/disable/
                                              continueRequest/fulfillRequest/failRequest
                                              + requestPaused 事件类型）
```

| 组件 | 职责 | 依赖 |
|---|---|---|
| `crates/aleph-cdp/src/methods/fetch.rs` | Fetch 域 5 方法 + `requestPaused` 事件类型手写包装；FakeCdpServer 用例 | 无（沿用 crate 既有手写模式） |
| `src/browser/cdp_backend/routes.rs`（新建） | `RouteTable`（规则 CRUD、LIFO 匹配、命中计数）+ `InterceptLoop`（per-tab consumer task：订阅 requestPaused → 决策 → 应答） | aleph-cdp Fetch 包装、`network_policy::check_url` |
| `src/browser/backend.rs` | trait 默认方法 ×4（`route_add/route_list/route_remove/route_clear`；工具 action `mock_*` 一一映射到这组 trait 方法，`mock` 是模型面词汇、`route` 是后端面词汇） | 老后端零改动（`snapshot_presented` 先例） |
| `src/builtin_tools/browser_tools/network.rs`（新建） | `browser_network` 工具薄 I/O 壳 | backend trait |

**生命周期**：InterceptLoop **惰性启动**（tab/profile 存在规则才起 task、才下发 `Fetch.enable`；零规则零 task 零开销）；规则清空即 `Fetch.disable`；tab 关闭 task 随 session 取消；profile 级规则在 `ensure_tab`/attach 时重放进新 tab。

**形状决定**：匹配与决策全在 Aleph 进程内。引擎只按粗 patterns（`*`）把事件送到 requestPaused；obscura 的 patterns 语义差异（侦察未确认项）因此不进正确性面——粗一点只是多收事件，决策不变。

## 3. 决策管线与安全（第 2 节已批）

每个 `requestPaused{requestId, url, method, headers, resourceType}` 严格有序：

1. **SSRF 复检**：`network_policy::check_url(url)`（含 `block_secrets_in_url` 半）——最高优先无条件先跑；拒绝 → `failRequest(BlockedByClient)` + 审计日志。**mock 规则无权豁免 SSRF**；redirect 链每一跳重新过此步（Fetch 逐跳拦截，无遗漏面）
2. **规则匹配**：RouteTable LIFO（tab 规则先于 profile 规则）；`url_contains` substring + 可选 `method`；未命中 → `continueRequest` 原样放行
3. **命中**：`Mock{status,headers,body}` → `fulfillRequest`；`Abort` → `failRequest(Failed)`；命中计数 +1

**故障兜底（fail-open 到放行，页面永不因 Aleph 内部故障挂死）**：

- 决策+应答包在既有 `call_with_timeout` 每命令超时内；超时/异常 → 自动 `continueRequest`
- InterceptLoop 崩溃 → 该 tab `Fetch.disable` + 规则保留标 `inactive`（mock_list 可见）+ 下次 `mock_add` 复活
- `EventStream.lagged > 0` → 审计 + 对积压 requestId 批量 continue 排干；lagged 持续 → 审计警告（broadcast 容量 64 对高频页面偏小为侦察已知项，spec 阈值：连续 3 次 lagged 即警告）

**两层咽喉不共享状态**：导航级 `check_navigation` 不动；请求级为第二道；redirect 落地后 `post_nav` 审计照旧。

## 4. 工具面与 wire 契约（第 3 节已批）

**`browser_network`（既有工具的扩展，不是新工具）**：该工具今天已存在（`browser_tools/network.rs`，只读网络日志查看器，`backend.network_log`），C1 给它加 `action` 字段（默认 `log`＝现状行为，向后兼容）而非造第 27 个工具（计划阶段发现，对第 3 节「新工具」表述的修订——更少面、零新增 catalog 条目字节压力，DESCRIPTION 改动走既有字节守卫）：

| Action | 参数 | 输出 |
|---|---|---|
| `log`（默认，现状） | — | 网络日志文本（现状不变） |
| `mock_add` | `url_contains`（必填）、`method?`、`kind: mock{status,headers,body} \| abort`、`scope: tab\|profile`（默认 tab）、`note?` | `{rule_id, scope}` |
| `mock_list` | `scope?` | 规则表 `{rule_id, url_contains, method, kind, hits, active}` |
| `mock_remove` | `rule_id` | 确认 + 最终命中数 |
| `mock_clear` | `scope`（必填，防误清） | 清除计数 |

- **wire**：`BrowserNetworkOutput{success, message, ...}` 同家族；错误走 `backend_error_text` 咽喉 → recovery 拖车自动继承（T5 chokepoint）；`classify` 穷尽 match 由编译器守卫
- **规则 id**：`r1, r2…` profile 内单调计数器，**不复用**（模型引述不串号）
- **匹配语义**：substring（与 exec DSL `ExecCondition.url_contains` 用词一致）+ 可选 method；冲突 LIFO（后到优先）
- **mock body**：string 或 JSON，上限 **256KB**（注册时输入侧拒绝，不进拦截环）；**无二进制 base64**
- **作用域**：默认 tab 级（tab 死规则死、navigate 保留），可选 `scope=profile` 升级（profile 下所有 tab 含后开的）
- **能力台账**：`network_interception` 行恢复；chromium=Supported；**obscura 行值以真机探针实测为准**（实施第一步对已装 v0.2.2 跑探针，注释写测量日期+方法）；doc comment 点名四个 action
- chrome_mcp/playwright_cli：trait 默认 no-op → `UnsupportedByDriver` + recovery 拖车（诚实不对称）

## 5. 错误处理与测试（第 4 节已批）

| 故障 | 行为 |
|---|---|
| 引擎无 Fetch 能力 | 调用即拒 `UnsupportedByEngine` + recovery 点名 `switch_engine` |
| InterceptLoop 崩溃/lagged | §3 兜底：fail-open + inactive + 审计 |
| mock body >256KB | 注册时拒绝 |
| 规则指向的 tab 已死 | `TabGone`（T1 契约复用） |

**测试（TDD + 逐条证伪）**：

1. aleph-cdp 层：FakeCdpServer 钉 Fetch 五方法形状 + requestPaused 事件解码
2. RouteTable 纯函数半：LIFO、tab 先于 profile、substring+method、命中计数
3. InterceptLoop 集成半（FakeCdpServer 驱动真 task）：**SSRF 优先序对抗用例**（mock 命中内网 URL 必须被①否决）、超时自动 continue、崩溃→inactive→复活
4. census 守卫：R39 防护测试更新 + 工具名 census 沿用 BUILTIN_TOOL_DEFINITIONS 派生模式
5. 真机 QA：`browser_dual caps` 加 `network_interception` 行探测（含 obscura `Fetch.enable` 路径确认）；`browser_mock` 独立场景缓做，记登记缺口

## 6. 范围边界（刻意不做）

- **HAR**：需 Network 观测面（obscura noop）+ 存储/PII/保留期独立 spec
- 二进制 mock body（base64）；请求改写（header override/passthrough——obscura #365 有先例，不暴露）
- exec DSL network 步骤（exec 前用 browser_network 注册即可）
- 规则持久化（内存态，绑 tab/session 生命周期）
- 工具面参照 pi-agent-browser-native（其无此功能；本工具面自创）

## 7. 执行形状

单线单分支（新文件为主，无 Round 1 双线争文件问题）。任务切分：T1=aleph-cdp Fetch 域包装；T2=RouteTable+InterceptLoop（含 SSRF 管线与兜底）；T3=browser_network 工具面+能力台账恢复+recovery 接线；T4=obscura 真机探针+caps 探测+FL/qa README 文档收尾+全量验证。

**遗留提醒**：T2 的 consumer task 是浏览器子系统第一个常驻 per-tab task——崩溃/取消/lagged 三条路径各有守卫测试；与 `tab_registry` 的 tab 关闭事件对齐（task 随 tab 死）。
