# 本地语音 Sidecar（aleph-voice）设计 — Tier 0+1

- **日期**: 2026-06-12
- **状态**: 已与用户逐节确认（§1-§5 全部通过）
- **来源**: 《Aleph使用本地语音模型支持AI TTS和STT服务》(/Volumes/TBU4/技术文章/) + brainstorming 讨论
- **范围**: Tier 0（spike 验证）+ Tier 1（sidecar + core 接线，所有渠道半双工本地语音）

---

## 1. 背景与目标

Aleph 现有语音链路（STT 转写 + TTS 合成）全部依赖云端 API（OpenAI Whisper / OpenAI TTS / ElevenLabs 等）。目标：引入**本地模型**支持，更快、离线可用、零 API 成本，同时遵守"点击唤醒 + 动态释放"的资源策略——不常驻、不抢内存，召之即来挥之即去。

**用户已拍板的产品决策**（来源文档，本设计不重新讨论）：

- 点击唤醒 + 闲置自动释放（非常驻监听），状态机管理生命周期
- 全双工连续对话只在自研客户端（Tier 3，独立子项目）；Telegram 等 IM 渠道走半双工
- 传输协议 WebSocket + PCM/Opus（Tier 3 适用），不上 WebRTC
- AEC 交给系统级 API（Apple VoiceProcessingIO，Tier 3 适用）

**本轮新增决策**（brainstorming 确认）：

| 决策点 | 选择 | 理由 |
|---|---|---|
| 落地形态 | 自研 Rust sidecar 独立进程 | R3 核心轻量化；杀进程 = 确定性还内存；FFI 偏好；externalBin 打包与 aleph-server 一致 |
| 推理引擎 | sherpa-onnx 全家桶（经 sherpa-rs 绑定） | 中文 G2P 已被该库在 C++ 层解决（jieba 词典 + lexicon + espeak-ng-data），零自研；STT/TTS/VAD 单库单运行时单构建链 |
| STT 默认模型 | SenseVoice-small int8（~230MB） | 非自回归，CPU ~100ms 出全文；中文 WER 官方基准优于 whisper-large-v3；自带标点/语种检测；冷启动友好 |
| TTS 默认模型 | Kokoro v1.1-zh（82M 参数，包 ~350MB fp32） | 中英双语；CPU 亚 100ms 合成；G2P 由 sherpa-onnx 内置 |
| 模型分发 | 运行时首次下载，安装包不含权重 | 安装包只增 ~30MB（sidecar 二进制）；多源镜像照顾国内网络 |
| 自研边界 | 服务壳自研（协议/生命周期/下载管理），推理站在 sherpa-onnx 肩上 | 用户明确：参考现成项目降低自研难度 |
| 首轮范围 | Tier 0+1 | Tier 1 独立可交付；Tier 2（流式）/Tier 3（全双工客户端）各自独立 spec |

---

## 2. 总体架构

```
aleph-server (daemon)                          aleph-voice (新·独立 Rust 二进制)
┌──────────────────────────────┐               ┌──────────────────────────────────┐
│ gateway/voice (现有)          │               │ axum loopback HTTP                │
│  inbound ─→ Transcription ────┼──────────────→│  POST /v1/audio/transcriptions    │
│  outbound ─→ GenerationProv ──┼──────────────→│  POST /v1/audio/speech            │
│            (动态 base_url ↗)  │               │  GET  /v1/voice/status            │
│                              │               │  (预留 WS /v1/voice/stream T2/T3) │
│ VoiceSidecarSupervisor (新)  │──spawn/守护──→│                                   │
└──────────────────────────────┘               │ sherpa-onnx 引擎层 (sherpa-rs)     │
                                               │  STT: SenseVoice-small int8       │
                                               │  TTS: Kokoro v1.1-zh              │
                                               └──────────────────────────────────┘
```

### 2.1 关键决策

