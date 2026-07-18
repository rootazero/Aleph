# 流式语音 ASR — 两阶段即时上屏 + AI 规整 设计

**Date**: 2026-06-14
**Status**: Brainstorming 完成,待 spec 评审(用户已逐项拍板:§2 五项决策 + 视觉过渡 C·水波拂过)
**Scope**: Aleph Core (`src/gateway/voice/`) + Panel (`interfaces/webchat/src/views/voice/`)——在**已生产**的沉浸式语音 UI 之上,把单发瀑布式 STT 升级为流式即时上屏
**前置依赖**: 沉浸式语音 UI 已上线(见 `2026-06-13-voice-immersive-ui-design.md`);语音后端已转向 BYO OpenAI 兼容端点(见 `2026-06-12-local-voice-sidecar-design.md` Amendment 1)

---

## 1. 背景与目标

现有语音链路的 STT 是**单发瀑布式**:VAD 判句 → 录完整段 → base64 → `voice.transcribe` RPC → 等待 → 一次性返回整段文字。在全屏沉浸界面里,这形成数百毫秒的"视觉死寂期",用户感到卡顿与不确定。

目标:复刻并超越"闪电说"的零延迟感——**边说边蹦字(灰色 interim)→ 话音落定稿(白色 final)**,定稿瞬间由 prompt 驱动的快模型把口语逐字稿规整成干净书面语。

**这不是重建**。沉浸式 UI(流体球、舞台、单行字幕、VAD、句切分、TTS 播放、打断)已在生产运行;本设计只升级两层:**STT 路径**(单发 → 流式)与**字幕渲染**(单段 → 两阶段)。底层音频采集、球、句切分、TTS、harness **一行不改**。

---

## 2. 已确认的决策(brainstorming 逐项拍板)

| # | 决策点 | 选择 | 理由 |
|---|--------|------|------|
| D1 | 流式 STT 来源 | **定义契约,不打包模型**(BYO 流式端点) | R3 核心轻量化;模型权重在用户自选的端点 |
| D2 | **供应商中立**(硬约束) | Aleph 只造能力,不绑厂商;本地自托管与云端流式**一律平权** | 平台原则;与 R1/P3/P4 同向 |
| D3 | 音频传输拓扑 | **经 core 中转**(Panel→core→STT) | R4 Panel 纯 I/O;STT 端点/密钥只在 core;协议适配集中一处;别的渠道可复用 |
| D4 | 规整引擎 | **一次快模型 LLM 调用,prompt 驱动**(复用 ModelOverride);**不上 FunASR** | R7/R9 智慧在 prompt;STT 自带标点使流式标点层冗余;FunASR 重 Python 依赖违 R3 |
| D5 | 规整语义 | **原文立刻送 Agent**(零额外延迟),规整并行跑,只用于①字幕坍缩动画 ②回填干净文本到历史/记忆 | Agent 本就消化口语;保真 + 清爽存档 + 对话零延迟 |
| D6 | 坍缩过渡动画 | **C · 水波拂过**(一道光波左→右扫过,身后留白字) | 交互样例对比后选定;中庸有方向感 |

---

## 3. 总体架构

```
Panel (已有连续 PCM 采集)        aleph-server (gateway/voice/streaming 新)        BYO STT (用户自选,平权)
┌────────────────────────┐       ┌──────────────────────────────────────┐       ┌─────────────────────────┐
│ MicSession (现有)       │       │ voice.transcribe_stream relay (新)     │       │ WhisperLiveKit (自托管)  │
│  连续 PCM + pre-roll     │       │   ┌──────────────────────────────┐    │       │ Deepgram 云              │
│  16k mono s16le 切块 ───┼──WS──▶│   │ trait StreamingTranscriber   │    │       │ collabora WhisperLive    │
│  ~100-200ms base64 帧    │       │   │  ├ deepgram adapter ──WS──────┼────┼──────▶│  (任一,配置决定)         │
│                          │       │   │  └ whisperlive adapter ──WS───┼────┼──────▶│                          │
│ 字幕渲染 {committed,     │◀─事件─│   归一化 → TranscriptDelta        │    │       └─────────────────────────┘
│  interim} + 水波坍缩 ◀──┼───────│ format.rs 规整(并行,快模型)      │    │
│ 原文 → chat.send (现有) ─┼──────▶│ → 原文送 Agent + 回填规整文本      │    │
└────────────────────────┘       └──────────────────────────────────────┘
```

