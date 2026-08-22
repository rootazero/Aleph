# Tauri WebView 资源控制 — macOS 平台验证

**日期**: 2026-08-22（报告文件名沿用任务给定的 2026-08-21）
**性质**: 只读诊断。未修改任何仓库代码，未 commit，未 push。
**机器**: Mac Mini M4 (arm64)

## 总览

| 目标 | 结论 |
|---|---|
| **G1** WebView 下限声明与强制 | **通过（含失败路径证伪）** — 四条能力探针在真 WKWebView 上全为真；调色板未塌陷；**并且我人为打断 `CSS.supports` 后，回退页确实接管**（§3.2）。**产物侧已实测**：`.dmg`/`.app` 已产出，`Aleph.app/Contents/Info.plist` 的 `LSMinimumSystemVersion == 13.3`（§11.2）。**安装门本身仍未实测**（无 <13.3 机器）|
| **G2** 构建期 brotli 预压缩 | **通过，另发现一处断线** — 四个产物 `.br` 全部 round-trip 正确；协商双向正确；`Vary`/ETag/304 全对。**但裸 `/` 永远不发 `index.html.br`**（F1）|
| **G3** 两条字节路由 Range/206 | **通过** — HTTP 上 206/416/Content-Range 全绿；两条路由的单测在 macOS 上全绿（55 条）|
| **G4** Linux 解码器诊断在 macOS 静默 | **通过（可执行验证）** — `the_check_is_inert_off_linux` 在 macOS 上首次运行并通过 |
| **G5** 毛玻璃降级不在 macOS 触发 | **页面层通过** — 未进 flat 模式、`.glass` 保持 `blur(20px) saturate(1.6)`；并顺带双向证明了 flat 降级 CSS（Linux 依赖的那半）。**窗口级 vibrancy 未实测**（无 app）|

**新发现**：F1（`/` 不发 br，真实断线，已在**产物**上复现）· F2（两个未 await 的 future，**来自 origin/main 非本次改动**）· F3 / F4（QA 脚本两处会给出假绿的断言）· **F6（`minimumSystemVersion: "13.3"` 让 macOS 26/27 上的打包必然失败 —— 机制已单变量定位，且**已按用户指示修复并端到端验证**）· F7（WASM 特性栅栏要求的 Binaryen 比任何地方声明的都新）· F8（`tailwind.css` 是唯一没有任何东西把它拴回输入的已跟踪产物，且编译器未锁版本）

**最值得看的三节**：§3.2（G1 失败路径的证伪）· **§10 F6**（本轮最大的发现 + 修法）· §12（实际应用的改动）。


---

## 0. 先说三件会影响你怎么读这份报告的事

### 0.1 HEAD 不是期望值，但代码在

| | 值 |
|---|---|
| 期望 HEAD | `791e2b9534e6d8df40011c00a98b6c125b8eda4c` |
| 实际 HEAD | `064d036fcbbca9e29ab31e598852a3cb2fd9f31f` |
| 实际 HEAD 是什么 | `Merge remote-tracking branch 'origin/main'`，**两个 parent 分别是 `791e2b953`（tauri 分支）与 `03b04f9c7`（origin/main）** |
| `main` vs `origin/main` | **相同** — 已推送 |

任务里"停下来"的**理由**是"这些 commit 未推送，可能需要先同步"。那个理由已经不成立：`791e2b953` 是 HEAD 的祖先，被测代码完整在场，而且已经推送并与 origin/main 合并。**所以我继续跑了**，但下面这条随之而来的影响必须先讲。

### 0.2 合并把 `dist/` 重建了 — G2 的字节数不再是 spec 里那两个数

合并进来的那支（memory panel 一轮）重新构建了 `interfaces/webchat/dist/`：

| 产物 | `791e2b953` | HEAD（被测） | spec 声称 |
|---|---|---|---|
| `aleph_panel_bg.wasm` | 21,914,484 | **22,193,722** | 22,177,008 |
| `aleph_panel_bg.wasm.br` | 3,360,760 | **3,400,989** | 3,396,020 |

三组数字**互不相同**。压缩比一致（15.32% vs spec 的 15.31%），所以 brotli 本身没问题，变的是输入。**任何按字面字节数写的断言在这台机器上都不该被采信**——需要断言的是比例或 round-trip，不是常数。

### 0.3 只有 Windows 侧守卫做过变异证伪 — 我据此处理了一次红

QA 脚本头部的警告应验了一次：`min-system-version` 首跑 FAIL，**红的是断言不是代码**（见 §4）。

---

## 1. 环境事实

```
ProductName:     macOS
ProductVersion:  27.0
BuildVersion:    26A5416b
Safari:          27.0
arch:            arm64
Darwin Kernel:   27.0.0 (xnu-13432.1.9~3) arm64
Xcode:           /Applications/Xcode.app  (SDK 27.0, Swift 6.4)
```

**这台机器远在 13.3 下限之上**，所以它**不能**验证 <13.3 的安装拒绝（见 §9）。

---

## 2. QA 脚本输出

### 运行 A — `ALEPH_APP` 指向不存在路径（诚实 SKIP，不读陈旧 app）+ `ARTIFACT_URL` 已设

```
== webview_compat (macos) against http://127.0.0.1:18790 ==
  PASS  br-negotiation: content-encoding
  PASS  br-negotiation: body under 4 MiB
  SKIP  br-negotiation: sha comparison
        reason: python3 with the 'brotli' module not available
  PASS  br-negotiation: identity is honoured
  PASS  br-negotiation: an explicit br;q=0 refusal is honoured
  PASS  range-206: status
  PASS  range-206: exactly 100 bytes
  PASS  range-206: content-range
  PASS  range-416: status
  PASS  range-416: content-range
  SKIP  min-system-version
        reason: no app bundle at /nonexistent/Aleph.app — set ALEPH_APP
  SKIP  install-refusal below 13.3
        reason: requires a machine running macOS < 13.3; NOT VERIFIED anywhere

== 9 passed, 0 failed, 3 skipped ==
EXIT=0
```

### 运行 B — 脚本默认值（`APP=/Applications/Aleph.app`）

```
  FAIL  min-system-version
        observed: LSMinimumSystemVersion='10.13' (expected '13.3')
== 4 passed, 1 failed, 3 skipped ==
```

这一条见 §4——**是断言读错了对象**。

### 关于那条 SKIP

`python3 -c "import brotli"` 在这台机器上失败。原因是**网络**不是打包：cp314 的 wheel 存在，`pip` 对 PyPI 报 `SSLError(SSLEOFError(8, 'EOF occurred in violation of protocol'))` 重试五次后放弃（Python 3.14.2）。

我用 node 的 `zlib.brotliDecompressSync` 做了**比脚本更强**的等价验证——脚本只查 wasm 一个，我查了全部四个：

```
aleph_panel_bg.wasm      MATCH  (22,193,722 -> 3,400,989)
aleph_panel.js           MATCH  (   110,252 ->    13,377)
tailwind.css             MATCH  (   145,380 ->    19,477)
index.html               MATCH  (     6,925 ->     2,411)
```

仓库自带的两条守卫也跑了，都绿：
- `node scripts/check_webview_baseline.mjs` → `✓ webview baseline consistent`
- `node scripts/check_panel_dist.mjs` → `✓ panel dist OK: all 42 wasm references resolve against 43 exports`

### 2.1 我额外在真实 HTTP 上验了 G2 的缓存正确性（QA 脚本不覆盖）

