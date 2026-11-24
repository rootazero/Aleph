# Browser — obscura v0.2.2 → v0.2.7 升级、按实测重定能力表、补 Aleph 侧缺口

- **日期**：2026-11-24
- **分支 / worktree**：`browser-obscura-v027` · `.worktrees/browser-obscura-v027`（基于 main `99090adce`）
- **承接**：[`2026-09-06-browser-dual-engine-design.md`](2026-09-06-browser-dual-engine-design.md)（双引擎架构，本轮**不改架构**）；FEATURE_LOCATOR §3.12 第八轮起的能力表记录。
- **用户选择**：2026-11-24 对话中选「做 obscura 升级和补齐缺口」，并同意方案 1「按实测升级」。

## 0. 意图

Aleph 的浏览器默认跑 obscura（轻、快），Chromium 是逃生舱，两者藏在同一个 Aleph 自有 CDP 层后面。这套架构 2026-09 已落地，现在的短板不在架构，而在**默认内核停在 v0.2.2**：能力表里 obscura 有六行是 `Unsupported`，模型在这些动作上只能切到 Chromium，「默认内核」名不副实。上游已到 v0.2.7（2026-11-13，658 个提交），其中有直接对着这几行的修复。

本轮要做到：默认内核换成 v0.2.7；能力表每一行都以 **v0.2.7 真机实测** 为依据；能在 Aleph 这侧补的缺口补上；新版本让其失去意义的绕行代码删掉；文档与代码一致。

## 1. 目标 / 非目标 / 反目标

**目标**
1. 台账锁定 `OBSCURA_TAG = v0.2.7`，安装、校验、启动、孤儿回收在真机走通。
2. `qa/browser_dual` 全阶段在 v0.2.7 上重跑；能力表 obscura 六行（`js_dialogs` / `drag` / `file_upload` / `effect_probe` / `network_interception` / `error_events`）各自有一条带日期、版本、平台的实测结论。
3. Aleph 侧三个缺口（§4）修掉。
4. 熵减：被 v0.2.7 证伪的绕行与注释清理（§5）。
5. FEATURE_LOCATOR §3.12 新增一轮记录；提及 obscura 的 reference 文档校对。

**非目标**
- 自动切换引擎。仍由模型判断、`browser_session{switch_engine}` 执行（R7 / R10 第 5 不；pi-agent-browser-native 同样明文 "No automatic Chrome fallback"）。
- 给 obscura 上游提补丁（拦截暂停、对话框事件）——下一轮。
- 传 `--allow-file-access`（spec 2026-09-06 §7.2 禁止，`allow_file_access_appears_nowhere_in_the_browser_subsystem` 守着）。
- 删掉 `page_state/fetch_obscura.rs`：v0.2.7 的 `domains/domsnapshot.rs` **仍然**编造几何（`let y = (i as f64) * 18.0`，源码已核），删除条件未满足。
- Linux / Windows 真机复测（本轮只在 arm64 macOS 跑，平台写进每条结论）。

**反目标**（技术上能跑，但算失败）
- 能力行凭源码阅读或单测翻成 `Supported`。只有 `qa/browser_dual` 的**效果断言**能翻（判据 §18：静态读一个分支 ≠ 测量它前面的闸）。
- 升级后某个原来 `Supported` 的行实际退化，而表没跟着改。
- 为新版本加一层兼容分支，让 v0.2.2 也能跑（台账只有一个 tag，不做多版本）。

## 2. 约束

- 红线 R1/R3/R7/R10；判据 §8（未知不能当许可）、§12（同一事实一处派生）、§17、§18。
- 能力表是一张**可证伪的名单**：每行旁边的注释必须说明依据（实测日期、版本、平台、探针位置）。
- 台账 tag 只有一个作者：`src/runtimes/specs.rs` 的 `obscura_tag!()`；`the_obscura_tag_has_one_author` 守着。
- 资产名：v0.2.7 发布了与 v0.2.2 同名的五个默认变体（`obscura-{aarch64,x86_64}-{macos,linux}.tar.gz`、`obscura-x86_64-windows.zip`），`obscura_asset` 表不用改。
- 已知环境风险：GitHub API 未认证请求会 403（本机 2026-11-24 已遇到）；安装器的 digest 校验依赖 API。真机运行前确认可达，失败要如实记录而不是跳过校验。

## 3. 方案取舍（对话中已选 1）

