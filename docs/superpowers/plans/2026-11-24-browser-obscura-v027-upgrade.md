# Browser obscura v0.2.7 升级 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 默认浏览器内核 obscura 从 v0.2.2 升到 v0.2.7，能力表每一行按 v0.2.7 真机实测重定，补 Aleph 侧三个缺口，清理过期断言，文档同步。

**Architecture:** 不改双引擎架构。改动集中在台账 tag（`src/runtimes/specs.rs`）、能力表（`src/browser/engine/capability.rs`）、效果探针（`src/browser/cdp_backend/actions.rs`）、obscura 盒模型读取（`src/browser/page_state/fetch_obscura.rs`）、截图工具（`src/builtin_tools/browser_tools/screenshot.rs`，视实测而定）与 QA 夹具（`qa/browser_dual/`）。**先测后改**：Task 1–2 拿到 v0.2.7 的实测结论，Task 3–6 的改动与 Task 7 的翻行都以它为依据。

**Tech Stack:** Rust（tokio、serde_json、uuid v4）、Python 3 QA（websockets、zlib 解 PNG，无 PIL）、Node（既有 `.mjs` 探针）。

**Spec:** `docs/superpowers/specs/2026-11-24-browser-obscura-v027-upgrade-design.md`

## Global Constraints

- tag 唯一作者：`src/runtimes/specs.rs` 的 `obscura_tag!()`；其他地方读它，不再手写（QA 脚本里的 `"v0.2.2"` 改为从该文件派生）。
- 能力行只在 `qa/browser_dual` 或既有探针的**效果断言**通过时翻成 `Supported`；每行注释写：实测日期、`v0.2.7`、`aarch64-macos`、探针位置、什么会让它翻回。
- 永不传 `--allow-file-access`；不加自动切换引擎；不删 `fetch_obscura.rs`；不改 `github_release.rs` 的 `"v0.2.2"` 夹具。
- 不新增模块、类型或依赖（uuid 已在 `Cargo.toml:69`）。
- 上游退化（Aleph 适配无法解释的回归）→ 停下回报用户，不硬推。
- 平台只测 arm64 macOS；结论里写明平台。Chrome 对照用本机 `/Applications/Google Chrome.app`。
- 代码注释英文；回执与文档中英按项目惯例。

## Review Focus

1. **点击导致跳转、但跳转发生在回读之后**：旧文档上仍读到 nonce，`NoHit` 会误报 `EffectNotDelivered`。期望：只在 nonce 相同且 `hit == null` 时才判 `NoHit`——与现状相同的竞态窗，Task 3 不扩大它；Chromium `click` 阶段全绿是证据。
2. **页面自己写 `window.__alephProbeDoc`**：恶意页面可伪造，与现有 `__alephProbeHit` 伪造同一边界；Task 3 在同一段 ⚠️ 注释里写明，不修。
3. **真实元素恰好位于 (8,8) 且 100×20**：正常路径丢矩形、降级路径丢元素（设计 §4.4 已接受）；Task 4 的测试钉住 100×21 / (9,8) 不被误判。
4. **v0.2.7 在已 `Supported` 的行上退化**（`pdf` / `insert_text` / `screencast`）：Task 2 的 caps 全量 diff 会抓到；退化即降级，不留旧值。
5. **安装器 digest 校验依赖 GitHub API**：未认证 403 时 `provision` 是「无法运行」不是「通过」；Task 1 如实记录，不绕过校验。

---

### Task 1: 升 tag、QA 派生 tag、装 v0.2.7、跑回归基线

**Files:**
- Modify: `src/runtimes/specs.rs:92-100`（tag + 文档里「DOMSnapshot fix lands 时再升」的过期说法）
- Modify: `qa/browser_dual/run.sh:82`、`qa/browser_dual/caps.py:34`
- Create: `docs/superpowers/runs/2026-11-24-browser-obscura-v027-receipt.md`（本任务写回归基线段）