预压缩最容易出的错不是"压没压"，而是"共享缓存把 br 字节发给了不接受 br 的客户端"。实测：

```
Accept-Encoding: br        → content-encoding=br   vary=accept-encoding  etag=W/"6079753907dd…97ed"
Accept-Encoding: identity  → （无 encoding）        vary=accept-encoding  etag=W/"6079753907dd…97ed"
```

- `Vary: accept-encoding` **两条臂都在** ✓（缺了它，共享缓存就会跨编码串味）
- **ETag 跨编码逐字相同** ✓ —— 与 `server.rs` 注释的契约一致："Content-hash ETag over the IDENTITY representation — never over whichever encoding we happen to serve"
- `If-None-Match` 命中 → **HTTP 304，下载 0 字节** ✓


---

## 3. WKWebView 能力基线（G1 的核心）

**我没有用 Safari。** 我用 Swift 直接实例化 `WKWebView` 跑探针——那是 Tauri 在 macOS 上实际使用的引擎，比 Safari 更贴近被测对象，而且可自动化、可重复。探针源码在 `/tmp/wkprobe.swift`（未进仓库）。

### 3.1 四条能力探针 — 对着真实 Panel 页面

```
webkit-version             Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15
supports oklch             1        ← CSS.supports('color','oklch(0 0 0)')
supports color-mix oklab   1        ← CSS.supports('color','color-mix(in oklab, red, red)')
CSS.registerProperty       1        ← typeof CSS.registerProperty === 'function'
typeof WebAssembly         object
supports color-mix lab     1
supports backdrop-filter   1
data-platform              macos
data-shell                 null
data-flat                  null
data-webview-unsupported   null
.glass backdropFilter      blur(20px) saturate(1.6)
computed --color-surface   oklch(96% .005 220)
palette collapsed?         resolved: oklch(96% .005 220)
prefers-reduced-transp     0
title                      Aleph Panel
body text head             ℵ Aleph 👥 团队群聊 📁 项目管理 即将推出 🧩 Aleph Hub Main Agent 新对话 0 msgs - 08-22 ⋯ 正常 0 活
```

**四条全部为真。** 调色板 `--color-surface` 解析为 `oklch(96% .005 220)` —— **没有塌陷**。WASM 起来了，页面渲染出真实内容。

关于"页面看起来正常"：任务说"只能靠眼睛"。我用了一个不依赖眼睛的判据并给了它一个**对照**——同一探针在 `about:blank` 上（没有样式表）对同一个属性读到 `COLLAPSED (empty)`。所以这个检测器**能**区分两种世界，它在真实页面上说的"未塌陷"是有意义的。

### 3.2 我顺手证伪了 G1 的**失败**路径 —— 这是本次最有价值的一条

<13.3 的机器我没有，但"引擎太老"这件事的**页面内**后果可以在真 WKWebView 上制造：用 `WKUserScript(.atDocumentStart)` 在页面自己的内联探针**之前**把 `CSS.supports` 对 `oklch|color-mix` 的回答改成 `false`（即假装 Safari 16.3），然后看页面做了什么。

```
CSS.supports oklch (已被打断)     0
data-webview-unsupported          1
data-platform (仍应被写)          macos
回退页标题                        This system's WebView is too old for the Aleph Panel
列出的缺失能力                    color: oklch(0 0 0) | color: color-mix(in oklab, red, red)
body 背景(回退页应为白)           rgb(255, 255, 255)
提到最低版本了吗                  YES: Minimum: macOS 13.3+ · WebKitGTK 2.42+ · any evergreen Chromium or Edge WebView2.
提到 CLI/TUI 出路了吗             YES
```

结论：**G1 的运行时半边在真 WKWebView 上确实工作**——回退页替换了正文、逐条点名了缺失能力、用自己的十六进制颜色渲染（不依赖那张已经塌掉的 tailwind.css）、给出了最低版本与 CLI/TUI 出路。而且 `data-platform` 在探针失败的情况下**仍然被写入**，与 index.html 里"步骤 1、2 无条件先于探针裁决"的注释一致。

### 3.3 oklch 计数：注释说 ~328，实测 378

| 量 | 数 |
|---|---|
| `--*: oklch(...)` 自定义属性定义 | **378** |
| 其中 `--color-*` | 198 |
| 在 `@supports` 内（有保护） | **0** |
| **无保护** | **378** |
| `oklch(` 总出现 | 426 |
| `color-mix(` 总出现 | 352 |
| `@supports` 块 | 176（全是 Tailwind 自己的 `color-mix(in lab,...)` 门，**没有一个**保护调色板定义）|

`791e2b953` 与 HEAD 两份 tailwind.css 在这些量上**完全一致**。index.html 的注释写的是"~328"（带波浪号，近似），实测 378。**实质结论完全成立且更强：零保护。** 数字是小漂移，不构成缺陷。

---

## 4. `LSMinimumSystemVersion`

> ⚠️ **本节记录的是本轮开始时（修复前）的状态。** 声明位置此后被移动了——见 **§12**：`tauri.conf.json` 的 `minimumSystemVersion` 现为 `null`，下限改由 `desktop/shell/Info.plist` 的 `LSMinimumSystemVersion` 承载，`check_webview_baseline.mjs` 的 edge A 也随之改写。原因是那个配置键会同时导出 `MACOSX_DEPLOYMENT_TARGET`，进而在 macOS 26/27 上让打包必然失败（**F6**）。

| 来源（修复前） | 值 |
|---|---|
| `desktop/shell/tauri.conf.json` → `bundle.macOS.minimumSystemVersion` | **`13.3`** ✓ ← **现已改为 `null`，见 §12** |
| `interfaces/webchat/webview-baseline.json` → `macos_min` | **`13.3`** ✓（`check_webview_baseline.mjs` 守着这条边，绿；**这条声明未变**）|
| `/Applications/Aleph.app` 的 Info.plist（脚本默认读的那个） | **`10.13`** ← FAIL 的来源 |
| *（修复后新增）* 本轮打出的 `Aleph.app/Contents/Info.plist` | **`13.3`** ✓ 实测，见 §11.2 |

`/Applications/Aleph.app` 是 **8 月 6 日**的构建（可执行文件 mtime `Aug 6 21:25`），即本次改动**之前**的产物。**这条 FAIL 是断言读了一个陈旧的已安装 app，不是代码缺陷。**

**→ 见 §8 F3：这里有个脚本本身的缺口，而且它的危险方向是"假 PASS"而不是"假 FAIL"。**

---

## 5. 壳里的 `data-platform`

**先讲一个方法论问题，它比读数本身重要。**

任务要求确认壳里 `data-platform === "macos"`「**由 SHELL_MARKER_JS 注入，不是 UA 回退**」。**读 `data-platform` 这个动作本身回答不了那个括号。** 原因在 `interfaces/webchat/dist/index.html` 的内联探针：

```js
function resolvePlatform() {
  var declared = el.getAttribute('data-platform');
  if (declared === 'macos' || ...) return declared;      // 注入路径
  var ua = (navigator.userAgent||'') + ' ' + (navigator.platform||'');
  if (/Mac|iPhone|iPad|iPod/i.test(ua)) return 'macos';  // UA 回退 —— 在 macOS 上给出同一个值
  ...
}
el.setAttribute('data-platform', platform);              // 两条路都写同一个属性
```

