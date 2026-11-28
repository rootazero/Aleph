# 语音对话重构验收回执 / Voice conversation refactor receipt

日期：2026-11-28  
分支：`voice-conversation-refactor`  
设计：`docs/superpowers/specs/2026-11-28-voice-conversation-refactor-design.md`  
计划：`docs/superpowers/plans/2026-11-28-voice-conversation-refactor.md`

## 1. 交付范围

本轮先修默认生产 Leptos Panel，覆盖浏览器与 NSH 共用的沉浸式语音页面；保留 composer 听写与 opt-in TSX Panel 的既有行为。

- `625bbe5c8`：Chat worker 保留 paired Shell provenance，原连接上的精确取消仍受原授权约束。
- `a86df6b65`：`voice.stream.audio` 的可选 `ack:true` 只报告真实 relay `try_send` admission；旧客户端、空值、未知流与无权限回复保持原形状。
- `9c6ffdbd8`：回合 owner、epoch-pinned RPC、精确回执投影、同步设备退场、流式识别与同回合 WAV fallback 接入生产 Panel。

主要行为：

1. 一句话只发送一次 Chat；保留实时字幕、逐句 TTS、barge-in 和现有 Chat dials。
2. 退出、barge-in 或连接替换先撤销本地回合所有权，再停止精确持有的麦克风 track、释放播放与订阅。
3. `chat.send` 使用 `voice_input:true`；只有收到原 epoch 的明确回执后才允许 `chat.abort { run_id, run_only:true }`，不带 `session_key`。回执丢失标为结果未知，不重发、不猜测 run，也不在替换 epoch 清理旧 run。
4. 首个回执前的无 session 事件和有歧义终态有界暂存；按明确 `run_id` 经正常 Chat projector 重放，不另造第二份业务状态。
5. 健康流式识别直接发送可用字幕；缺流、空字幕、丢帧、关闭或 admission 未确认时，用同一段保留录音补做一次批式 STT。末帧 admission 最多等待 250 ms（另含事件循环调度开销）；每个流只封存一次尾帧。
6. TSX 路径未被迁移到这套生命周期，避免同时改变两个播放与传输契约。

## 2. 自动化验证

| 验证 | 结果 |
|---|---|
| `cargo test --locked -p aleph-panel --target aarch64-apple-darwin` | **1588 passed / 0 failed** |
| `cargo test --locked -p alephcore --bin aleph-server gateway::handlers::voice` | **21 passed / 0 failed** |
| run-only abort visibility tests | **3 passed / 0 failed** |
| 真实 dispatch/worker 的 paired Shell provenance 回归 | **1 passed / 0 failed**；先 RED（worker provenance `Some(false)`），修复后 GREEN |
| `cargo check --locked -p alephcore --bin aleph-server` | 通过；3 条 warning，包含非 Linux、非 test 构建中平台限定 provenance bookkeeping 的 dead-code 提示，详见构建日志 |
| `just wasm` | 通过 WebView feature fence；39 个 JS import reference、42 个 WASM export 检查通过 |
| `just panel-tsx-web` | 通过，产出 `interfaces/panel/out-web/Aleph Panel` |
| QA Node suites | **10/10** |
| `python3 -B -m unittest discover -s qa/nsh_voice -p 'test_*.py' -v` | **5/5** |
| `git diff --check` | 通过 |

`cargo fmt --all -- --check` 会报告本分支外的既有格式差异；本轮仅格式化修改过的 Rust 文件，没有把无关重排混入提交。

## 3. 默认生产浏览器证据

最终矩阵使用当前 `interfaces/webchat/dist`、真实 feature `aleph-server`、隔离 Gateway/数据目录、受控 loopback provider，以及本机 Chrome 158。结果为 **15/15 PASS**：

| 用例 | 证明内容 |
|---|---|
| `success` short / long，各 cold+warm | 实际 STT → Chat → TTS、自然播放结束、设备 track 释放；每轮只发送一次 |
| `permission` | 权限拒绝可见，未创建 Chat/TTS 副作用 |
| `close-capture` / `cancel-stt` | 麦克风取得前与 STT 期间退出，无迟到提交 |
| `close-send` / `cancel-chat` | 明确 run 回执后只取消原 run；关闭后释放 track |
| `close-playback` / `barge-in` | 当前 source 提前停止，不发送多余 Chat abort；barge-in 后 successor 能重新采集且旧回复不重放 |
| `disconnect-send` | 原 epoch 断开后不重发、不推断 run；替换连接可建立，track 释放 |
| `batch-empty` | 空批式识别不发送 Chat |
| `stream-healthy` | 10 个已 admission 帧、0 次批式上传、2 次逐句合成；首次播放比终态早 **2005.69 ms** |
| `stream-empty` | 空流式字幕只补做 1 次 WAV，随后只发送 1 次 Chat |
| `stream-late-ack` | 800 ms 延迟 admission 下只补做 1 次 WAV；末帧请求到批式请求 **255.72 ms**，没有旧回合尾帧越过 fallback |
| `exact-queue` | configured-provider 下 queued/running 精确取消、活跃 sibling 与 successor 完成；queued run 未到达 provider，最后一次 abort 为 `{ run_matches:true, aborted:true, dropped:0 }` |