1. **新 workspace member `aleph-voice/`**（仓库顶层目录，独立 binary crate）。打包与 `aleph-server` 完全一致：Tauri `externalBin`，随三平台 App 分发。二进制 ~30MB（不含模型权重）。
2. **协议选 OpenAI 兼容**。现有 `WhisperTranscription`（`src/media/whisper.rs`）和 `OpenAiTtsProvider`（`src/generation/providers/openai_tts/`）已支持 `base_url` + Bearer `api_key`。sidecar 说同一种方言 → core 的 HTTP 语义零新发明，只补"进程在不在"的管理。
3. **安全握手**：sidecar 绑定 `127.0.0.1:0`（临时端口），自行生成随机 token，启动后向 stdout 打一行：

   ```
   READY {"port":<u16>,"token":"<random>"}
   ```

   supervisor 解析持有。token 直接当 provider 的 `api_key` 字段以 `Authorization: Bearer` 发送——零新协议字段。进程每次重启 token 轮换。固定端口方案被否决（冲突与陈旧进程风险）。
4. **红线对齐**：
   - R1 ✓ — sidecar 是跨平台推理服务而非平台系统 API；core 只持 trait + HTTP client
   - R3 ✓ — sherpa-onnx C++ 重依赖完全隔离在 sidecar crate，alephcore 零新增重依赖
   - R6 ✓ — sidecar 是 daemon 的下属资源，不是新"端"
   - R8 ✓ — 状态查询/预热暴露为 LLM 工具，对话即管理
5. **许可证边界**：espeak-ng（GPLv3）经 sherpa-onnx 静态链入 aleph-voice 二进制。sidecar 是独立进程独立二进制，GPL 边界清晰，不传染主程序；sidecar 自身源码随项目开源即合规。

---

## 3. sidecar 内部设计

```
aleph-voice/
├── server/     axum 路由 + Bearer token 鉴权 + OpenAI 兼容序列化
├── engine/     trait SttEngine / TtsEngine（sherpa-rs 为当前唯一实现）
├── lifecycle/  每引擎状态机 + 闲置定时器 + 进程自退
├── models/     ModelManager: 多源下载 + sha256 + Range 断点续传 + manifest
└── audio/      入: symphonia 解码 + rubato 重采样 → 16kHz mono PCM
                出: wav 直出 + ogg/opus 编码（libopus 绑定 audiopus + ogg crate）
```

### 3.1 HTTP API

| 端点 | 行为 |
|---|---|
| `POST /v1/audio/transcriptions` | OpenAI 兼容 multipart（`file`、`model`、可选 `language`）→ `{"text": "..."}` |
| `POST /v1/audio/speech` | OpenAI 兼容 JSON（`model`/`input`/`voice`/`speed`/`response_format`）→ 音频字节流 |
| `GET /v1/voice/status` | `{stt: {state, model, progress?}, tts: {...}, uptime_secs}` |

- 所有 `/v1/*` 端点要求 `Authorization: Bearer <token>`，失败 401。
- `response_format` 支持 `wav` 与 `opus`（OGG 容器，Telegram 语音气泡原生格式）；其余值返回 400。**不做 mp3**（lame FFI 不值得，core 侧把 local provider 默认 format 配成 opus）。
- 模型 `Downloading` 时业务端点返回 `503 + {"status":"downloading","percent":N}`。

### 3.2 engine trait

`SttEngine` / `TtsEngine` trait 隔离 sherpa-rs：日后换 whisper-rs、加流式 Zipformer，协议层与生命周期层不动。这也是 spike 的保险——若 sherpa-rs 绑定覆盖不足，改为对 sherpa-onnx C API 自写薄绑定（该 C API 专为多语言绑定设计，工作量可控），trait 之上无感。

### 3.3 生命周期状态机

```
Unloaded → Loading → Ready → IdleCountdown → Unloaded
                       ↑______(新请求重置)______|
```

| 参数 | 默认 | 说明 |
|---|---|---|
| TTS 闲置卸载 | 120s | 加载快（~1-2s），留着收益低 |
| STT 闲置卸载 | 600s | SenseVoice 加载 ~1s，可激进卸载 |
| 进程自退 | 1800s 全闲 | 内存归零的终极保证；supervisor 下次需求懒 respawn |

- `Loading`（秒级）期间请求**排队 hold**（15s 超时），用户无感；`Downloading`（分钟级）才返回 503。
- 所有定时器值经配置可调。
- 模型卸载 = drop sherpa context（C++ 析构，确定性回收，spike 实测验证）。

### 3.4 ModelManager