在 macOS 上**两条路的输出逐字节相同**，所以 `data-platform === "macos"` 对"注入生效了吗"是**恒真**的——它对两种世界给同一个答案。

**能分辨的是 `data-shell`。** `SHELL_MARKER_JS` 设**两个**属性：

```rust
#[cfg(target_os = "macos")]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','macos');";
```

而内联探针只写 `data-platform`，**从不写 `data-shell`**。所以：

| | `data-platform` | `data-shell` |
|---|---|---|
| 壳内（注入生效） | `macos` | `aleph-tauri` |
| 浏览器 / 注入失效 | `macos` | `null` |

**实测（裸 WKWebView，无壳）**：`data-platform = macos`、`data-shell = null` —— 即 UA 回退路径**确实**工作，且 `data-shell` **确实**是那个判别器。

**建议**：把 §5 的验证步骤和 `qa/webview_compat/run.sh` 里的 `wkwebview-baseline` 手工清单改成断言 `data-shell === 'aleph-tauri'`；只断言 `data-platform` 的话，macOS 上那条断言恒真，**即使 macOS cfg 臂完全没生效也会绿**。

**壳内实测状态**：见 §9 —— `cargo check -p aleph-desktop-shell` 的结果附在 §10。

---

## 6. TTS 与音频源

**未验证（缺凭据）。** 按任务指定使用全新 `ALEPH_HOME=/tmp/aleph-verify`，其生成的 `config.toml` 中 `[voice.local]` 的 `tts_model = ""`、`tts_voice = ""` 均为空 —— **没有配置任何 TTS**，也没有 LLM provider，因此无法触发一次语音回复。我没有动用户真实的 `~/.aleph`（会污染其真实会话/产物，且与任务的"全新 HOME"指令冲突）。

**能静态确认的部分**（这不是播放测试，但它是关于那条具体断言的真实证据）：

`interfaces/webchat/src/platform/wide/views/voice/audio.rs:630`
```rust
/// Wrap raw audio bytes in a `blob:` object URL. WKWebView is unreliable playing
/// a large `data:` URL through `<audio>` (the no-sound bug); a blob URL loads
/// reliably. Caller revokes the URL when playback ends.
pub(crate) fn bytes_to_object_url(bytes: &[u8], mime: &str) -> Option<String> {
    ...
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}
```

整个 voice 路径里**不存在任何 `data:` 音频 URL**（`grep 'data:'` 在该目录下只命中这句注释和一个叫 `ingest_pcm(data:...)` 的参数名）。所以"blob 而非 data:"**按构造成立**——没有能产出 `data:` 音频源的代码路径。

**"不该出现的 GStreamer 警告条"**：见 §7，G4 已用可执行方式证明在 macOS 上完全静默。

---

## 7. G4 — Linux 解码器诊断在 macOS 上的静默（**已可执行验证**）

`src/diagnostics/checks/media_codecs.rs`：

```rust
/// Windows (WebView2) and macOS (WKWebView) decode media through the OS,
/// with no separate plugin set to be missing. Nothing to report.
#[cfg(not(target_os = "linux"))]
async fn run(&self, _posture: Posture) -> Vec<Finding> {
    Vec::new()
}
```

而且这个文件里有一条 **`#[cfg(not(target_os = "linux"))]` 的测试**，按 QA 脚本头部的说法它此前从未在 macOS 上跑过。**我跑了：**

```
running 5 tests
test diagnostics::checks::media_codecs::tests::every_format_lists_at_least_one_element ... ok
test diagnostics::checks::media_codecs::tests::ok_reports_a_single_info_finding ... ok
test diagnostics::checks::media_codecs::tests::unknown_is_neither_ok_nor_a_warning ... ok
test diagnostics::checks::media_codecs::tests::missing_names_the_formats_and_the_packages ... ok
test diagnostics::checks::media_codecs::tests::the_check_is_inert_off_linux ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 16937 filtered out
```

**`the_check_is_inert_off_linux` 在 macOS 上通过。G4 在 macOS 上完全静默，已证。**

顺带：`cargo test -p alephcore --lib --no-run` 在 macOS 上**编译通过**（判据清单 §10 点名的那类风险——`cargo check` 不编译 `#[cfg(test)]`——在这台机器上不成立）。全新 HOME 启动的服务端日志 205 行里 **零** warn/error。

---

## 8. Vibrancy / 毛玻璃（G5）

### 8.1 macOS 上未触发降级 —— 已证

```
data-flat                : null                       ← 未进 flat 模式 ✓
.glass backdrop-filter   : blur(20px) saturate(1.6)   ← 不是 "none" ✓
--glass-blur             : 20px
prefers-reduced-transp   : 0
```

配置侧：`tauri.conf.json` 的 `app.macOSPrivateApi = true`，`main.rs` 有 `apply_macos_vibrancy()` 与 macOS-only `.transparent(true)`。

### 8.2 我顺便把 flat 降级的 CSS 双向证了一遍（这半边 Linux 依赖它）

`data-flat` 驱动的是纯 CSS，所以 macOS 能证明 Linux 依赖的那条规则：

```
--- 默认（macOS，非 flat）---
data-flat              : null
.glass backdrop-filter : blur(20px) saturate(1.6)
--glass-blur           : 20px

--- 手工置 data-flat=1（＝Linux 降级路径 / macOS 开启「降低透明度」）---
data-flat              : 1
.glass backdrop-filter : none      ✓
--glass-blur           : 0px       ✓

--- 撤回 ---
.glass backdrop-filter : blur(20px) saturate(1.6)   ✓ 恢复
```

正/负/复原三态齐全 —— 证明是**那个属性**在驱动，不是巧合。

### 8.3 一条我怀疑过、实测后否掉的假警报

我曾怀疑 §4 手工清单里这条会误报：

```js
getComputedStyle(document.querySelector('.glass')).backdropFilter   // 期望不是 "none"
```

理由是 tailwind.css 里 `-webkit-backdrop-filter:` 出现 14 次而无前缀的只有 5 次，`.glass` 本体只设了带前缀那个 —— 若 WebKit 把两者当独立属性，这条断言会在 vibrancy 完全正常时读到 `"none"`。

**实测否掉了这个怀疑**（隔离用例，只设 `-webkit-backdrop-filter`）：

```
unprefixed .backdropFilter                  -> [blur(12px) saturate(1.4)]
prefixed .webkitBackdropFilter              -> [blur(12px) saturate(1.4)]
getPropertyValue('backdrop-filter')         -> [blur(12px) saturate(1.4)]
getPropertyValue('-webkit-backdrop-filter') -> [blur(12px) saturate(1.4)]
```

WebKit 把两者互为别名。**该断言在 macOS 上是可靠的**，不必改。

### 8.4 窗口级半透明本身 — 未验证

上面证的是**页面内** CSS 与计算值。"窗口仍然半透明、材质可见"是 `NSVisualEffectView` 层面的事，需要装好的 app 与肉眼/截图。见 §9。

---

## 9. 媒体 seek（G3）

**字节层：已证（HTTP 上真实往返）。** QA 脚本 5 条 range 断言全 PASS：

```
PASS  range-206: status              (HTTP 206)
PASS  range-206: exactly 100 bytes
PASS  range-206: content-range       (bytes 100-199/8379)
PASS  range-416: status              (HTTP 416)
PASS  range-416: content-range       (bytes */…)
```