**Interfaces:**
- Produces: `OBSCURA_TAG == "v0.2.7"`；QA 两个脚本都从 `src/runtimes/specs.rs` 读 tag。

- [ ] **Step 1:** `obscura_tag!()` → `"v0.2.7"`；`OBSCURA_TAG` 文档改为「bump when a re-measured release is adopted; capability rows must be re-measured with it」。核对 `specs.rs` 中描述 release 资产的测试注释与 v0.2.7 资产清单一致（`gh api repos/h4ckf0r0day/obscura/releases/tags/v0.2.7`）。
- [ ] **Step 2:** `run.sh`：`OBSCURA_TAG="$(sed -n 's/^ *"\(v[0-9.]*\)"$/\1/p' "$REPO/src/runtimes/specs.rs" | head -1)"`，空值即 `exit 69`；`caps.py`：`MEASURED_ON` 用同一正则从 `specs.rs` 读出。（`REPO` 在 `run.sh` 中已定义于 `:60` 左右，派生行放其后。）
- [ ] **Step 3:** `cargo test -p aleph --lib runtimes::specs browser::engine::capability` → PASS（`the_obscura_tag_has_one_author`、`the_obscura_row_is_stamped_with_the_pinned_tag` 仍绿）。
- [ ] **Step 4:** `./qa/browser_dual/run.sh provision` → 台账装出 `~/.aleph/runtimes/obscura/v0.2.7/obscura`，sha256 与 release digest 一致（arm64：`fc7e73f2…0e5c`）。403 → 记为「无法运行」，改用 `gh release download` 手工取包、`shasum -a 256` 核对后以 `ALEPH_QA_OBSCURA` 继续，回执写明。
- [ ] **Step 5:** 依次跑 `open snapshot click stall reap switch escape caps`。期望：除 `caps` 的能力 diff 外全绿。任一回归：先判 Aleph 适配 vs 上游退化；后者停下回报。
- [ ] **Step 6:** 回执写「回归基线」：二进制 sha256、`obscura --version`、每阶段结果与 `caps` 的原始 diff。
- [ ] **Step 7:** Commit `chore(browser): pin obscura v0.2.7; derive QA tag from the ledger`

### Task 2: caps 探针改为测量真实 verb 的效果（drag / js_dialogs），重测六行

**Files:**
- Modify: `qa/browser_dual/caps.py:118-128`（js_dialogs、drag 探针）、`qa/browser_dual/pages/caps.html`（拖拽目标、对话框结果字段）
- Run（不改）：`docs/superpowers/specs/2026-09-24-browser-network-mock-design/probes/fetch-probe.mjs`、`docs/superpowers/specs/2026-09-30-browser-qa-design/probes/qa-events-probe.mjs`

**Interfaces:**
- Consumes: Task 1 的 v0.2.7 二进制。
- Produces: 回执「六行实测」段，每行：探针、原始输出、结论（supported / unsupported + 机制）。Task 7 只依据它翻行。

- [ ] **Step 1: drag 探针** — 换成 `cdp_backend::actions::drag` 实际发送的序列：`mouseMoved`(起点) → `mousePressed` → 三个插值 `mouseMoved` → `mouseReleased`(终点)，`button: "left"`。`caps.html` 在源元素上记 `mousedown`、在目标上记 `mouseup`（或 HTML5 `drop`），写入 `window.__dragged = "dropped:mouseup" | "dropped:html5" | "dropped:no"`。`found["drag"] = "supported"` 当且仅当 `__dragged != "dropped:no"`。删除对 `Input.dispatchDragEvent` 的探测（没有 verb 发它）。
- [ ] **Step 2: js_dialogs 探针** — 对 `alert` / `confirm` / `prompt` 各一次：页面 `setTimeout` 打开对话框并把返回值写入 `window.__dlg_<kind>`；读 2 s 内是否收到 `Page.javascriptDialogOpening`；收到则 `Page.handleJavaScriptDialog{accept:true, promptText:"caps"}` 并读回 `__dlg_confirm === true`、`__dlg_prompt === "caps"`。三者都收到事件且读回正确才 `supported`。保留「alert 不阻塞渲染」检查但改为记录（v0.2.7 行为可能变），不作断言。
- [ ] **Step 3:** `python3 -m py_compile qa/browser_dual/caps.py` → OK。
- [ ] **Step 4:** `./qa/browser_dual/run.sh caps`，记录六行。`node fetch-probe.mjs` 与 `node qa-events-probe.mjs` 用 v0.2.7（按各自文件头的启动方式设二进制路径）各跑 3 次，记录五环/事件是否到达。
- [ ] **Step 5:** 回执写「六行实测」；`effect_probe` 一行暂记前提 1、2 的结果，前提 3 等 Task 3。
- [ ] **Step 6:** Commit `test(qa): measure drag and dialogs by the verbs Aleph sends`