**关键不变量**:Panel 永远只收 Aleph 归一化的 `TranscriptDelta{committed, interim}` 事件,**对后端是 WhisperLive / WhisperLiveKit / Deepgram 完全无感**。协议差异全部吸收在 core 的 adapter 层。

---

## 4. 供应商中立的落地(D1/D2)

### 4.1 Aleph 契约 = 一个 trait,不是任何厂商的 wire 协议

```rust
// src/gateway/voice/streaming/mod.rs
#[async_trait]
pub trait StreamingTranscriber: Send + Sync {
    /// 打开到后端的流式会话,返回可喂 PCM、产出归一化 delta 的句柄
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<Box<dyn StreamSession>>;
}

#[async_trait]
pub trait StreamSession: Send {
    async fn push_audio(&mut self, pcm_s16le_16k: &[u8]) -> anyhow::Result<()>;
    /// 后端推来的归一化更新流(committed=已锁,interim=漂浮)
    fn deltas(&mut self) -> &mut (dyn Stream<Item = TranscriptDelta> + Unpin + Send);
    async fn finish(&mut self) -> anyhow::Result<()>;
}

/// 后端无关的归一化转写更新
pub struct TranscriptDelta {
    pub committed: String,    // 已锁文本,不会再变
    pub interim: String,      // 漂浮假设,可能被改写
    pub utterance_end: bool,  // 后端是否发了 end-of-utterance(可选,不依赖)
}
```

> 注:`deltas()` 的精确形态(channel vs Stream)留实现计划定,本设计只钉语义。

### 4.2 厂商协议 = 薄 adapter(开放新增,实现 trait 即可,不改 core——OCP)

| adapter | 覆盖后端 | wire 协议要点(已读源码核实) |
|---------|----------|------------------------------|
| `deepgram` | **Deepgram 云 + WhisperLiveKit**(`/v1/listen` 兼容) | WS `?encoding=linear16&sample_rate=16000`;`Results.is_final=true`→committed,`false`→interim;`UtteranceEnd`→utterance_end |
| `whisperlive` | **collabora WhisperLive** native | WS;首发 config 握手 JSON(`uid/model/use_vad/...`);二进制帧;`segments[].completed=true`→累加 committed,末段 `completed=false`→interim |
| (future) `whisperlivekit-asr` | WhisperLiveKit native `/asr` | `FrontData.lines[]`(committed)+`buffer_transcription`(interim);**因 WhisperLiveKit 也说 `/v1/listen`,deepgram adapter 已覆盖,native 暂不必做** |

**两个参考服务器的本质是同一个模型**——"已锁文本 + 漂浮 interim",Aleph 的 `TranscriptDelta` 同时干净映射两者。

### 4.3 配置形状(本地/云端同形,无偏袒)

```toml
[voice.streaming]
enabled  = false                       # 默认关;开启后语音模式优先走流式,否则回落批量
provider = "deepgram"                   # adapter:"deepgram" | "whisperlive"
base_url = "wss://api.deepgram.com"     # 或自托管 "ws://192.168.1.50:9090" —— 同一字段
api_key  = ""                           # 云端填;loopback 自托管留空
language = "zh"

[voice.format]                          # AI 规整(D4/D5)
enabled  = true
# 规整快模型经 ModelOverride 选(本地/云端皆可,不内置);prompt 有默认值,可覆盖
```

**预设 registry** 并列举例 Deepgram 流式(云)、WhisperLiveKit 端点(自托管)、collabora WhisperLive(自托管),**等权呈现**。

### 4.4 批量兜底(P7 + 中立)

`[voice.streaming].enabled=false` 或所配 provider 无流式 adapter → 自动回落现有 `voice.transcribe` 批量路径(录完整段→转写→直接出白字,无灰字阶段)。**流式是能力增益,不是强制**;不强迫任何用户上流式后端。

---

## 5. 数据流(一轮沉浸语音,形态 A)