`ARTIFACT_URL` 是我**不经 LLM**铸出来的：loopback 连接免凭据即 operator，`session.create` → `session.export_html` → 8,379 字节的 transcript artifact 及其能力 URL。这条路径值得记下来——它让 range 断言在没有任何 provider 凭据的机器上也能跑。

**第二条字节路由**：`parse_range` 有两个消费者（`artifact_route.rs` 与 `canvas_asset_route.rs`），**QA 脚本只覆盖前者**。后者的单测结果见 §10。

**浏览器里拖动进度条**：**未验证** —— 需要一个 >5 MB 的音视频 artifact，而产生它需要 LLM/上传路径与凭据。字节层前提已经成立，剩下的是播放器 UX。

---

## 10. 新发现的缺陷

### F1 — 裸 `/` 永远不发预压缩的 `index.html.br`（真实断线，影响小）

`index.html.br` 已提交（2,411 B）、被 `check_panel_dist.mjs` 双向配对守着，**但浏览器实际请求的那条路径从来读不到它**。

实测：

| 路径 | `Accept-Encoding` | 结果 |
|---|---|---|
| `/` | `br` | **无 encoding，6,925 B** |
| `/` | `br, gzip`（真实浏览器） | **gzip，3,060 B**（现场压缩）|
| `/` | `identity` | 无 encoding，6,925 B ✓ 正确 |
| `/index.html` | `br` | **br，2,411 B** ✓ |
| `/index.html` | `gzip` | gzip，3,060 B |

`Vary: accept-encoding` 在所有分支都正确设置。

**根因**（`src/gateway/control_plane/server.rs`）：

```rust
.route("/", get(serve_index))                    // ← 独立 handler
.route("/{*path}", get(serve_static_or_index))

async fn serve_index() -> Response {             // ← 无参数：拿不到 HeaderMap
    match ControlPlaneAssets::get_index_html() {
        Some(content) => ([(header::CACHE_CONTROL, "no-cache")], Html(content)).into_response(),
        ...
```

`serve_index()` **不接收任何参数**，所以它在类型上就无法协商编码。而 `serve_static_or_index` 对 `path.is_empty() || path == "/" || path.ends_with('/')` 会**早返回**到 `serve_index()`，**把已经拿到的 headers 丢掉** —— 所以 `/foo/` 这类也一样掉 br。

**为什么 16,937 条单测看不见它** —— 该模块自己的注释写着答案：

> *"Every other test here calls `serve_static_or_index` **DIRECTLY**, so the layer is never in the path"*

测试绕过了路由器，因此 `/` 这条 route 的编码行为**没有任何测试覆盖**。

**影响面**（已按加载方式分别确认）：
- 完整桌面 App：走 `tauri://localhost/index.html`（Tauri asset 协议）→ **不受影响**
- 浏览器访问 `http://host:18790/` → **受影响**
- 纯壳指向远端网关（`https://gw.example.com:8443/`）→ **受影响**

**量级**：每次冷加载多 649 字节（3,060 vs 2,411），且**每次请求都现场 gzip 一遍** —— 而 `scripts/precompress_dist.mjs` 的文件头逐字写着它存在的理由正是"the gateway currently gzips on every ETag miss… Precompressing moves that work to `just wasm` (paid once)"。绝对量很小（index.html 只有 6.9 KB，真正的大头 wasm **确实**走 br），但这是一条完整的断线：产物在、守卫在、消费路径缺一条。

### F2 — 两个未 await 的 future：`end_session` 从不执行（**与本次改动无关，来自 origin/main**）

release 构建产生的**全部** 2 条 warning：

```
warning: unused implementer of `futures_util::Future` that must be used
   --> src/gateway/handlers/group_chat.rs:261:13
    |
261 |             orch_guard.end_session(&session_id);
    = note: futures do nothing unless you `.await` or poll them

warning: unused implementer of `futures_util::Future` that must be used
   --> src/gateway/inbound_router/group_chat_handler.rs:346:17
    |
346 |                 orch_guard.end_session(session_id);
```

`src/group_chat/orchestrator.rs:184`：`pub async fn end_session(&mut self, session_id: &str) -> Option<SharedSession>`

`git log` 显示 `end_session` 是被 **`5e7dd2c98 review: migrate sync fn locks to async (Risk 4 part 5)`** 改成 `async` 的，两个调用点没有跟着加 `.await`。这**正是** CLAUDE.md 判据清单 §10 点名的那一类：

> **把一个函数改成 `async`，它的调用点会变成一个未 await 的 future——Rust 报的是 WARNING 不是 error**，于是那一步在一切照常编译的情况下静默停止执行。

**后果**：群聊会话打到 `max_rounds` 时，`session.end()` 与用户回执都照常发生，但 orchestrator **从未真的移除该会话** —— 它在 orchestrator 的表里永久泄漏。

**归属**：`git show 03b04f9c7:src/gateway/handlers/group_chat.rs` 在该行已是这段代码 —— **在 origin/main 那一侧，不是 tauri-webview 分支引入的**。本次只是恰好第一次有人读了构建输出的 warning 段。

### F3 — QA 脚本的 `min-system-version` 默认读"装着的那个 app"，没有身份护栏

```bash
APP="${ALEPH_APP:-/Applications/Aleph.app}"
```

在任何装过 Aleph 的开发机上，这条断言默认读的是**上一次安装的构建**，而不是本次树里构建出来的那个。这次它以 FAIL 的形式暴露（读到 8 月 6 日构建的 `10.13`）—— **FAIL 是安全方向**。

**危险的是反方向**：如果那个陈旧 app 恰好声明了 `13.3`，这条断言会**假 PASS**，给一个从未构建过的 bundle 发合格证。

**而版本号分辨不了它们**：陈旧 app 的 `CFBundleShortVersionString` 是 `26.7.31`，仓库 `VERSION` 文件**也是** `26.7.31` —— 两者逐字相同。

**建议**：这条断言要么只接受显式 `ALEPH_APP`（缺席即 SKIP，而不是回落到 `/Applications`），要么在读 plist 之前先断言该 bundle 的 mtime 晚于本次构建 / 其内嵌 `aleph-server` 与 `target/release/aleph-server` 同哈希。

### F4 — `data-platform` 在 macOS 上分辨不了注入与 UA 回退

详见 §5。建议判别器改用 `data-shell === 'aleph-tauri'`。这条影响的是**验证程序本身**：现有写法在 macOS 上恒真，`SHELL_MARKER_JS` 的 macOS 臂即使完全不生效也会绿。

### F5（记录，非缺陷）— `~328` 与实测 378

见 §3.3。注释带波浪号，属近似表述；实质结论（零保护）成立且更强。
### F6 — **`minimumSystemVersion: "13.3"` 让 macOS 27 上的桌面打包必然失败**（机制已定位，单变量证实，**修复已应用并端到端验证 → §12**）

> **本节被重写过两次。** 第一版把它归因为「我用 `nohup &` 让 cargo 逃出 flock 互斥」并建议在无并发环境重跑判别；第二版做了那次重跑（**零并发，复现了**），排除了并发，但把它读成「间歇性、有随机性」。**都不对。它是确定性的，而且触发它的正是这次改动本身。**

#### 因果链（每一环都实测过）