- 多源下载，`auto` 顺序：HuggingFace → hf-mirror.com → ModelScope / sherpa-onnx GitHub releases。
- sha256 钉死校验 + HTTP Range 断点续传；校验失败删除重下一次，二次失败转 error。
- 落盘 `~/.aleph/models/voice/<model-id>/` + `manifest.json`（版本、校验和）。
- 进程自退/重启不付下载成本，只付加载成本。

---

## 4. core 接线（最小改动清单）

1. **`VoiceSidecarSupervisor`**（新，`src/gateway/voice/sidecar.rs`）：进程级单例（`OnceLock`，沿用 SwiftBridge 单例先例）。职责仅三件：`ensure_running()`（懒 spawn + 解析 READY）、持有 `(port, token)`、崩溃检测。**不 eager 启动**。二进制路径解析：与 aleph-server 同目录优先，config 可覆盖。
2. **两个薄 provider 包装**（`provider_type = "local"`）：端口动态分配 → 静态 `base_url` 配置写不死 → 必须包薄层。每次请求向 supervisor 拿当前 `(port, token)`，发 OpenAI 兼容 HTTP（请求体几十行，wrapper 内直接实现，**不重构**现有 provider）。
   - `LocalTranscription` 实现 `TranscriptionService`
   - `LocalTtsProvider` 实现 `GenerationProvider`（`GenerationType::Speech`）
3. **自动注册**：`[voice.local].enabled = true` 时 boot 自动注册两个名为 `"local"` 的 provider；`default_transcription_provider` / `default_speech_provider` 未显式设置时默认指向 local，用户显式配置永远优先。
4. **R8 工具 `local_voice`**：两个 action——`status`（引擎状态/下载进度/磁盘占用）、`warmup`（预拉起 + 双引擎预载）。
5. **预热钩子**：`voice_mode_set(enabled=true)` 时 fire-and-forget 一次 warmup——"点击瞬间异步预热，用户张嘴时模型已就位"落在此处。
6. **本地 / 云端后台切换（显式承诺，2026-06-12 补充需求）**：local 只是注册表中名为 `"local"` 的又一个 provider，云端 provider 配置与行为**原样保留**。切换面：
   - **全局**：`default_speech_provider` / `default_transcription_provider` 设为 `"local"` 或任意云 provider 名，随配置热切换；
   - **每渠道**：现有 `voice_mode_set(provider=...)` 覆盖（TTS），不动；
   - **归一化只填空**：`[voice.local].enabled = true` 且 default 未显式设置时才默认指向 local，**用户显式配置（含云端）永远优先**；
   - **降级回退**：local 故障/下载中时 STT 自动回退到其他已配置云 provider（§5），云→本地方向不自动回退（显式配置即尊重）。
6. **配置**（core 单一来源，spawn 时经 CLI 参数下发，sidecar 无自有配置文件）：

```toml
[voice.local]
enabled = false                 # 默认关
stt_model = "sense-voice-small"
tts_model = "kokoro-v1.1-zh"
tts_voice = "zf_001"            # spike 确认音色表后定稿
tts_format = "opus"
idle_unload_tts_secs = 120
idle_unload_stt_secs = 600
idle_exit_secs = 1800
download_source = "auto"        # auto: HF → hf-mirror → ModelScope/GitHub
```

---

## 5. 降级矩阵（P7，全部不 panic）

| 故障 | 行为 |
|---|---|
| spawn 失败 / 握手超时 | STT：存在其他已配置 transcription provider 则回退之，否则走现有 placeholder 文本路径；TTS：本轮发纯文本 |
| 模型下载中（503） | **不计入** VoiceState 失败计数（绝不触发 3 连败自动关语音）；对话内提示"语音模型下载中 N%" |
| 真推理失败 | 计入现有 3-strike 计数，沿用既有自动禁用行为 |
| sha256 校验失败 | 删除重下一次，二次失败转 error 状态，`local_voice(status)` 可见 |
| 崩溃环路 | 60s 内 3 次崩溃 → 标记 unavailable 300s，期间走降级路径 |
| 断网下载中断 | 状态保留，恢复后 Range 续传 |

**安全**：仅 loopback + per-spawn 随机 token；音频上传沿用 25MB 上限；TTS 文本沿用现有 `sanitize_for_tts`（4000 字符钳制）。Kokoro 单次合成长度上限为 spike 确认项，超限则 sidecar 内部按句切分再拼接。