| 方案 | 内容 | 取舍 |
|---|---|---|
| **1 按实测升级（选中）** | 升 tag → 真机重测六行 → 只按效果断言翻行 → 补 Aleph 侧缺口 → 熵减 → 文档 | 工作量中等；结论可信；本轮只覆盖 arm64 macOS |
| 2 只升 tag + 回归 | 跑现有测试即止 | 最便宜，但能力表不动，缺口不补 |
| 3 方案 1 + 上游补丁 | 额外给 obscura 提拦截 / 对话框修复 | 收益最大，节奏受上游发版控制 → 下一轮 |

## 4. 设计

### 4.1 升级本身

- `src/runtimes/specs.rs`：`obscura_tag!()` → `"v0.2.7"`。`specs.rs` 中说「v0.2.2 发布确实如何」的测试注释要按 v0.2.7 资产清单重核（Windows 默认 zip 在 v0.2.7 存在，`windows/aarch64` 仍无）。
- `src/runtimes/github_release.rs` 测试里的 `"v0.2.2"` 是**夹具数据**（假 release），不代表锁定版本，**不改**。
- 升级的首要风险是**回归**而非新能力：⑭ 全局命令屏障、⑮ 一引擎一连接、60 s 断头台、CDP WebSocket 的 Origin 校验（上游近期收紧了默认 origin 规则）。`open` / `stall` / `switch` / `reap` 阶段任何一项变红，先判定是 Aleph 适配问题还是上游退化；后者则**停下来回报用户**，不硬推升级。

### 4.2 六行能力的重测

全部走 `qa/browser_dual/caps.py`（已有各行的效果探针，见 `found[...]` 赋值处）与对应阶段，在 v0.2.7 arm64 macOS 上跑。源码预判仅用于排优先级：

| 行 | v0.2.2 结论 | v0.2.7 源码预判 | 本轮动作 |
|---|---|---|---|
| `network_interception` | 实测 NO：导航暂停不保持；subresource 事件 2/3 | `56b97c4` 按页面会话路由 Fetch、多个暂停/通配修复 | 重跑 fetch-probe 五个环节；**五环全过**才翻 |
| `error_events` | 实测 NO：`Runtime.exceptionThrown` 10 s 不到 | `dispatch.rs` 新增 `RuntimeEvent::Exception` → `exceptionThrown` | 重跑 caps 行（回调已证实执行的异步 throw） |
| `js_dialogs` | 未知按 NO | 只有 beforeunload 发 `javascriptDialogOpening`；`e1b6f4c` 默认自动关闭非交互确认框 | 重测 alert / confirm / prompt；预期仍 NO，行注释写明新依据 |
| `drag` | 从未在 obscura 上测（fail-closed） | 与版本无关；`actions::drag` 发六个 `Input.dispatchMouseEvent` | **首次**在 obscura 上跑 drag 效果断言 |
| `file_upload` | 实测 NO：被 `--allow-file-access` 闸住 | v0.2.7 `dom.rs:515` 仍检查 `page.context.allow_file_access` | 实测确认；预期仍 NO，注释更新为 v0.2.7 依据 |
| `effect_probe` | 实测 NO：自导航点击后回读读到新文档的 null | 卡点在 Aleph 回读方式 | 见 §4.3，改完再测 |

规则：测到 `Supported` 才翻；测到 `Unsupported` 的，注释从「v0.2.2 实测」改为「v0.2.7 实测」并写新依据；原本 `Supported` 的行（`pdf` / `insert_text` / `screencast` / `ref_precheck`）随全阶段重跑一起复核，退化就降级。`capability_table_agrees_with_the_t0_support_matrix` 等守卫测试同步更新。

### 4.3 缺口 A：效果探针在自导航点击后的回读（engine 无关修复）

现状：`src/browser/cdp_backend/actions.rs` 的探针在节点上装监听，点击后读 `window.__alephProbeHit`。点击触发跳转时，新文档上读到 `null`，与「点击没送达」无法区分，所以 obscura 行被关着。

设计：安装时在同一文档写一个本次调用独有的 nonce（`window.__alephProbeDoc = <nonce>`）；回读一次取回 `{doc, hit}`。
- `doc` 等于 nonce → 同一文档，按 `hit` 判 `Hit` / `NoHit`（现有语义）。
- `doc` 缺失或不等 → 文档已换，返回 `ProbeReadback::Unknown`（动作成立、效果不可证），与 Chromium 现有的「回读输掉竞态」同一处理。

只改探针 JS 与 `read_effect_probe` 的解析，不加新类型。改完后在两个引擎上跑 `click` 阶段（含自导航点击）；obscura 上全绿才把 `effect_probe` 翻成 `Supported`。