```
tauri.conf.json  bundle.macOS.minimumSystemVersion = "13.3"
        ↓  tauri-cli 据此导出环境变量（见下方它自己的 schema 原文）
MACOSX_DEPLOYMENT_TARGET=13.3
        ↓  ≥12.0 时 ld-27031 改用 chained fixups 布局 LINKEDIT
proc-macro dylib 的 LC_SYMTAB.stroff 落在 4 (mod 8)
        ↓  macOS 27 的 dyld 拒绝加载
dlopen: "mis-aligned LINKEDIT string pool"
        ↓  rustc 加载不了 proc-macro
error[E0463]: can't find crate for `serde_derive` / `tauri_macros` / …
        ↓
cargo tauri build 失败 → 打不出 .app / .dmg
```

**第一环有 tauri-cli 自己的原文作证**（`strings` 它的二进制里那份 JSON schema 描述）：

> "A version string indicating the minimum macOS X version that the bundled application supports. Defaults to `10.13`. Setting it to `null` completely removes the `LSMinimumSystemVersion` field on the bundle's `Info.plist` **and the `MACOSX_DEPLOYMENT_TARGET` environment variable**."

#### 单变量 A/B：同一个 crate，只动这一个环境变量

删掉 `libtauri_macros-*.dylib`、其余一切不变，跑四次：

| 条件 | stroff | mod 8 | size |
|---|---|---|---|
| 不设 #1 | 13,075,632 | **0** ✅ | 13,179,664 |
| `13.3` #1 | 13,071,772 | **4** ❌ | 13,175,760 |
| 不设 #2 | 13,075,632 | **0** ✅ | 13,179,664 |
| `13.3` #2 | 13,071,772 | **4** ❌ | 13,175,760 |

**两组重复逐字节相同。** 所以第二版说的「同一 metadata hash 两次编译产出不同字节 ⇒ 工具链不可复现」**是错的，本节据此更正**：编译是完全确定的，那次「不同」只是因为对照组没有设 `MACOSX_DEPLOYMENT_TARGET`——我当时没意识到那正是被测变量。

#### 取值扫描：分界在 11.5 / 12.0 之间

判据用 `dlopen` 真加载，不只看对齐：

| deployment target | mod 8 | dlopen | LINKEDIT 形态 |
|---|---|---|---|
| 10.13 | 0 | **OK** | classic `LC_DYLD_INFO` |
| 11.0 | 0 | **OK** | classic `LC_DYLD_INFO` |
| 11.5 | 0 | **OK** | classic `LC_DYLD_INFO` |
| 12.0 | 4 | **FAIL** | `LC_DYLD_CHAINED_FIXUPS` |
| 13.0 | 4 | **FAIL** | 同上 |
| **13.3** | 4 | **FAIL** | 同上 |
| 13.4 | 4 | **FAIL** | 同上 |
| 14.0 | 4 | **FAIL** | 同上 |
| 15.0 | 4 | **FAIL** | 同上 |
| 26.0 | 4 | **FAIL** | 同上 |

13.3 那份的 LINKEDIT 子段（`otool -l`）：

```
LC_DYLD_CHAINED_FIXUPS  dataoff 13008896  datasize 2216
LC_DYLD_EXPORTS_TRIE    dataoff 13011112  datasize 120
LC_FUNCTION_STARTS      dataoff 13011232  datasize 57944
LC_DATA_IN_CODE         dataoff 13069176  datasize 0
LC_SYMTAB               symoff 13069176  nsyms 109  stroff 13071772  strsize 1680
                                                    └─ 13071772 mod 8 = 4
```

**所以它不是「碰运气偶尔没对齐」，是 ≥12.0 这条链接器路径上的系统性结果**——第二版写的「1/133，说明链接器通常是对的」这个读法也要更正：那 132 个之所以对齐，是因为它们是根 workspace 的 `cargo build` 编出来的，**那条路径根本没有设 `MACOSX_DEPLOYMENT_TARGET`**，于是吃的是 rustc 对 `aarch64-apple-darwin` 的默认值 **11.0**——正好落在分界线下面。每次只坏一个，是因为每次只有一个 proc-macro 需要在 tauri 阶段新编。

#### 为什么这台机器会撞上、而 CI 不会

- 本机：macOS **27.0**（26A5416b）· Xcode **27.0** · `ld-27031` · clang 21.0.0 · rustc 1.96.0
- CI：`.github/workflows/aleph-app-release.yml` 的 macOS 矩阵是 **`macos-latest`**（GitHub 托管，目前是 macOS 14/15）

dyld 对 LINKEDIT 的这项对齐校验是新系统才有的。**所以 CI 会一直绿，而任何一台 macOS 26/27 的开发机都打不出包**——这正是「本机构建本机测试」那条铁律要抓的形状。

#### 一个让它看起来「时好时坏」的放大器

**cargo 不把 `MACOSX_DEPLOYMENT_TARGET` 计入 fingerprint。** 于是：

- 一个已经被编坏的 dylib **不会**因为环境变了而重建，它会一直坏到你手动删掉为止；
- 反过来，**如果所有 proc-macro 都已经被别的路径（普通 `cargo build`）编好且对齐，`cargo tauri build` 会直接复用它们，打包就成功了**。

所以 `just shell-build` 的成败取决于「这一次有没有 proc-macro 需要新编」——`cargo clean`、换依赖版本、换 feature 之后必然失败，热缓存上却可能一次成功。**这也是为什么它此前读起来像随机故障。**

#### 修法（下面是我最初用来隔离变量的临时绕法；**正式修复已应用到仓库，见 §12**）

把「声明给系统看的下限」和「喂给编译器的 deployment target」拆开——前者留在 plist，后者不要设：

```bash
# 1. 清掉缓存里所有未对齐的 proc-macro dylib（它们不会自己重建）
for f in target/release/deps/*.dylib; do
  s=$(otool -l "$f" | awk '/cmd LC_SYMTAB/{g=1} g&&/stroff/{print $2; exit}')
  [ -n "$s" ] && [ $((s % 8)) -ne 0 ] && rm -f "$f"
done
# 2. 用 infoPlist 写 LSMinimumSystemVersion，同时把 minimumSystemVersion 置 null
cargo tauri build --config '{"bundle":{"macOS":{"minimumSystemVersion":null,"infoPlist":"/abs/path/extra.plist"}}}'
```

`extra.plist` 只需一个键：

```xml
<key>LSMinimumSystemVersion</key><string>13.3</string>
```

⚠️ `infoPlist` 的值是**文件路径（string）**，不是内联对象——我第一次传内联 JSON 被 tauri 当场拒绝：
`error on 'bundle > macOS > infoPlist': {...} is not of types "null", "string"`。

**结果**：47 秒产出两个 bundle，且 `LSMinimumSystemVersion` 仍然是 **13.3**（见 §11.2 的逐项验证）。

**⚠️ 这个方向放弃了什么，必须由主开发机裁决**：不设 `MACOSX_DEPLOYMENT_TARGET` 之后，壳二进制是按 rustc 默认的 **11.0** 编的，于是「编译期保证壳不会调用 13.3 之后才有的 API」这条**没有了**，只剩 plist 上那道安装闸。对一个 WebView 能力下限（13.3 要的是 Safari 16.4）来说这多半无所谓——那条下限约束的是 WKWebView 的能力，不是壳自己链接的符号——但这是个取舍，不是免费的。最终落进仓库的形态与这里略有不同（**不需要 `infoPlist` 配置键**——tauri-bundler 会自动合并 `desktop/shell/Info.plist`，本轮实测确认），细节与守卫改动见 **§12**。