```
1. 点球 → Listening
   Panel 开 voice.transcribe_stream(新 WS RPC,先发 JSON start{language,...})
   → core 按 [voice.streaming] 配置选 adapter,open() 到 BYO STT

2. 说话(连续)
   MicSession 现有连续 PCM → 重采样 16k mono s16le → ~100-200ms 切块 → base64
   → JSON-RPC notification 帧发 core → relay.push_audio() → adapter → STT
   STT 吐 committed+interim → adapter 归一化 TranscriptDelta
   → core 经 SessionEvent 回吐 Panel
   → Panel 字幕:committed 实色、interim 灰色漂浮(带光标)——【边说边蹦字】

3. Panel 客户端 VAD 判句(现有,静音 ~700ms)→ 话音落定:
   (a) 原文(累计 committed)立刻 chat.send 进当前 session —— Agent 开始思考【零额外延迟】
   (b) Panel 立即对已 commit 文本起【C·水波拂过】(~150ms,灰/interim → 锁定白字)
       —— 这是"定稿"节拍,不等规整;因流式期文字早已在屏,死寂期本就归零
   (c) core format.rs 并行跑规整快模型(精炼师 prompt + ModelOverride)
       → 回 TranscriptFormatted 事件(通常 100-300ms 到)→ Panel 安静淡入替换为规整白字(非二次大动画)
       → 同时回填规整文本到该 user message(走已有 message-updated fanout)→ 历史/记忆清爽

4. Agent 回复流式 → 现有句切分 → 逐句 voice.synthesize → TtsQueue 播放 + 打断 —— 全不改

5. 降级(P7):规整超时(>1.5s)/失败 → 保留已 commit 原文当最终白字,不卡循环;
   STT 流断 → 当轮回落批量 voice.transcribe,下轮重试流式
```

> **存档语义说明(D5)**:Agent 实际读到的是原文(它本就消化口语);字幕显示与历史/记忆存的是规整版。二者刻意分离——保真送达 + 清爽留痕,且对话关键路径零额外 LLM 延迟。

---

## 6. AI 规整(D4/D5)

- **位置**:`src/gateway/voice/format.rs`(core,小)。不在 Panel(R4 纯 I/O)。
- **触发**:话音落定时,对累计原文跑**一次**快模型调用。与"原文送 Agent"**并行**,不阻塞 Agent。
- **模型**:经 **ModelOverride** 指定的快模型(本地/云端中立,**不内置 Qwen/llama.cpp**)。
- **默认 prompt**(用户可覆盖):

  ```
  你是一个冷酷的语音实时格式化微型引擎。请将以下口语化的逐字稿转化为
  排版优雅、逻辑清晰、无语气词的正式书面语。
  【硬性要求】
  1. 绝对不能回答用户的提问,只能对文本进行润色和纠错。
  2. 剔除所有"额、啊、那个、就是、然后"等口语冗余。
  3. 补全错别字和缺失的标点。
  4. 如果文本本身已经很清晰,原样输出。
  输入:[原始转写文本]
  ```
  低温 + 短 max_tokens,压制改写原意与发散。

- **不做**:FunASR/CT-Transformer 流式标点层(STT 自带标点 → 冗余;重 Python 依赖 → 违 R3)。

---

## 7. 视觉层(D6:C·水波拂过)

- **interim**:字幕区灰色(`--color-muted` 量级),逐增上屏,末尾闪烁光标;允许末几字抖动(本就是漂浮假设)。
- **committed**:转为准白实色(尚未规整)。
- **话音落定 → C·水波拂过**:一道主题色光波从左扫到右(`clip-path: inset()` 推进 + 一条 `screen` 混合的 sheen 带),身后把灰/interim **锁定为白字**;约 **150ms**。**水波是"定稿"节拍,不 gate 在规整 LLM 上**——流式期文字早已在屏(死寂期本就归零);规整白字随后(~100-300ms)安静淡入替换,不打断节奏。
- **球**:落定瞬间一次能量脉冲(呼应"话音落")。
- 复用现有 orb / stage / 五色板 / 玻璃主题;**纯 CSS + 音频驱动变量**,JS 只写信号。
- **a11y**:`prefers-reduced-motion` → 光波退化为直接淡入(无扫掠);`prefers-reduced-transparency` → 去 sheen 混合,纯色替换。
- 实现期产出 standalone HTML mock + chrome-devtools 真机比对(沉浸 UI spec §7 同款验收)。

---

## 8. 改动足迹

