# 交接 Prompt：Panel 语音闭环（一核多端，端采集 → 核 STT/LLM/TTS → 端播放）

> 复制以下整段作为新 session 的首条消息。

---

## 任务目标

把 Aleph 桌面 App 的 **Panel 麦克风按钮**做成一个**完整的语音通道闭环**（Panel = 一个语音 channel，和 Telegram/Slack 同理）：

```
①端侧录音 → ②上传音频给核 → ③核STT → ④LLM回答 → ⑤核TTS → ⑥端侧播放语音
```

用户说话 → Aleph STT 转文字 → LLM 回答 → 回答文字经 TTS → Panel 自动播放语音回复。
**范围已确认：一次做完整闭环 ①→⑥（含 TTS 语音回复 + 端侧播放）。**

## 架构红线（必须遵守，用户反复强调）

**一核多端：麦克风是「端能力」按钮，和「文件上传」同类**，不是核心业务。
- **端（endpoint）**：①录音、⑥播放 —— 设备 I/O，归桌面端原生 limb。
- **核（core）**：③STT、④LLM、⑤TTS —— 只处理字节，**绝不把"采集编排"塞进 core 业务逻辑**。
- 对照：「进入项目工作」才是核心侧按钮（业务逻辑）；麦克风/上传/摄像头/截图都是端能力。
- ②上传：端把音频字节当作"语音消息/上传文件"发给核，复用核现成的 inbound 语音管线。

R1（大脑-四肢分离）、R4（Interface 纯 I/O）、R6（一核多端）、R7（LLM 主权）全程适用。

## 已诊断的根因（不要重走弯路）

WKWebView 的 `getUserMedia` 在**未签名/ad-hoc 的 macOS App 上必然失败**（红字 `Microphone permission denied`）：
- 采集发生在 WebKit 沙箱 GPU 进程，其麦克风授权与 app 级 TCC 是**两套独立闸门**；GPU 进程那道闸需要**正式 Developer ID 签名**，ad-hoc + Info.plist + `com.apple.security.device.audio-input` entitlement **都不够**（已实测仍 denied）。
- 本机 `security find-identity -p codesigning -v` = **0 个有效签名身份**，发布构建也没签名 → getUserMedia 路线在 macOS 上**绕不开**。
- **原生 AVFoundation 采集（经 Swift bridge）可用**：已由用户实测——让 agent 调 `media` 工具的 `record_audio` 时**弹出系统授权框并成功录音**。这是 macOS 的正解（也符合 R6：bridge 是端侧原生 limb）。

> 参考实现：`/Volumes/TBU4/Github/openclaw` 的 macOS app 用原生 `AVAudioEngine`/`AVCaptureDevice.requestAccess(for:.audio)`（`apps/macos/Sources/OpenClaw/VoiceWakeTester.swift`、`scripts/codesign-mac-app.sh`），不用 webview getUserMedia。

## 当前代码状态（关键！）

**已提交（commit `54887d81b`）—— getUserMedia 路线，macOS 上已弃用，仅 Win/Linux 仍有意义：**
- `desktop/shell/Info.plist`（`NSMicrophoneUsageDescription`）、`desktop/shell/Entitlements.plist`（`com.apple.security.device.audio-input`）
- `desktop/shell/src/webview_perms.rs` + `main.rs` mod 接线 + `Cargo.toml`（Linux `webkit2gtk`/Windows `webview2-com` 的 getUserMedia 授权 handler；Win/Linux webview 不签名也能用，**保留**；macOS 这条已被原生路线取代）
- `interfaces/webchat/src/views/chat/composer/voice.rs`：mic 按钮 + getUserMedia + 失败红色错误气泡

**未提交（本轮工作，层 1-4 = 端侧原生采集能力，已写好待编译/验证）：**
- `desktop/macos/bridge/Sources/AlephBridge/RPC/AudioSession.swift`：actor 内加保活的 `recordStart()`/`recordStop()`（开放式 `AVAudioRecorder.record()`，`recordStop` 先读 `currentTime` 再 `stop()` 返回 file path；`recordStart` 用 `await AVCaptureDevice.requestAccess(for:.audio)` 主动弹框）
- `desktop/macos/bridge/Sources/AlephBridge/RPC/AudioHandlers.swift`：注册 `media.audio.record_start` / `media.audio.record_stop`
- `shared/protocol/src/desktop_bridge/methods/media.rs`：`METHOD_AUDIO_RECORD_START/STOP` + `RecordStartParams`/`RecordStartResult`/`RecordStopParams`（stop 复用 `RecordAudioResult`）
- `desktop/shared/src/traits/media.rs`：`record_audio_start()` / `record_audio_stop()`（默认 `NotImplemented` → 非 macOS 自动回退）
- `desktop/macos/src/lib.rs`：两个 bridge 转发器（仿 `record_audio`，stop 用 `call_with_timeout` 15s）