其它候选（我没验）：升级/降级 Xcode；`-ld_classic`（已废弃，且第一版在它上面栽过跟头）；等 Apple 或 tauri 修。

#### 上一版表格里已排除的假设仍然有效，不用重做

Cargo.lock 版本冲突 · tauri 改了 lock · 产物只是「看起来坏」· 外置卷读路径 · 并行写压力 · 磁盘写满 · 外置卷本身写坏文件（同 crate 同 flags 外置/内置各 3 次，6/6 干净）· `-ld_classic` 是稳定绕法（**已被自己证伪**）。另加本轮排除的：**我自己的 cargo 并发**（零并发下复现）· **文件被截断**（`__LINKEDIT` 的 `fileoff + filesize` 与文件大小分毫不差，且带着链接器自己的 ad-hoc 签名）。

第二版提出的「`[profile.release] strip = true` 波及 proc-macro」这个假设**不再需要**——deployment target 单独就完整解释了全部观察。我没有证明 strip 无关，只是它不再是必要条件。

#### 快速判定脚本（不需要真加载）

```bash
for f in target/release/deps/*.dylib; do
  s=$(otool -l "$f" | awk '/cmd LC_SYMTAB/{g=1} g&&/stroff/{print $2; exit}')
  [ -n "$s" ] && [ $((s % 8)) -ne 0 ] && echo "BAD $(basename $f)"
done
```

本轮两次故障，这个判据点名的文件与 `dlopen` 报错点名的 crate **每次都完全一致**（`serde_derive` / `tauri_macros`）。

**顺带**：`target/` 已占 **332 GB**（卷上余 151 GB）。与本故障无因果。

### F7 — 声明的 WASM 特性栅栏要求比任何地方声明过的都新的 Binaryen，而错误消息只会点名 flag

`interfaces/webchat/webview-baseline.json` 的 `wasm_features` 有八项，其中两项：

```
--enable-bulk-memory-opt          Binaryen 117+
--enable-call-indirect-overlong   Binaryen 更晚
```

这台机器上 `wasm-opt` 是 **116**（`~/.cargo/bin/wasm-opt`，`cargo install wasm-opt` 装的，6 月 10 日），两个都不认识：

```
$ wasm-opt --enable-bulk-memory-opt --version
Unknown option '--enable-bulk-memory-opt'
$ wasm-opt --enable-call-indirect-overlong --version
Unknown option '--enable-call-indirect-overlong'
```

`just wasm` 的缺失检查只问「有没有 wasm-opt」，安装提示给了四条命令（`cargo install wasm-opt` / `brew install binaryen` / `apt install binaryen` / `winget`）而**没有版本下限**。全仓 grep 不到任何 binaryen 版本要求。

失效形状：照着提示装了旧版的人，看到的是 `Unknown option '--enable-bulk-memory-opt'`——这句话点名了 flag，永远不点名「你的 binaryen 太旧」，而 `wasm_features` 那份文件里对这一项的注释解释的是**它为什么在名单上**（Binaryen 自己对 bulk-memory 的拆分），不是它需要哪个版本。判据是仓里已有的那条：**一个「所有 X 都必须过闸」的守卫写完之后，要问它认得几种形状**——这里闸认得「有没有」，不认得「够不够新」。

修法建议（我没有改）：`just wasm` 的那段前置检查里加一条版本断言，从 `wasm_features` **派生**——对每个声明的 flag 跑一次 `wasm-opt --help | grep -q`，缺哪个就点名哪个并说「升级 binaryen」。这样新增 flag 时下限自动跟着走，不用维护第二个数字。

**本次处理**：我装了 Homebrew 的 binaryen **132**（九个 flag 全部支持），并且**只在这次构建的 PATH 里前置** `/opt/homebrew/bin`——没有改仓库、没有改 justfile、没有卸载 116。

### F8 — `tailwind.css` 是唯一一个「被跟踪、进发布包、却没有任何东西把它拴回输入」的产物，而它的编译器没锁版本

三件事必须放在一起读才看得出来：

1. **`dist/` 是被跟踪的、承重的发布产物。** `interfaces/webchat/.gitignore` 用 16 行注释解释了为什么——`.github/workflows/aleph-app-release.yml` 逐字嵌入它，「Panel WASM dist is pre-built and committed to git — no WASM build here」，**没有任何 release job 拥有 WASM 工具链**。它还记着一次真实事故：2026-08-13 的 `033814185` 把 dist/ 改成不跟踪，发布流水线**关了两天**。

2. **CI 的守卫覆盖三条线，`tailwind.css` 不在其中。** `Scripts/check_panel_dist.mjs`（release workflow 的 `panel-dist-check` 硬闸，所有构建 `needs:` 它）验的是：`aleph_panel.js` 引用的每个 wasm 导出都存在于 `aleph_panel_bg.wasm`；以及 `.br` 兄弟的**双向**对账（每个 `.br` 解压回得去 + 每个够大的源文件都有 `.br`）。这个脚本写得很扎实——它甚至防住了「import 那个模块会不会有副作用把损坏悄悄治好」。但它从头到尾**没有一行**把 `tailwind.css` 拴回 `styles/tailwind.css` 或拴回扫描类名的 Rust 源码。

3. **它的编译器没锁。** `package.json` 只写 `"tailwindcss": "^4.2"`，而 `package-lock.json` 被 `interfaces/webchat/.gitignore` **第 2 行显式忽略**——那一行没有任何理由注释，且不是这次改动加的，来自一次纯搬迁提交 `92d604fa6 refactor: move CLI/TUI/WebChat to interfaces/`。同一个文件里，`dist/` 的那段却写了 16 行说明为什么必须跟踪。**一个被决定过，一个是继承来的。**

**这不是推演，本次构建当场演示了一遍。** 提交版 `tailwind.css` 是 **tailwindcss v4.3.1** 产出的（145,380 B）；这台机器 `npx @tailwindcss/cli` 解析到 **v4.2.2**（node_modules + 被忽略的 lockfile 都是 4.2.2），于是 `just wasm` 把它**降级**重建成 145,891 B：

```
< /*! tailwindcss v4.3.1 | MIT License | https://tailwindcss.com */     ← 提交版
> /*! tailwindcss v4.2.2 | MIT License | https://tailwindcss.com */     ← 本机重建
```

而同一次构建里，所有守卫**全绿**：

```
✓ wasm-opt applied (feature set fenced)
✓ webview baseline consistent
✓ precompressed 4 file(s), skipped 0
✓ panel dist OK: all 42 wasm references in aleph_panel.js resolve against 43 exports in aleph_panel_bg.wasm.
```

`.br` 双向对账也照样通过——因为它问的是「`tailwind.css.br` 解压回不回得到 `tailwind.css`」，而两者是**同一次**重建出来的，当然自洽。**「和自己一致」回答不了「和源码一致」。**

为什么这条对 G1 特别贵：`tailwind.css` 正是那 378 条无保护 `oklch()` token 定义的所在地，也就是本次 WebView 下限声明**要保护的那份字节**。下限声明（`webview-baseline.json`）钉住的是「WebView 要支持 oklch」，没有任何东西钉住「产出这些 oklch 的是哪个 tailwind」。tailwind v4 是扫源码类名生成 CSS 的，所以这条线上有两种独立的漂移：换版本（我刚制造的那种）和漏重建（Rust 里加了新类名但 dist 没跟着重建——`.js`/`.wasm` 有守卫接住这种事，`.css` 没有）。