### Task 3: 效果探针回读识别「文档已换」（缺口 A）

**Files:**
- Modify: `src/browser/cdp_backend/actions.rs:955-1135`（安装/回读 JS、`arm_effect_probe`、`read_effect_probe`、`conclude_effect_probe`）及调用点 `:1161`、`:1372`、`:1523`
- Modify: `qa/browser_dual/caps.py`（effect_probe 前提 3）
- Test: `actions.rs` 内既有 `mod tests`（`:3588` 附近）

**Interfaces:**
- Produces:
  - `pub(super) fn effect_probe_install_js(ref_id: &str, nonce: &str) -> String` — 额外写 `window.__alephProbeDoc = <nonce JSON>`。
  - `pub(super) fn effect_probe_readback_js(nonce: &str) -> String`（替代 `EFFECT_PROBE_READBACK_JS`）— 若 `window.__alephProbeDoc !== <nonce>` 返回 `{ otherDocument: true }`，否则返回现有 `null | eventType` 并清理（同时清 `__alephProbeDoc`）。
  - `async fn arm_effect_probe(...) -> Option<String>`（返回 nonce；`None` = 未武装）；`conclude_effect_probe(..., armed: Option<&str>)`；`read_effect_probe(handle, session, nonce: &str)`。
  - 解析不变：`Null → NoHit`、`String → Hit`、其他（含 `{otherDocument}`）→ `Unknown`；给 `_` 臂加注释说明「另一个文档」走这里。nonce = `uuid::Uuid::new_v4().simple().to_string()`。

- [ ] **Step 1: 失败测试** — `effect_probe_scripts_carry_the_same_nonce`：`install_js("e7","n1")` 含 `__alephProbeDoc = "n1"`；`readback_js("n1")` 含 `"n1"` 与 `otherDocument`；`readback_js("a\"b")` 的 nonce 经 JSON 转义。改造 `:3588` 既有测试的调用签名。
- [ ] **Step 2:** `cargo test -p aleph --lib cdp_backend::actions` → 编译失败/断言失败。
- [ ] **Step 3:** 按 Interfaces 实现；`:1777`、`:3836`、`:3886` 的 mock 仍按表达式含 `__alephProbeHit` 应答（回读脚本里保留该名字）。⚠️ 伪造注释补一句：页面也能写 `__alephProbeDoc`。
- [ ] **Step 4:** `cargo test -p aleph --lib cdp_backend::actions` → PASS。
- [ ] **Step 5: 前提 3 重测** — `caps.py` 在自导航点击后读 `typeof window.__alephProbeDoc`，期望新文档上为 `"undefined"`（即回读会答 `{otherDocument}` → `Unknown`）；同时在不导航的点击上读回 nonce 仍在。记录到回执。
- [ ] **Step 6:** Commit `fix(browser): effect probe tells a replaced document from a missed event`

### Task 4: 识别 obscura 的编造盒子（缺口 B）

**Files:**
- Modify: `src/browser/page_state/fetch_obscura.rs:417-453`（`rect_from` 及其文档）、模块文档（`:40-63` 附近加一段）
- Test: 同文件 `mod tests`（`:2847` 附近）
- Modify: `qa/browser_dual/caps.py`（统计 caps 页与 `index.html` 上该 quad 出现的节点）