---

## 6. 测试与验收

### 6.1 Tier 0 spike（实施计划的第一个任务，产出 verdict 文档）

| # | 验证项 | 通过标准 | 不过的回退 |
|---|---|---|---|
| 1 | Kokoro v1.1-zh 中英混合句听感 | 中文自然度可接受；确认音色表；单句延迟与加载时长实测 | 换 MeloTTS-zh_en（同库内，trait 无感） |
| 2 | SenseVoice int8 中英混合转写 | 准确度可接受；延迟实测 | 换 whisper-onnx int8（同库内） |
| 3 | 内存释放 | load → RSS 上升 → drop → RSS 回落（确定性回收） | 改进程级释放策略（缩短自退时间） |
| 4 | sherpa-rs API 覆盖度 | 覆盖 STT/TTS 所需 API | 对 sherpa-onnx C API 自写薄绑定 |

**验收门**：任一 verdict 不过 → 回设计层换模型/换绑定，不带病进实施。

### 6.2 自动化测试

- **单元**（host 跑，零真模型）：lifecycle 状态机纯函数（mock clock：闲置触发/请求重置/Loading 排队/Downloading 503）；ModelManager（sha256 校验、断点续传、多源 fallback，本地 HTTP mock）；协议层（engine trait mock、token 鉴权、错误映射）。
- **集成**：core 侧 supervisor 对假 sidecar 脚本（打 READY 行的 shell 脚本）做握手/崩溃重启/环路守卫；真模型 e2e 标 `#[ignore]` 本地手动跑（CI 不拉 600MB）。

### 6.3 人工验收清单

- [ ] Telegram 语音 round-trip：语音 → 本地转写 → 回复 → 本地 opus 语音气泡
- [ ] Panel voice mode 同链路
- [ ] Activity Monitor 观察 RSS 阶梯回落（TTS 卸载 → STT 卸载 → 进程自退）
- [ ] 断网下载中断 → 恢复续传
- [ ] 首次使用下载进度对话提示

---

## 7. 打包与平台

- **macOS 先行**：aleph-voice 进 justfile 构建链 + tauri externalBin。
- **Linux / Windows 的 sherpa-onnx 静态编译单列为显式收尾任务**，不隐含在"顺便支持"里（CI 平台门控血泪史：`#[cfg(target_os)]` 类问题本地 macOS 看不见）。
- 版本号：sidecar 读同一 VERSION 文件（`env!("ALEPH_VERSION")`），CalVer 同步。

---

## 8. 范围外（后续独立规划）

| 项 | 归属 |
|---|---|
| 流式管线（LLM token 边吐边按句喂 TTS、TG 分段语音、首包优化） | Tier 2，独立 spec |
| 全双工原生客户端（WS 音频流 + Silero VAD + 系统 AEC + 瞬间打断） | Tier 3，独立 spec（产品决策已定：自研客户端独占） |
| Panel 专属下载/管理 UI | 后续轮（本轮对话式 status 够用） |
| 零样本声音克隆（CosyVoice2） | 远期，PyTorch 生态暂无可行 Rust 路径 |
| 流式逐字上屏 STT（Zipformer） | 按需，sherpa-onnx 同库可加 |
| 唤醒词 / 常驻监听 | 明确不做（产品决策：点击唤醒） |

---

## 9. 风险登记

| 风险 | 等级 | 缓解 |
|---|---|---|
| sherpa-rs 绑定覆盖不足 | 中 | spike #4 前置验证；回退自写 C API 薄绑定 |
| Kokoro v1.1-zh 中文听感不达标 | 中 | spike #1 前置验证；回退 MeloTTS-zh_en |
| 模型包尺寸/量化版与知识截止（2026-01）有出入 | 低 | spike 实测为准，spec 中尺寸均为估值 |
| 三平台 sherpa-onnx 静态编译复杂 | 中 | macOS 先行，三平台单列收尾任务 |
| 国内网络下载不稳 | 中 | 多源镜像 + 断点续传 |
| opus/ogg 容器与 TG 语音气泡兼容性 | 低 | 人工验收清单覆盖 |