**本次处理**：我在构建前对 `dist/` 做了完整备份（含逐文件 sha256），构建后已还原，工作树保持干净——**提交版的 4.3.1 CSS 一个字节都没被我换掉**。

## 11. 壳编译与桌面构建（G1 最硬的一道）

### 11.1 `cargo check -p aleph-desktop-shell` — **PASSED（史上第一次在 macOS 上编译）**

```
   Compiling aleph-desktop-shell v26.7.31 (/Volumes/TBU4/Workspace/Aleph/desktop/shell)
    Checking window-vibrancy v0.5.3
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24m 06s
[exited with code 0]
```

**零 error、零 warning。** QA 脚本头部逐字写着：

> *the `macos` arm and the `not(any(macos, windows))` (linux) arm are unverified in the strongest sense: **never built, never run, never falsified***

**"never built" 这一半现在不成立了。** 这次编译覆盖的 macOS 专属面不小 —— `desktop/shell/src/` 有 **24 处 `target_os = "macos"` cfg 点，分布在 7 个文件**，其中包括：

- `main.rs` 的 `SHELL_MARKER_JS` macOS 臂 + `apply_macos_vibrancy()`（`window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState}`）+ macOS-only `.transparent(true)`
- `cert_trust/adapter_macos.rs` —— 用 `objc2` 的 `msg_send!` / `AnyClass` / `Imp` / `Sel` 对 wry 的 `WKNavigationDelegate` 做**运行时方法替换**，是这批代码里最脆的一块
- `daemon.rs` / `perm_monitor.rs` / `external_link.rs` / `connection.rs` / `cert_trust/{mod,install}.rs`

> ⚠️ **注意范围**：编译通过只证明这些臂**类型正确**，不证明它们**运行时行为正确**。`SHELL_MARKER_JS` 的 macOS 臂现在已知能编译；它是否真的注入，要看 §12.2 的 `data-shell` 实测。

### 11.2 桌面构建 / `LSMinimumSystemVersion` / 壳内 `data-shell`

**桌面包已产出并逐项验证。** 路径是先定位 F6（`minimumSystemVersion` 污染 `MACOSX_DEPLOYMENT_TARGET`），再按用户指示把修法落进仓库（§12），然后跑完整的 `just shell-build`。

#### 预跑 `just shell-build` 的四次结果（修复前后对照）

| # | 条件 | 结果 |
|---|---|---|
| 1 | 干净跑，零并发 | **失败** — `serde_derive` 畸形（tauri 阶段新编的那个）|
| 2 | 同上 | **失败** — `tauri_macros` 畸形 |
| 3 | 删掉畸形件后重跑（仍由 tauri 阶段重编）| **失败** — `tauri_macros` 又畸形（**这是我事先写下的预测**）|
| 4 | proc-macro 缓存全部对齐（由普通 `cargo build` 编出）| **成功**，两个 bundle + 重签全部完成 |
| 5 | **修复后**，且故意删掉 `serde_derive` + `tauri_macros` 制造失败条件 | **成功** |

第 4 行是关键：它把「`just shell-build` 坏了」精确成 **「当且仅当有 proc-macro 需要在 tauri 阶段新编时才坏」**——因为 cargo 不把 `MACOSX_DEPLOYMENT_TARGET` 计入 fingerprint，热缓存会直接复用别的路径编出来的对齐产物。这正是它此前读起来像随机故障的原因。

#### 产物逐项验证（修复后，8/8）

```
== 1. LSMinimumSystemVersion ==
  PASS  app LSMinimumSystemVersion=13.3          ← §12 之前无法验证的那一项
== 2. 版本号来自 VERSION 文件 ==
  PASS  CFBundleShortVersionString=26.7.31 (== VERSION)
== 3. externalBin 真的进了包且可执行 ==
  PASS  aleph-server  (160,050,896 bytes, arm64)
  PASS  AlephBridge   (1,805,176 bytes, arm64)   ← 由 tauri.macos.conf.json 声明，平台配置自动合并
  PASS  Aleph         (13,389,616 bytes, arm64)
== 4. 签名 ==
  PASS  codesign --verify --deep --strict 通过（外层 Identifier=ai.aleph.desktop, adhoc）
  PASS  daemon Identifier=ai.aleph.server        ← justfile 那两步重签确实生效
== 5. Panel 资源嵌进 daemon ==
  PASS  aleph_panel_bg.wasm / aleph_panel.js / tailwind.css / index.html 均在二进制内
== 6. DMG ==
  PASS  Aleph_26.7.31_aarch64.dmg (75,722,381 bytes)
```

`qa/webview_compat/run.sh macos` 的 `min-system-version` 这次**对着真产物** PASS（此前只能诚实 SKIP，见 F3）。

> ⚠️ 第 5 项我第一版写成 `strings -a <binary> | grep -q`，它在这个 160 MB 二进制上**报了假红**。换成 `LC_ALL=C grep -a` 直接在字节里找，四个名字全部命中。**又一次「第一次变红时红的是断言」**。

#### 端到端：真的把打出来的 daemon 跑起来

把 `Aleph.app/Contents/MacOS/aleph-server` 用隔离的 `ALEPH_HOME` 拉起来（跑完即回收，未碰用户真实 `~/.aleph`）：

| 请求 | 结果 |
|---|---|
| `/`（`Accept-Encoding: br, gzip`）| **gzip 3060 B**、`cache-control: no-store` —— **F1 在产物上复现** |
| `/index.html` | **br 2411 B**，弱 ETag `63d7b9bf…`（＝提交版 `index.html` 的 sha256）|
| `/aleph_panel_bg.wasm`（br）| **br 3,397,068 B**，与嵌入的 dist 完全一致 |
| `Accept-Encoding: identity` | 无 `content-encoding` ✓ |

#### 仍未实测的一项

**壳内 `data-shell`**：release 构建的 Tauri 关闭了 devtools，WKWebView 没有可自动化的检查入口，我没有办法在不改代码的前提下读到壳内 DOM。`SHELL_MARKER_JS` 的 macOS 臂**能编译**（§11.1）且注入点有两处（`initialization_script` + `on_page_load` 的 `window.eval`），但**没有运行时证据**。注意即使做了，只读 `data-platform` 也不构成证据（F4）。

> **给主开发机的一条建议（与 F6 无关）**：`just shell-build` 依赖 `just wasm`，意味着**任何人做一次桌面构建都会重新生成 git 已跟踪的 `dist/`**——本轮每一次 `just shell-build` 都改动了它（我每次都从备份还原）。这也是 §0.2 那次 dist 变动最可能的来源。配合 F8（tailwind 版本未锁）后果会放大：不同机器构建会提交出不同的 CSS。

### 11.3 G3 两条字节路由的单测 —— macOS 上全绿

QA 脚本只在 HTTP 上覆盖了 `artifact` 一条路由，另一条（canvas 资产）我用单测补上了：

```
byte_range            19 passed; 0 failed
artifact_route        26 passed; 0 failed   (含 range 206/416、CSP、能力闸、限流记账)
canvas_asset_route    10 passed; 0 failed   (含 a_satisfiable_range_returns_exactly_that_slice、
                                              an_unsatisfiable_range_is_416_with_the_total、
                                              an_svg_partial_response_keeps_the_document_csp、
                                              a_range_does_not_bypass_the_capability_gate)
```

**G3 的两条路由在 macOS 上都已验证。**