**Interfaces:**
- Produces: `const OBSCURA_FALLBACK_BORDER: [f64; 8] = [8.0, 8.0, 108.0, 8.0, 108.0, 28.0, 8.0, 28.0];` — `rect_from` 在 `border == OBSCURA_FALLBACK_BORDER && width == 100 && height == 20` 时返回 `None`。

- [ ] **Step 1: 失败测试** — `the_fabricated_fallback_box_is_no_box`（该 quad + 100×20 → `None`）；`a_real_box_near_the_fallback_is_kept`（border 同 quad 但 `height 21` → `Some`；quad 平移到 x=9 → `Some`）；`a_fabricated_box_reply_leaves_the_node_with_a_ref_and_no_rect`：`fetch_with_every_box_answered(<该 BoxModel>)` 后可见、有文本的元素 `rect == None` 且仍出现在 build 后的 refs 中。
- [ ] **Step 2:** `cargo test -p aleph --lib page_state::fetch_obscura` → FAIL。
- [ ] **Step 3:** 实现；`rect_from` 文档写来源（obscura `domains/dom.rs` 的 getBoxModel fallback，v0.2.2 与 v0.2.7 相同）、两条路径的代价（设计 §4.4 第 3、5 点）。
- [ ] **Step 4:** 同命令 → PASS；`cargo test -p aleph --lib page_state` → PASS。
- [ ] **Step 5:** caps 记录：在 v0.2.7 上对页面所有元素调 `DOM.getBoxModel`，统计等于该 quad 的节点数与标签；写入回执。
- [ ] **Step 6:** Commit `fix(browser): treat obscura's fabricated fallback box as no box`

### Task 5: obscura 截图坐标可信度（缺口 C）

**Files:**
- Modify: `qa/browser_dual/pages/caps.html`（一个 `position:absolute; left:137px; top:211px; width:53px; height:29px; background:#ff00ff` 的 `#geo`）、`qa/browser_dual/caps.py`（几何测量）
- 条件 Modify: `src/builtin_tools/browser_tools/screenshot.rs`

**Interfaces:**
- Produces（仅当测量不一致）：`screenshot.rs` 中一个 `const OBSCURA_SCREENSHOT_NOTE: &str`，在 `engine == Engine::Obscura` 时附在结果文本里。

- [ ] **Step 1:** caps.py 几何测量：同一 `#geo` 的 `DOM.getBoxModel`（经 `rect_from` 同规则取整）、`getBoundingClientRect`、`Page.captureScreenshot{format:"png"}` 中 `#ff00ff` 像素的包围盒（纯 Python：解析 IHDR/IDAT，`zlib.decompress`，按 PNG 五种行过滤还原，RGBA/RGB 两种色型），按 `Page.getLayoutMetrics` 的 `cssVisualViewport` 与截图宽度求 DPR 后换算到 CSS px。容差 ≤ 1 CSS px。在 Chromium 上同样跑一次作对照（验证解码器本身）。
- [ ] **Step 2:** 跑 caps，记录三组矩形到回执。
- [ ] **Step 3（条件）:** 一致 → 不改代码，回执写结论。不一致 → `screenshot.rs` 加 `OBSCURA_SCREENSHOT_NOTE`（英文，意为：截图仅用于看内容，像素位置不作为点击坐标，点击请用 ref），先写测试 `obscura_screenshots_carry_the_geometry_note` / `chromium_screenshots_do_not`，再实现，`cargo test -p aleph --lib browser_tools::screenshot` → PASS。
- [ ] **Step 4:** Commit `test(qa): measure obscura screenshot geometry`（若有代码改动另写 `feat(browser): …`）

### Task 6: loaderId（⑯b）复测

**Files:**
- 条件 Modify: `src/browser/cdp_backend/navigate.rs:114-130` 注释；`qa/browser_dual/caps.py`