### 4.4 缺口 B：obscura `DOM.getBoxModel` 的编造盒子

现状：obscura（v0.2.2 与 v0.2.7 相同）在 `getBoundingClientRect` 拿不到数组时，返回固定 quad `[8,8,108,8,108,28,8,28]`、`width 100`、`height 20`，**报成功且无标记**。`page_state/fetch_obscura.rs` 不识别它，假几何会进 `PageState` 并被模型当坐标用。

设计：
1. 先测：在真页面上统计这个 quad 出现在哪些节点（预计是 `_wrap` 失败或无布局的节点）。
2. `fetch_obscura::rect_from` 把与该常量**完全相等**的应答当作「无盒」（`None`），计入头行 `no_box`。
3. 可见性分两条路径说清楚（`src/browser/page_state/build.rs::visibility_of`）：
   - **正常路径**（`computed` 交叉校验通过）：可见性只看 `computed`，丢掉假盒子只去掉 `@x,y wxh`，元素与 ref 保留。
   - **降级路径**（交叉校验连续两次失败，所有 `computed == None`）：`visibility_of` 回落到 `rect.is_some()`，带假盒子的元素会**从快照中消失、失去 ref**。本轮**接受**这个结果，不改降级路径的可见性策略：假盒子本来就不是「存在」的证据（判据 §8），降级路径今天已经让所有无盒元素和全部文本消失（模块文档称之为 textless），假盒子元素与它们同等对待。降级路径的可见性策略不在本轮范围。
4. 回归测试：正常路径上假盒子元素保留 ref、无矩形；降级路径上假盒子元素与无盒元素结果一致。
5. 代价写进模块文档：一个真实位于 `(8,8)` 且恰好 100×20 的元素，正常路径下丢掉矩形，降级路径下整个消失。

### 4.5 缺口 C：obscura 截图的坐标可信度

借 pi-agent-browser-native 的做法：不确定的几何就说未知。v0.2.7 的截图由 `obscura-render` 绘制（有真实布局），但截图像素与 CSS 坐标、与 `PageState` 矩形是否对得上**没人测过**。

设计：在 `snapshot` / `click` 阶段旁加一个测量：同一元素的 `PageState` 矩形、`getBoundingClientRect`、截图里该元素的像素位置三者比对。
- 对得上：不加任何提示，记录实测。
- 对不上：`browser_screenshot` 在 obscura 上返回一句固定说明（截图用于看内容，不能作为点击坐标依据；点击请用 ref），文案与触发条件只在一处定义。

### 4.6 loaderId 绕行（⑯b）复核

v0.2.7 有 `7341597 Preserve document identity through native navigation`。重测「点击链接导航后 `Page.frameNavigated` 的 loaderId」：若新版本已正确换 loader，复核 ⑯b 当时为绕开它写的代码是否可以简化；不成立则只更新注释里的版本依据。**不预设结果。**

## 5. 熵减

- 能力表、`fetch_obscura.rs` 模块文档、`engine/obscura.rs` 等处「verified at v0.2.2」类断言：凡本轮重测过的，改写为 v0.2.7 结论；未重测的保留原文并注明版本。dated 历史段落不改写。
- §4.6 若成立，删除对应绕行代码及其测试。
- 不新增模块、不新增类型；§4.3 / §4.4 都在现有函数内完成。

## 6. 验证

- `cargo check` / `cargo clippy` / 浏览器相关 `cargo test`（`src/browser`、`src/builtin_tools/browser_tools`、`src/runtimes`）。
- `qa/browser_dual/run.sh` 全部阶段 + `caps` 在 v0.2.7 arm64 macOS 真机跑；Chromium 侧作对照。
- 回执落 `docs/superpowers/runs/2026-11-24-browser-obscura-v027-receipt.md`：每行能力的原始探针输出、平台、二进制 sha256。

## 7. 文档

- FEATURE_LOCATOR §3.12 新增一轮：升级、六行结论、三个缺口、刻意不做。
- 校对 `docs/reference/{TOOL_SYSTEM,PROCESS_MANAGEMENT,BROWSER_LIVE_VIEWER}.md` 中 obscura 相关描述与本轮代码一致。

## 8. 留给实现者

- §4.3 nonce 的生成方式与长度（只需单次调用内唯一）。
- §4.4 的常量是否同时比对 `width/height`（建议比对，降低误判）。
- §4.5 比对的容差（建议 ≤ 1 CSS px，DPR 取 `Page.getLayoutMetrics`）。
- 重测失败时的排障顺序：先区分 Aleph 适配 / 上游退化，再决定改代码或回报。