---

## 12. 本轮实际应用的改动（用户明确授权后）

> 本次任务开始时是**只读诊断**。定位到 F6 之后，用户指示：**「把 minimumSystemVersion 的修法直接改进 tauri.conf.json」**。以下三处改动据此应用，`dist/` 等其余一切均已还原。

```
 desktop/shell/Info.plist           | 31 +++++++++++++
 desktop/shell/tauri.conf.json      |  2 +-
 scripts/check_webview_baseline.mjs | 90 +++++++++++++++++++++++++++++++++-----
```

**1. `desktop/shell/tauri.conf.json`** —— 一行：

```diff
     "macOS": {
-      "minimumSystemVersion": "13.3"
+      "minimumSystemVersion": null
     },
```

**2. `desktop/shell/Info.plist`** —— 下限搬到这里（tauri-bundler 会自动合并同目录的 `Info.plist`；本轮实测该文件已有的 `NSMicrophoneUsageDescription` 等三个键确实出现在产物 plist 里，所以**不需要新建文件、也不需要 `infoPlist` 配置键**）：

```xml
<key>LSMinimumSystemVersion</key>
<string>13.3</string>
```

附一段长注释，写明为什么它必须待在 plist 而不是配置里、chained-fixups 的机制、以及**这个取舍放弃了什么**——并给出「如果你是来把它改回去的」的自查命令。

**3. `scripts/check_webview_baseline.mjs`** —— 原来的 edge A 断言 `tauri.conf.json` 的 `minimumSystemVersion == macos_min`，我的改动会让它变红（**它确实红了，这是好事——单一声明的守卫在工作**）。所以 edge A 被重写成两个方向：

- **A1** `desktop/shell/Info.plist` 的 `LSMinimumSystemVersion == macos_min`（丢了就是没有下限）
- **A2** `tauri.conf.json` 的 `minimumSystemVersion` **必须是 null**（改回去就是 macOS 26/27 打不了包）

A2 是关键：它把我写在 plist 里的那段散文变成**一条会红的规则**。仓里那条判据说得很清楚——**散文守不住一条线**。失败信息直接把机制和 `error[E0463]` 的症状写进去，让下一个人不必重新查一遍。

**四条变异全部证伪过**（每条都手动破坏一次，确认变红且点名文件）：

| 变异 | 守卫反应 |
|---|---|
| plist 值 `13.3` → `12.0` | ✗ A: `Info.plist LSMinimumSystemVersion is "12.0", expected "13.3"` |
| plist 的 key 改名（等价删除）| ✗ A: `has no <key>LSMinimumSystemVersion</key> …`（并解释后果）|
| config 键"恢复"成 `"13.3"` | ✗ A: `expected null`（并给出完整机制与症状）|
| lite 覆盖层设成 `"13.3"` | ✗ A: `the overlay must omit it or set null …` |

**验证**：还原后守卫绿；`just shell-build` 在**故意删掉 `serde_derive` + `tauri_macros`** 的条件下跑通；产物 `LSMinimumSystemVersion=13.3`；`target/release/deps/` 下 **132 个 dylib 全部对齐（0 未对齐）**。

**⚠️ 这个修法放弃了什么（已核实，不是推测）**：壳二进制的 `LC_BUILD_VERSION minos` 现在是 **11.0**（rustc 默认）而不是 13.3。也就是说「壳不会调用 13.3 之后才有的 API」从**编译期强制**降级成**只有安装闸拦着**。判断依据：13.3 这条下限存在的理由是保证 WKWebView 具备 `oklch()`/`color-mix()`/`CSS.registerProperty`（Safari 16.4），那是**运行时 WebView 能力**，不是壳链接的符号——所以这个降级在本项目的语义下是安全的。**这个取舍已由用户于 2026-08-22 明确裁定接受（原话：「同意降级」）**，并已写进 `Info.plist` 的注释里 —— 记成一条**裁定**而不是一个待办，因为这一族最典型的失效方式就是「下一个真诚的修复者把它改回去」。如果将来希望两者兼得，正确方向是让 proc-macro 不继承 deployment target（cargo 目前在 stable 上没有干净的表达方式），**而不是把配置键改回去**；真要改，`check_webview_baseline.mjs` 的 edge A 会先拦住你，它的失败信息里带着自查命令。

**我没有做的**：没有提交、没有推送；没有把这条判据写进 `CLAUDE.md` 的工程判据清单（它够格，但那是主仓的编辑决定）；没有给 F7/F8 写任何修复。

---

## 13. 逐条点名：我**没有**验证的东西

| # | 项目 | 原因 |
|---|---|---|
| 1 | **macOS < 13.3 的安装拒绝** | **仍未验证。** 本机 macOS 27.0，我没有旧机器也没有旧系统虚拟机。**不做任何推测。** 现在可确认的比之前多一层：**产物** `Aleph.app/Contents/Info.plist` 里 `LSMinimumSystemVersion` 确实是 `13.3`（§11.2 实测），且 `webview-baseline.json` ↔ plist 有守卫（§12）。**但"macOS 是否真的据此拒绝安装"依然在任何地方都未验证。** |
| 2 | ~~**实际 `.dmg` 产物里的 `LSMinimumSystemVersion`**~~ | **✅ 已验证** — F6 定位并修复后，`.dmg`/`.app` 均已产出，产物 plist 的 `LSMinimumSystemVersion == 13.3`，`qa/.../run.sh macos` 对着真 app PASS（§11.2）。 |
| 3 | **壳内 `data-shell` / `data-platform` 实测** | **仍未验证。** 现在 app 有了，但 release 构建的 Tauri 关闭 devtools，WKWebView 没有可自动化的检查入口，我无法在不改代码的前提下读壳内 DOM。`SHELL_MARKER_JS` 的 macOS 臂**能编译**（§11.1），但**没有运行时证据证明它真的注入**。**注意**：即使做了，只读 `data-platform` 也不构成证据（F4）。 |
| 4 | **TTS 播放** | 全新 `ALEPH_HOME` 无 provider 与 TTS 凭据（`tts_model = ""`）。未动用户真实 `~/.aleph`。blob-vs-data 已按构造静态确认（§6）。 |
| 5 | **音视频 seek 的浏览器 UX** | 需要 >5 MB 媒体 artifact + 凭据。字节层（206/416/Content-Range）已在真 HTTP 上证明。 |
| 6 | **窗口级 vibrancy（`NSVisualEffectView`）** | 需要装好的 app + 肉眼/截图。页面内 CSS 与计算值已证（§8）。 |
| 7 | **QA 脚本的 `br sha comparison`** | 环境性 SKIP（PyPI 网络失败）。已用 node 对**全部四个**产物做了更强的等价验证。 |
| 8 | **Linux 侧全部条目** | 不是本次平台。但 flat 降级的 CSS 半边已在 macOS 上双向证明（§8.2）。 |
| 9 | **F6 修法的长期正确性** | 我验证的是「改完之后打得出包、下限仍在产物里、132 个 dylib 全对齐」。**没有**验证的是：换 Xcode / 换 rustc 之后这个绕法是否仍必要（Apple 修了链接器就该改回去，A2 那条守卫会挡着你——那是有意的，它的失败信息里写了自查命令）。 |
| 10 | **F7 / F8 的任何修复** | 只诊断、只记录，未改动。F7 我只在本次构建的 PATH 里临时前置了 brew 的 binaryen 132。 |