流式 QA 按首个 `stream_id` 归因帧，避免把播放结束后的下一次自动 Listening 错算成旧回合晚帧。最终 `exact-queue` 的 Chat provider attempts 为 6；允许辅助模型调用保留 sibling 文本，但四个固定场景 marker 与 queued-run 未达 provider 的断言仍分别核对。

Opt-in TSX 兼容矩阵另行 **7/7 PASS**：success、limit、permission、config-only、cancel-stt、cancel-chat、disconnect。

### 同机同夹具时延对照

下表是浏览器请求/Web Audio hook，不是声学延迟，也不是多轮统计的 p50/p95；每个格子只有一个 cold 或 warm 样本。基线和最终值均为 STT 请求到 player-start hook 的毫秒数。

| 输入 | Cold 基线 → 最终 | Warm 基线 → 最终 |
|---|---:|---:|
| short | 371.17 → 132.05 | 213.93 → 70.84 |
| long | 290.08 → 129.75 | 398.60 → 79.98 |

缓存的 Playwright Chromium 145 在图初始化前以 `NotSupportedError` 拒绝 fake-device capture；最终证据使用 `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`，没有改写声音夹具或放宽声音行为断言。

## 4. NSH 打包与设备边界

- 使用校验过的 NSH **0.6.0 (5541637)** 工具链打包 macOS app，包内含 `aleph-server` 与 `AlephBridge`；最终包路径见下方证据根目录的 `delivery-package/`，`codesign --verify --deep --strict` 通过。最终包首次本地 readiness probe 超时后 `/ready` 达到 `phase:complete`，点击“重试”加载了生产 Panel。
- 原生 QA 保留真实 `HOME=/Users/zouguojun` 以使用登录 keyring，同时隔离 `ALEPH_HOME` 和 provider。把 HOME 整体替换成临时目录会导致 `Secure credential storage could not be read`，因此那不是生产凭据回归。
- 本机 `ffmpeg -f avfoundation -list_devices true -i ''` 没有发现音频设备。生产 NSH 页面实际打开沉浸式语音后显示“未检测到麦克风设备”，原生日志为 `Audio capture was requested but no device was found amongst 0 devices`。
- 通过实际 AX 退出按钮完成关闭；原生自动化没有形成可信的 Escape 按键证据。由于无输入设备，**未声称**真实麦克风识别、原生声学播放或最终 track-stop 真机验证。
- 独立观察到此打包 debug 宿主 Cmd-Q 后 bundled Gateway 仍监听 18790，测试后已显式 SIGTERM 清理。这是单独的宿主生命周期问题，本轮没有把它混入语音提交或宣称修复。

## 5. 证据位置与未覆盖项

私有证据根目录：

`/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/aleph-voice-integration.u74GnyW7`

- 最终默认矩阵：`delivery-<case>-<clip>/receipt.json`，汇总日志 `delivery-matrix.log`。
- TSX：`final-tsx-*`；最终单元/构建：`final-validation.log`。
- 原生配置、截图与日志：`native-runtime/`；打包日志：`delivery-package.log`。
- 早期 RED 与诊断回执保留在同一目录，不能与最终 PASS 混算。

未覆盖或明确不做：

1. 当前机器无音频设备，真实麦克风与可听播放仍需有设备的 macOS 真机验收；Linux paired-Shell effect 也未在本机执行。
2. 三次最终独立审阅均未在时限内产出结论（两次 GPT 6.2 Sol、一次 Kimi 3.1 Pro），因此没有独立 whole-branch 批准；由主会话完成源码核查和上述验证，并保留此限制。
3. 不把 loopback provider、Web Audio hook 或 fake-device capture 写成真实声学/网络质量证明。
4. 未迁移 TSX 到 Classic 回合 owner，未改变 composer 听写，未处理上述独立 NSH quit-stop 问题。