| 类别 | 位置 |
|------|------|
| **新增** | `src/gateway/voice/streaming/{mod.rs(trait+relay), deepgram.rs, whisperlive.rs}` |
| | `src/gateway/voice/format.rs`(规整,小) |
| **扩展** | `src/gateway/handlers/voice.rs` — 加 `voice.transcribe_stream` WS relay handler + `TranscriptDelta`/`TranscriptFormatted` 事件 |
| | `interfaces/webchat/src/views/voice/audio.rs` — PCM 重采样 16k s16le + 切块 base64 上行 |
| | `interfaces/webchat/src/views/voice/mod.rs` — 流式订阅 + 字幕两阶段状态 + 话音落定钩子 |
| | `interfaces/webchat/styles/tailwind.css` — 灰/白字层 + 水波坍缩 keyframes |
| | 配置类型 + 预设 registry — `[voice.streaming]`/`[voice.format]` + 三个并列预设 |
| **零改动** | orb / VAD 纯函数 / 句切分 / TTS 播放 / pre-roll 采集 / harness |

**传输细节**:Panel→core 音频走**现有 JSON-RPC 通道**的 notification(base64 帧,16k s16le 100ms ≈ 3.2KB → base64 ≈ 4.3KB,可忽略),**不新开二进制 WS 通道**(R10/R3 极简;二进制传输留作后续可选优化)。

---

## 9. 降级矩阵(P7,全部不 panic)

| 故障 | 行为 |
|------|------|
| `streaming.enabled=false` / provider 无流式 adapter | 回落批量 `voice.transcribe`(无灰字,体验等同今天) |
| STT 流连接失败 / 中途断 | 当轮回落批量;下轮重试流式;字幕提示"重连中" |
| 规整快模型超时(>1.5s)/失败 | 水波定稿照常(已 commit 原文即最终白字),只是没有后续规整替换;Agent 不受影响(已收原文) |
| Panel VAD 误判 | 沿用现有 700ms hangover + 最小句长;流式下额外可参考 STT `utterance_end` 辅助(不强依赖) |
| 麦克风权限/切后台 | 沿用现有沉浸 UI 降级(球变灰/暂停 Listening) |

**安全**:STT 端点/密钥只在 core 配置,浏览器永不接触;音频 base64 帧走现有 loopback/LAN-trust 边界,不新增信任面。

---

## 10. 红线自检

- **R1** ✓ core 持 `StreamingTranscriber` trait,厂商实现在 adapter,非平台系统 API
- **R3** ✓ 零新增重依赖;模型权重在用户 BYO 端点;不打包 WhisperLive/llama.cpp/FunASR
- **R4** ✓ Panel 纯 I/O,只渲染归一化事件;协议适配/规整/路由全在 core
- **R6** ✓ 流式中转是 daemon 能力,别的渠道可复用
- **R7/R9** ✓ 规整靠 prompt 不靠确定性代码;无意图分类/工具过滤
- **R10** ✓ harness 一行不碰;relay/format 落在 gateway(I/O 层)非 harness
- **D2 中立** ✓ trait+adapter+同形配置+并列预设+批量兜底,本地与云端等权

---

## 11. 测试

1. `TranscriptDelta` 归一化纯函数单测:deepgram `Results`/`UtteranceEnd` 映射、whisperlive `segments[].completed` 累加(host 跑,假报文,零真模型)
2. relay 状态机单测:open/push/finish、流断回落、规整超时降级(mock adapter + mock clock)
3. 规整 prompt 调用单测:mock provider,验证"只润色不回答"硬约束、空输入/已干净输入原样返回
4. Panel:字幕两阶段状态机纯函数单测(interim→committed→规整白字、水波过渡触发);`cargo build --target wasm32-unknown-unknown` + clippy
5. 视觉验收:standalone HTML + chrome-devtools,三材质×五色板下水波坍缩截图
6. 人工 E2E:真麦 + 自托管 WhisperLiveKit + Deepgram 云**双后端**各跑一遍(验证中立),含打断、超时降级、批量回落

---

## 12. 范围外(YAGNI,后续独立)

- 真全双工 WebRTC 流式通道(用户既有决策:全双工仅自研客户端)
- Panel→core **二进制**音频通道(本设计用 base64,够用;二进制是后续可选优化)
- 唤醒词 / 常驻监听(用户既有决策:点击唤醒)
- 文字粒子坍缩 / 毛玻璃过渡(交互样例已选 C·水波;其余作二期可选皮肤)
- 逐字时间戳精确字幕对齐
- Telegram 等 IM 渠道的流式上屏(批量已够,IM 无沉浸字幕场景)
- WhisperLiveKit native `/asr` adapter(deepgram `/v1/listen` 已覆盖)
- 说话人分离(diarization)/ 翻译流(两参考服务器支持,但非本设计目标)