**运行中的 `/Applications/Aleph.app`**：已热补丁 —— Info.plist 注入 mic 用途串 + `audio-input` entitlement + adhoc 重签（`codesign --force --deep --entitlements desktop/shell/Entitlements.plist --sign -`）+ 用户已授予原生 mic TCC（`ai.aleph.desktop`）。daemon 在跑，AlephBridge 在跑。

## 待实现（完整闭环）

**复用（已存在，先读懂再接）：**
- 核 STT：`src/gateway/voice/inbound.rs`（`transcribe_bytes`、`resolve_stt_source`）——channel 入站语音管线
- `src/gateway/inbound_router/mod.rs`（入站消息路由）、`src/gateway/voice/outbound.rs`（TTS 出站）、`src/gateway/reply_emitter/emitter/helpers.rs`
- `src/gateway/handlers/voice.rs`：现有 `voice.transcribe`（bytes→text，复用 inbound 的 STT）
- channel 既有 TTS out（memory 记：voice backend ~80% wired）
- agent 的 `media` 工具：`src/builtin_tools/media_tool.rs`（action `record_audio` 时长版；已加 start/stop trait）

**新增（端采集 → 接入 inbound 语音管线 → 端播放）：**
1. **端采集触发 + 取字节**：Panel 调端能力 RPC 触发 bridge `record_start`/`record_stop`；`record_stop` 让核读回音频文件 → 返回 base64 字节给 Panel（像文件选择器产出文件字节）。⚠️ 不要把 STT 融进 stop（采集=端，转写=核，两步分开，对齐文件上传）。
2. **上传音频走 inbound 语音管线**：Panel 把音频字节当**语音消息**发给核（不是填草稿！）→ 核 STT → 进正常 agent 循环 LLM 回答 → 复用 channel 既有 TTS。注意 Panel 作为 channel/session 的接入方式，复用 inbound_router。
3. **端侧播放**：Panel 收到 TTS 音频 → `<audio>` 自动播放。
4. **平台分支**：macOS → 原生 bridge 采集；Windows/Linux → 现有 getUserMedia（`webview_perms.rs` 已授权，webview 不签名能用）。用 `record_audio_start` 返回 `NotImplemented` 作为"回退浏览器采集"的信号。

**gateway 取 MediaCapability 的接线难点**：gateway handler 当前**拿不到** `Arc<dyn DesktopPlatform>`（`media` 工具靠 `self.platform.media()`，平台在 `src/bin/aleph-server/commands/start/mod.rs:1728/1769` 各 reporter 内各自 `Arc::new`，无共享句柄线到 gateway）。需要把 platform/MediaCapability 线进 voice 端能力 handler（`register_handler!` 在 `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`，voice.transcribe 在 ~527 行，捕获 `config`+`shared_token_mgr`）。

## 构建/部署/验证链（macOS，关键）

改动涉及 Swift bridge + Rust core + WASM panel，**必须全量刷新**才能端到端测：
1. `just swift-bridge`（重编 AlephBridge，含新 record_start/stop）
2. `just wasm`（重编 panel）
3. `cargo build --release -p alephcore --bin aleph-server`（rust_embed 把新 dist + 新协议烧进 binary）
4. 替换 `/Applications/Aleph.app/Contents/MacOS/aleph-server` + 替换 `.../AlephBridge`（staged 在 `desktop/shell/binaries/` 或 `.build`）
5. **重签**：`codesign --force --deep --timestamp=none --entitlements desktop/shell/Entitlements.plist --sign - /Applications/Aleph.app`（app 是 adhoc，换 binary 破坏签名，必须重签且带 entitlement）
6. `osascript -e 'quit app "Aleph"'` → `open -a Aleph`（supervisor 拉起新 daemon）
- 或一次性 `just shell-build`（全量 dmg，~10+ 分钟，发布形态）

**编译验证手段（本机能做的）**：`cargo check -p alephcore`、`cargo check -p aleph-desktop-shell`、`cargo check -p aleph-panel --target wasm32-unknown-unknown`、`swift build`(在 `desktop/macos/bridge`)。Windows 交叉 check 需先在 `desktop/shell/binaries/` 塞占位 `aleph-server-<triple>.exe` 绕过 tauri-build。Linux 交叉 check 被 gtk/wayland sysroot 挡住（写对即可，没法本机编）。

## Gotchas
- 共享卷 `/Volumes/TBU4`：并发 session 会提交/移动 HEAD，`git status` 可能与你预期不符 —— 先 `git log --oneline -3` 看 HEAD。
- zsh：`grep --include='*.rs'` 要引号；`pgrep` 无 `-c`（macOS BSD）。
- 部署后日志：`~/.aleph/logs/gateway.log`、`~/.aleph/logs/aleph-server.log.*`。验证录音落地：`ls ~/.aleph/data/_media/`。
- 先把未提交的层 1-4 提交（`git add` 那 5 个文件），再继续。
```

请先读 `src/gateway/voice/inbound.rs` 和 `src/gateway/inbound_router/mod.rs` 弄清 Panel 怎样作为 channel 把语音消息接入既有 STT→LLM→TTS 管线，再动手。严格遵守 端采集/核处理/端播放 的职责划分。