- [ ] **Step 1:** caps 记录：点击链接导航后 `Page.frameNavigated.frame.loaderId` 与 `Page.getFrameTree` 的 loaderId 是否变化。
- [ ] **Step 2:** `result.loader_id.or(outcome.loader_id)` 的顺序无论结论都是正确的（`Page.navigate` 应答是权威），**不删**；只把注释改成 v0.2.7 的实测结论（仍旧 → 写 v0.2.7 仍复现；已修 → 写已修，并说明顺序仍保留的理由）。
- [ ] **Step 3:** Commit（与 Task 7 合并亦可）。

### Task 7: 按实测重写能力表

**Files:**
- Modify: `src/browser/engine/capability.rs:173-310`（OBSCURA 行与注释）、`:526-584`（`NOT_PROBED`）、`:836-905`（`every_matrix_exempt_disagreement_cites_its_reading` 的分歧名单与引用断言）、`describe_for_tool` 相关断言

**Interfaces:**
- Consumes: 回执「六行实测」与 Task 3/4 的结论。

- [ ] **Step 1:** 逐行：测到 `supported` 才改 `Cap::Supported`；仍 `unsupported` 的注释改为 v0.2.7 实测与新机制；`effect_probe` 在 Task 3 后翻 `Supported` 的前提是 Step 3 的 `click` 阶段在 obscura 上全绿。`drag` 一旦有效果读数，从 `NOT_PROBED` 的理由改为引用 `qa/browser_dual` 的 drag 探针。
- [ ] **Step 2:** 同步 `NOT_PROBED` 文案、分歧名单 `vec![...]` 与引用断言（引用串必须仍在 production 前缀里出现）。
- [ ] **Step 3:** `cargo test -p aleph --lib browser::engine` → PASS；`./qa/browser_dual/run.sh caps` → 能力 diff 为空；任一翻成 `Supported` 的行，跑对应阶段（`effect_probe` → `click`；其他行跑 caps）在两个引擎上全绿。
- [ ] **Step 4:** Commit `feat(browser): capability table re-measured on obscura v0.2.7`

### Task 8: 熵减 + 全量验证 + 回执

**Files:**
- Modify: 本轮重测过的事实在 `src/browser/**`、`crates/aleph-cdp/src/methods/{dom,target}.rs`、`src/builtin_tools/browser_tools/session.rs:789` 里的 「v0.2.2」断言
- Modify: 回执

- [ ] **Step 1:** 对 `grep -rn "v0\.2\.2" src/browser crates/aleph-cdp src/builtin_tools/browser_tools qa/browser_dual` 的每一处分类：本轮重测过 → 改写为 v0.2.7 结论；未重测 → 保留并确认写明了版本；dated 历史不动。
- [ ] **Step 2:** `cargo check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -p aleph --lib browser runtimes builtin_tools::browser_tools` → 全绿。
- [ ] **Step 3:** `qa/browser_dual/run.sh` 全部阶段最终一遍 → 全绿；回执补齐（每阶段结果、能力表最终值、刻意不做）。
- [ ] **Step 4:** Commit `chore(browser): retire v0.2.2-era claims re-measured on v0.2.7`

### Task 9: 文档

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` §3.12（新增一轮）；`docs/reference/{TOOL_SYSTEM,PROCESS_MANAGEMENT,BROWSER_LIVE_VIEWER}.md` 中 obscura 相关描述

- [ ] **Step 1:** §3.12 新增「obscura v0.2.7 升级」一轮：升级、六行结论（带回执链接）、三个缺口的处理、刻意不做。
- [ ] **Step 2:** 三份 reference 文档中与本轮代码不一致的 obscura 描述逐条改正。
- [ ] **Step 3:** 若存在 `doc_wiring_audit`，跑一次 → 0 failures。
- [ ] **Step 4:** Commit `docs(browser): obscura v0.2.7 round`；随后整分支交一次独立评审（不同于实现者的模型）。
