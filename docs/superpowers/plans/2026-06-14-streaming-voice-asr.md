# 流式语音 ASR — 两阶段即时上屏 + AI 规整 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在已生产的沉浸式语音 UI 之上,把单发瀑布式 STT 升级为"边说边蹦灰字 → 话音落水波定稿白字 + AI 规整"。

**Architecture:** Core 定义 `StreamingTranscriber` trait + 厂商薄 adapter(deepgram/whisperlive,供应商中立),一个 per-stream 中转 task 把 Panel 上行的 PCM 帧桥到 BYO STT 的 WebSocket,再把归一化的 `TranscriptDelta{committed,interim}` 经 `event_bus` 的 `voice.transcribe.delta` 主题推回 Panel。Panel 两阶段渲染(灰 interim / 白 committed)+ 话音落定的 C·水波拂过坍缩。AI 规整是独立 `voice.format` RPC(快模型 + prompt),只做显示层润色,不阻塞 Agent。

**Tech Stack:** Rust(alephcore:axum/tokio/serde/reqwest;新增 WS 客户端 tokio-tungstenite)、Leptos/WASM(Panel,web-sys/AudioContext)、CSS(tailwind 自定义 keyframes)。

**设计依据:** `docs/superpowers/specs/2026-06-14-streaming-voice-asr-design.md`(已评审)。

**分三阶段,每阶段独立可交付可测:**
- **Phase 1**(Task 1–6):Core 流式引擎(trait + adapter 归一化 + 中转 relay + config + 预设)。归一化是纯函数 TDD 主体。
- **Phase 2**(Task 7–10):Panel 两阶段渲染 + 水波坍缩。交付可见的"闪电说"效果。
- **Phase 3**(Task 11–12):AI 规整(显示层润色)。

**全局约定:**
- 核心测试:`cargo test -p alephcore --lib <name>`;Panel 编译:`cargo build -p alephcore`(server)/ `cargo build --target wasm32-unknown-unknown`(panel)。
- 红线:harness 一行不碰(R10);Panel 纯渲染(R4);本地/云端等权(D2)。
- **用户对 cargo 调用极度节制**:每个 commit 前只跑该 task 涉及的**单个**测试(`--lib <name>`),不跑全量 `--tests`/`--all-targets`,除非被显式要求。

---

## File Structure

**新增(Core):**
- `src/gateway/voice/streaming/mod.rs` — `StreamingTranscriber` trait、`TranscriptDelta`/`StreamConfig`/`StreamHandles` 类型、adapter 工厂 `build_transcriber()`
- `src/gateway/voice/streaming/deepgram.rs` — Deepgram `/v1/listen` 协议 adapter(覆盖 Deepgram 云 + WhisperLiveKit)+ 纯归一化 `normalize_deepgram()`
- `src/gateway/voice/streaming/whisperlive.rs` — collabora WhisperLive 协议 adapter + 纯归一化 `WhisperLiveDecoder`
- `src/gateway/voice/streaming/relay.rs` — per-stream 中转 task:Panel 帧 → backend WS → `voice.transcribe.delta` TopicEvent
- `src/gateway/voice/format.rs` — AI 规整(快模型一次性调用)

**修改(Core):**
- `src/gateway/handlers/voice.rs` — 加 `voice.stream.start` / `voice.stream.audio` / `voice.stream.stop` / `voice.format` 四个 handler
- `src/gateway/handlers/mod.rs`(或控制面 router 注册处) — 注册上述方法
- `src/config/types/voice/`(或 generation config) — 加 `StreamingConfig`(`[voice.streaming]`)+ `FormatConfig`(`[voice.format]`)
- `src/config/types/generation/presets/registry.rs` — 加流式 STT 预设(deepgram-stream / whisperlivekit / whisperlive,等权)
- `Cargo.toml` — 若无,加 `tokio-tungstenite`

**修改(Panel):**
- `interfaces/webchat/src/views/voice/mod.rs` — `Caption` 加 `Streaming{committed,interim}`/`Locked`;流式会话 start/stop;订阅 delta;话音落水波触发;调用 `voice.format`
- `interfaces/webchat/src/views/voice/audio.rs` — 暴露 16k mono s16le 帧回调(切块 base64 上行)
- `interfaces/webchat/src/views/voice/caption_state.rs`(新) — 纯函数 reducer:`apply_delta(state, TranscriptDelta) -> CaptionState`(host 可测)
- `interfaces/webchat/styles/tailwind.css` — 灰/白字层 + C·水波 keyframes + reduced-motion 降级

---

# Phase 1 — Core 流式引擎

## Task 1: 流式核心类型 + trait

**Files:**
- Create: `src/gateway/voice/streaming/mod.rs`
- Modify: `src/gateway/voice/mod.rs`(加 `pub mod streaming;`)

- [ ] **Step 1: 写失败测试**(放在 `mod.rs` 文件尾 `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_delta_committed_only_serializes_snake_case() {
        let d = TranscriptDelta { committed: "你好".into(), interim: String::new(), utterance_end: false };
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["committed"], "你好");
        assert_eq!(j["interim"], "");
        assert_eq!(j["utterance_end"], false);
    }

    #[test]
    fn stream_config_defaults_to_16k() {
        let c = StreamConfig::new(None);
        assert_eq!(c.sample_rate, 16_000);
        assert!(c.language.is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::tests`
Expected: FAIL — `cannot find type TranscriptDelta`.

- [ ] **Step 3: 写最小实现**(`mod.rs` 顶部)

```rust
//! Provider-neutral streaming transcription contract.
//!
//! Core defines the `{committed, interim}` semantics; vendor protocols live in
//! per-adapter submodules. Panel never sees a vendor wire format — only the
//! normalized [`TranscriptDelta`] pushed over the `voice.transcribe.delta` topic.

pub mod deepgram;
pub mod relay;
pub mod whisperlive;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Backend-agnostic transcript update. `committed` is locked (won't change);
/// `interim` is the floating hypothesis (may be rewritten next delta).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TranscriptDelta {
    #[serde(default)]
    pub committed: String,
    #[serde(default)]
    pub interim: String,
    /// Backend explicitly signaled end-of-utterance (best-effort; Panel VAD is
    /// authoritative for turn segmentation, this is advisory only).
    #[serde(default)]
    pub utterance_end: bool,
}

/// Per-session open parameters handed to an adapter.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub sample_rate: u32,
    pub language: Option<String>,
}

impl StreamConfig {
    #[must_use]
    pub fn new(language: Option<String>) -> Self {
        Self { sample_rate: 16_000, language }
    }
}

/// Channels bridging the relay and a backend session: relay pushes s16le audio
/// frames into `audio_tx`; backend deltas arrive on `delta_rx`.
pub struct StreamHandles {
    pub audio_tx: mpsc::Sender<Vec<u8>>,
    pub delta_rx: mpsc::Receiver<TranscriptDelta>,
}

/// The Aleph contract. Each vendor protocol implements this; `build_transcriber`
/// picks the impl from config (provider-neutral — local self-host and cloud are
/// just different `base_url`/`provider` values).
#[async_trait]
pub trait StreamingTranscriber: Send + Sync {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles>;
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::tests`
Expected: PASS（若 `deepgram`/`whisperlive`/`relay` 子模块尚未建,先在 Step 3 暂时注释 `pub mod` 三行,Task 2/3/4 各自补回并解注释。提交时三模块齐全。）

- [ ] **Step 5: Commit**

```bash
git add src/gateway/voice/streaming/mod.rs src/gateway/voice/mod.rs
git commit -m "feat(voice): streaming transcriber contract (TranscriptDelta + trait)"
```

---

## Task 2: Deepgram adapter 归一化(纯函数 TDD)

覆盖 Deepgram 云 + WhisperLiveKit(`/v1/listen` 兼容)。先做**纯归一化**(可测),WS 连接在 Task 4。

**Files:**
- Create: `src/gateway/voice/streaming/deepgram.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn results(is_final: bool, transcript: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "Results",
            "is_final": is_final,
            "channel": { "alternatives": [ { "transcript": transcript } ] }
        })
    }

    #[test]
    fn final_result_accumulates_into_committed() {
        let mut dec = DeepgramDecoder::default();
        let d = dec.push(&results(true, "你好")).unwrap();
        assert_eq!(d.committed, "你好");
        assert_eq!(d.interim, "");
        // second final appends with a space-joined growth
        let d = dec.push(&results(true, "世界")).unwrap();
        assert_eq!(d.committed, "你好 世界");
    }

    #[test]
    fn interim_result_is_floating_not_committed() {
        let mut dec = DeepgramDecoder::default();
        let _ = dec.push(&results(true, "你好"));
        let d = dec.push(&results(false, "世")).unwrap();
        assert_eq!(d.committed, "你好");
        assert_eq!(d.interim, "世");
    }

    #[test]
    fn utterance_end_message_sets_flag() {
        let mut dec = DeepgramDecoder::default();
        let msg = serde_json::json!({ "type": "UtteranceEnd", "last_word_end": 3.4 });
        let d = dec.push(&msg).unwrap();
        assert!(d.utterance_end);
    }

    #[test]
    fn empty_transcript_interim_is_ignored() {
        let mut dec = DeepgramDecoder::default();
        assert!(dec.push(&results(false, "")).is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::deepgram`
Expected: FAIL — `cannot find DeepgramDecoder`.

- [ ] **Step 3: 写最小实现**

```rust
//! Deepgram `/v1/listen` streaming protocol adapter.
//! Covers Deepgram cloud AND self-hosted WhisperLiveKit (`/v1/listen` compat).

use super::TranscriptDelta;

/// Stateful normalizer: folds Deepgram `Results`/`UtteranceEnd` JSON messages
/// into the Aleph `{committed, interim}` model. Pure — no I/O, host-testable.
#[derive(Default)]
pub struct DeepgramDecoder {
    committed: String,
}

impl DeepgramDecoder {
    /// Returns `None` for non-transcript / empty messages.
    pub fn push(&mut self, msg: &serde_json::Value) -> Option<TranscriptDelta> {
        match msg.get("type").and_then(|t| t.as_str()) {
            Some("UtteranceEnd") => Some(TranscriptDelta {
                committed: self.committed.clone(),
                interim: String::new(),
                utterance_end: true,
            }),
            Some("Results") | None => {
                let is_final = msg.get("is_final").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let text = msg
                    .pointer("/channel/alternatives/0/transcript")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim();
                if text.is_empty() {
                    return None;
                }
                if is_final {
                    if self.committed.is_empty() {
                        self.committed = text.to_string();
                    } else {
                        self.committed.push(' ');
                        self.committed.push_str(text);
                    }
                    Some(TranscriptDelta { committed: self.committed.clone(), interim: String::new(), utterance_end: false })
                } else {
                    Some(TranscriptDelta { committed: self.committed.clone(), interim: text.to_string(), utterance_end: false })
                }
            }
            _ => None,
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::deepgram`
Expected: PASS（4 个测试全过）。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/voice/streaming/deepgram.rs
git commit -m "feat(voice): Deepgram /v1/listen delta normalizer (cloud + WhisperLiveKit)"
```

---

## Task 3: WhisperLive adapter 归一化(纯函数 TDD)

覆盖 collabora WhisperLive(`segments[].completed` 协议)。

**Files:**
- Create: `src/gateway/voice/streaming/whisperlive.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, completed: bool) -> serde_json::Value {
        serde_json::json!({ "start": "0.0", "end": "1.0", "text": text, "completed": completed })
    }
    fn msg(segs: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "segments": segs })
    }

    #[test]
    fn completed_segments_lock_into_committed_last_floats() {
        let mut dec = WhisperLiveDecoder::default();
        // two completed + one trailing interim
        let d = dec
            .push(&msg(vec![seg("你好", true), seg("世界", true), seg("现", false)]))
            .unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "现");
    }

    #[test]
    fn committed_dedup_does_not_double_count_across_messages() {
        let mut dec = WhisperLiveDecoder::default();
        let _ = dec.push(&msg(vec![seg("你好", true), seg("在", false)]));
        // next message re-sends the same completed segment (send_last_n_segments window)
        let d = dec.push(&msg(vec![seg("你好", true), seg("世界", true), seg("吗", false)])).unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "吗");
    }

    #[test]
    fn server_ready_message_is_ignored() {
        let mut dec = WhisperLiveDecoder::default();
        let ready = serde_json::json!({ "message": "SERVER_READY", "backend": "faster_whisper" });
        assert!(dec.push(&ready).is_none());
    }
}
```

> 设计说明:WhisperLive 每条消息回送 `send_last_n_segments`(默认 10)个 completed + 1 interim,**会重发已 completed 段**。去重按"已吸收的 completed 段数量"推进:committed 重建为本条消息中所有 `completed=true` 段拼接(它们单调增长且稳定),interim 取末尾 `completed=false` 段。重建优于增量,天然抗重发。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::whisperlive`
Expected: FAIL — `cannot find WhisperLiveDecoder`.

- [ ] **Step 3: 写最小实现**

```rust
//! collabora WhisperLive native protocol adapter (`segments[].completed`).

use super::TranscriptDelta;

#[derive(Default)]
pub struct WhisperLiveDecoder {
    /// All completed segment texts seen so far, in order. WhisperLive resends a
    /// trailing window of completed segments, so we rebuild from the union by
    /// appending only segments we have not locked yet.
    committed_segments: Vec<String>,
}

impl WhisperLiveDecoder {
    pub fn push(&mut self, msg: &serde_json::Value) -> Option<TranscriptDelta> {
        let segs = msg.get("segments").and_then(|s| s.as_array())?;
        let mut interim = String::new();
        let mut completed_this_msg: Vec<String> = Vec::new();
        for s in segs {
            let text = s.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
            if text.is_empty() {
                continue;
            }
            let completed = s.get("completed").and_then(serde_json::Value::as_bool).unwrap_or(false);
            if completed {
                completed_this_msg.push(text);
            } else {
                interim = text; // last interim wins
            }
        }
        // Lock any newly-completed segments not yet absorbed (monotonic growth).
        for (i, text) in completed_this_msg.iter().enumerate() {
            match self.committed_segments.get(i) {
                Some(existing) if existing == text => {}
                _ => {
                    if i < self.committed_segments.len() {
                        self.committed_segments[i] = text.clone();
                    } else {
                        self.committed_segments.push(text.clone());
                    }
                }
            }
        }
        if completed_this_msg.is_empty() && interim.is_empty() {
            return None;
        }
        Some(TranscriptDelta {
            committed: self.committed_segments.concat(),
            interim,
            utterance_end: false,
        })
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::whisperlive`
Expected: PASS（3 个测试全过）。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/voice/streaming/whisperlive.rs
git commit -m "feat(voice): WhisperLive segments[].completed delta normalizer"
```

---

## Task 4: adapter WS 连接 + 工厂 build_transcriber

把 Task 2/3 的纯归一化接上真实 WS。连接逻辑难以纯单测,提供完整实现 + `#[ignore]` 真端点 e2e + host 可测的工厂选择逻辑。

**Files:**
- Modify: `src/gateway/voice/streaming/mod.rs`(加 `build_transcriber` + `StreamingProvider` enum)
- Modify: `src/gateway/voice/streaming/deepgram.rs`(impl `StreamingTranscriber`)
- Modify: `src/gateway/voice/streaming/whisperlive.rs`(impl `StreamingTranscriber`)
- Modify: `Cargo.toml`(若无 `tokio-tungstenite`)

- [ ] **Step 1: 确认/添加 WS 客户端依赖**

Run: `grep -n "tokio-tungstenite\|tungstenite" /Volumes/TBU4/Workspace/Aleph/Cargo.toml`
若无,在 `[dependencies]` 加(标准 WS 客户端,属 I/O 基础设施,符合 R3):
```toml
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
```
> 决策点:`tokio-tungstenite` 是 WS **客户端**(gateway 自身的 axum 是 WS server,不复用);轻量标准件,非"为单一功能引重库"。若 workspace 已有等价 WS 客户端,优先复用,跳过添加。

- [ ] **Step 2: 写工厂选择的失败测试**(`mod.rs` tests)

```rust
#[test]
fn build_transcriber_picks_adapter_by_provider() {
    let cfg = StreamingTarget { provider: "whisperlive".into(), base_url: "ws://127.0.0.1:9090".into(), api_key: String::new(), language: None };
    assert!(matches!(classify_provider(&cfg.provider), StreamingProvider::WhisperLive));
    let cfg2 = StreamingTarget { provider: "deepgram".into(), base_url: "wss://api.deepgram.com".into(), api_key: "k".into(), language: None };
    assert!(matches!(classify_provider(&cfg2.provider), StreamingProvider::Deepgram));
    // unknown defaults to deepgram (the lingua-franca protocol)
    assert!(matches!(classify_provider("mystery"), StreamingProvider::Deepgram));
}
```

- [ ] **Step 3: 跑确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::tests::build_transcriber_picks_adapter_by_provider`
Expected: FAIL — `cannot find StreamingTarget`.

- [ ] **Step 4: 实现工厂 + 两个 adapter 的 `open()`**

`mod.rs` 追加:
```rust
/// Resolved streaming target (from `[voice.streaming]` config, provider-neutral).
#[derive(Debug, Clone)]
pub struct StreamingTarget {
    pub provider: String,   // "deepgram" | "whisperlive"
    pub base_url: String,
    pub api_key: String,
    pub language: Option<String>,
}

pub enum StreamingProvider { Deepgram, WhisperLive }

#[must_use]
pub fn classify_provider(provider: &str) -> StreamingProvider {
    match provider.trim().to_ascii_lowercase().as_str() {
        "whisperlive" => StreamingProvider::WhisperLive,
        // "deepgram", "whisperlivekit", unknown → Deepgram /v1/listen lingua franca
        _ => StreamingProvider::Deepgram,
    }
}

#[must_use]
pub fn build_transcriber(t: StreamingTarget) -> Box<dyn StreamingTranscriber> {
    match classify_provider(&t.provider) {
        StreamingProvider::Deepgram => Box::new(deepgram::DeepgramStream::new(t)),
        StreamingProvider::WhisperLive => Box::new(whisperlive::WhisperLiveStream::new(t)),
    }
}
```

`deepgram.rs` 追加(WS 连接 + select 循环;`DeepgramStream::new(StreamingTarget)`):
```rust
use super::{StreamConfig, StreamHandles, StreamingTarget, StreamingTranscriber, TranscriptDelta};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

pub struct DeepgramStream { target: StreamingTarget }
impl DeepgramStream { pub fn new(target: StreamingTarget) -> Self { Self { target } } }

#[async_trait]
impl StreamingTranscriber for DeepgramStream {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles> {
        // /v1/listen with linear16/16k/interim_results/utterance_end so the
        // server emits BOTH interim and final + UtteranceEnd.
        let base = self.target.base_url.trim_end_matches('/');
        let host = base.replace("https://", "wss://").replace("http://", "ws://");
        let lang = cfg.language.or_else(|| self.target.language.clone()).unwrap_or_default();
        let mut url = format!(
            "{host}/v1/listen?encoding=linear16&sample_rate={}&channels=1&interim_results=true&utterance_end_ms=1000",
            cfg.sample_rate
        );
        if !lang.is_empty() { url.push_str(&format!("&language={lang}")); }

        let mut req = url.into_client_request()?;
        if !self.target.api_key.is_empty() {
            req.headers_mut()
                .insert("Authorization", format!("Token {}", self.target.api_key).parse()?);
        }
        let (ws, _) = tokio_tungstenite::connect_async(req).await?;
        let (mut sink, mut stream) = ws.split();

        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(64);
        let (delta_tx, delta_rx) = mpsc::channel::<TranscriptDelta>(64);

        tokio::spawn(async move {
            let mut dec = DeepgramDecoder::default();
            loop {
                tokio::select! {
                    frame = audio_rx.recv() => match frame {
                        Some(bytes) => { if sink.send(Message::Binary(bytes)).await.is_err() { break; } }
                        None => { // Panel closed: send Deepgram "CloseStream" then finish
                            let _ = sink.send(Message::Text("{\"type\":\"CloseStream\"}".into())).await;
                            break;
                        }
                    },
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                                if let Some(d) = dec.push(&v) { if delta_tx.send(d).await.is_err() { break; } }
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    }
                }
            }
        });
        Ok(StreamHandles { audio_tx, delta_rx })
    }
}
```

`whisperlive.rs` 追加(`WhisperLiveStream::new` + config 握手 + int16 帧):
```rust
use super::{StreamConfig, StreamHandles, StreamingTarget, StreamingTranscriber, TranscriptDelta};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct WhisperLiveStream { target: StreamingTarget }
impl WhisperLiveStream { pub fn new(target: StreamingTarget) -> Self { Self { target } } }

#[async_trait]
impl StreamingTranscriber for WhisperLiveStream {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles> {
        let url = self.target.base_url.replace("https://", "wss://").replace("http://", "ws://");
        let (ws, _) = tokio_tungstenite::connect_async(url).await?;
        let (mut sink, mut stream) = ws.split();
        // WhisperLive config handshake (first message). audio_format=int16 so we
        // can forward s16le frames verbatim.
        let handshake = serde_json::json!({
            "uid": uuid::Uuid::new_v4().to_string(),
            "language": cfg.language.or_else(|| self.target.language.clone()),
            "task": "transcribe",
            "model": "small",
            "use_vad": true,
            "send_last_n_segments": 10,
            "audio_format": "int16"
        });
        sink.send(Message::Text(handshake.to_string())).await?;

        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(64);
        let (delta_tx, delta_rx) = mpsc::channel::<TranscriptDelta>(64);
        tokio::spawn(async move {
            let mut dec = WhisperLiveDecoder::default();
            loop {
                tokio::select! {
                    frame = audio_rx.recv() => match frame {
                        Some(bytes) => { if sink.send(Message::Binary(bytes)).await.is_err() { break; } }
                        None => { let _ = sink.send(Message::Binary(b"END_OF_AUDIO".to_vec())).await; break; }
                    },
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                                if let Some(d) = dec.push(&v) { if delta_tx.send(d).await.is_err() { break; } }
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    }
                }
            }
        });
        Ok(StreamHandles { audio_tx, delta_rx })
    }
}
```

> `futures_util` 已是 workspace 依赖(gateway 用 axum/tokio 流);若缺则补。`uuid` 已用于 events.rs。

- [ ] **Step 5: 跑工厂测试确认通过 + 写 `#[ignore]` 真端点 e2e**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::tests::build_transcriber_picks_adapter_by_provider`
Expected: PASS。

追加(本地手动跑,CI 不连真服务):
```rust
#[tokio::test]
#[ignore = "needs a live WhisperLiveKit at WL_URL; run manually"]
async fn deepgram_adapter_round_trips_against_whisperlivekit() {
    let url = std::env::var("WL_URL").unwrap(); // e.g. ws://127.0.0.1:8000
    let t = StreamingTarget { provider: "deepgram".into(), base_url: url, api_key: String::new(), language: Some("zh".into()) };
    let tr = build_transcriber(t);
    let mut h = tr.open(StreamConfig::new(Some("zh".into()))).await.unwrap();
    // feed 1s of silence (s16le 16k) then expect the stream to stay alive
    h.audio_tx.send(vec![0u8; 32_000]).await.unwrap();
    drop(h.audio_tx);
    // (manual) observe deltas on h.delta_rx
    let _ = h.delta_rx.recv().await;
}
```

- [ ] **Step 6: Commit**

```bash
git add src/gateway/voice/streaming/ Cargo.toml Cargo.lock
git commit -m "feat(voice): WS connect for deepgram + whisperlive adapters; build_transcriber factory"
```

---

## Task 5: 配置 `[voice.streaming]` + `[voice.format]` + 流式预设

**Files:**
- Modify: 语音配置类型(参照现有 `src/config/types/voice/` 或 `generation` config — 实现时 `grep "default_transcription_provider"` 定位)
- Modify: `src/config/types/generation/presets/registry.rs`

- [ ] **Step 1: 写失败测试**(配置类型所在文件 tests)

```rust
#[test]
fn streaming_config_defaults_disabled_and_neutral() {
    let c: StreamingConfig = toml::from_str("").unwrap();
    assert!(!c.enabled);
    assert_eq!(c.provider, "deepgram"); // lingua-franca default protocol, NOT a vendor preference
}

#[test]
fn streaming_config_accepts_self_hosted_endpoint() {
    let c: StreamingConfig = toml::from_str(
        "enabled = true\nprovider = \"whisperlive\"\nbase_url = \"ws://192.168.1.50:9090\"\n",
    ).unwrap();
    assert!(c.enabled);
    assert_eq!(c.base_url, "ws://192.168.1.50:9090");
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p alephcore --lib streaming_config_defaults_disabled_and_neutral`
Expected: FAIL — `cannot find StreamingConfig`.

- [ ] **Step 3: 实现配置类型**

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StreamingConfig {
    pub enabled: bool,
    /// Protocol adapter: "deepgram" (covers Deepgram cloud + WhisperLiveKit) | "whisperlive".
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub language: Option<String>,
}
impl Default for StreamingConfig {
    fn default() -> Self {
        Self { enabled: false, provider: "deepgram".into(), base_url: String::new(), api_key: String::new(), language: None }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FormatConfig {
    pub enabled: bool,
    /// Fast model for the "言语精炼师" pass, via ModelOverride::from_voice(provider, model).
    pub provider: String,
    pub model: String,
    /// Override the default system prompt (empty → built-in default).
    pub prompt: String,
}
impl Default for FormatConfig {
    fn default() -> Self {
        Self { enabled: true, provider: String::new(), model: String::new(), prompt: String::new() }
    }
}
```
把这两个挂进 `VoiceConfig`(或顶层 config 的 `[voice]`)作为 `pub streaming: StreamingConfig` / `pub format: FormatConfig`。

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p alephcore --lib streaming_config`
Expected: PASS。

- [ ] **Step 5: 加流式预设(等权,本地/云端并列)**

在 `registry.rs` 预设表追加(镜像现有 `deepgram-stt` 条目风格):
```rust
(
    "deepgram-stream",
    GenerationPreset::new("deepgram_stream", "nova-3", Some("wss://api.deepgram.com"))
        .with_modalities(TRANSCRIPTION)
        .with_display("Deepgram 流式 STT（云）")
        .with_description("Deepgram Nova-3 实时流式 /v1/listen")
        .with_homepage("https://developers.deepgram.com"),
),
(
    "whisperlivekit",
    GenerationPreset::new("deepgram_stream", "whisper", None)
        .with_modalities(TRANSCRIPTION)
        .with_display("WhisperLiveKit（自托管）")
        .with_description("自托管流式 STT，/v1/listen 兼容（填 base_url 指向你的实例）"),
),
(
    "whisperlive",
    GenerationPreset::new("whisperlive", "small", None)
        .with_modalities(TRANSCRIPTION)
        .with_display("WhisperLive（自托管）")
        .with_description("collabora WhisperLive，segments 协议（填 base_url 指向你的实例）"),
),
```
> 顺序与措辞对本地/云端等权呈现(D2)。

- [ ] **Step 6: 跑预设测试 + Commit**

Run: `cargo test -p alephcore --lib presets`(若有预设表完整性测试)
Expected: PASS。

```bash
git add src/config
git commit -m "feat(voice): [voice.streaming] + [voice.format] config and neutral streaming presets"
```

---

## Task 6: relay + `voice.stream.*` RPC + TopicEvent 推送 + 注册

把 Panel 上行帧桥到 backend,把 delta 经 `voice.transcribe.delta` 主题推回。每流一个 `stream_id`。

**Files:**
- Create: `src/gateway/voice/streaming/relay.rs`
- Modify: `src/gateway/handlers/voice.rs`(4 个 handler)
- Modify: 方法注册处(`grep -rn '"voice.transcribe"' src/gateway` 定位 dispatch)

- [ ] **Step 1: relay 注册表纯函数测试**(`relay.rs` tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn registry_start_returns_id_and_stop_removes() {
        let reg = StreamRegistry::default();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let id = reg.insert(tx).await;
        assert!(reg.contains(&id).await);
        reg.remove(&id).await;
        assert!(!reg.contains(&id).await);
    }
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::relay`
Expected: FAIL — `cannot find StreamRegistry`.

- [ ] **Step 3: 实现 relay**

```rust
//! Per-stream relay: bridges Panel audio frames → backend → delta TopicEvents.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use super::{build_transcriber, StreamConfig, StreamingTarget, TranscriptDelta};
use crate::gateway::event_bus::{EventBus, TopicEvent};

/// Active streams: stream_id → audio sender into the backend bridge task.
#[derive(Default, Clone)]
pub struct StreamRegistry {
    inner: Arc<Mutex<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
}

impl StreamRegistry {
    pub async fn insert(&self, tx: mpsc::Sender<Vec<u8>>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.inner.lock().await.insert(id.clone(), tx);
        id
    }
    pub async fn contains(&self, id: &str) -> bool { self.inner.lock().await.contains_key(id) }
    pub async fn audio_sender(&self, id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.inner.lock().await.get(id).cloned()
    }
    pub async fn remove(&self, id: &str) { self.inner.lock().await.remove(id); }
}

/// Open a backend stream and spawn the delta→TopicEvent pump. Returns stream_id.
pub async fn start_stream(
    reg: &StreamRegistry,
    bus: EventBus,
    target: StreamingTarget,
    cfg: StreamConfig,
) -> anyhow::Result<String> {
    let transcriber = build_transcriber(target);
    let mut handles = transcriber.open(cfg).await?;
    let id = reg.insert(handles.audio_tx.clone()).await;
    let pump_id = id.clone();
    tokio::spawn(async move {
        while let Some(delta) = handles.delta_rx.recv().await {
            let data = serde_json::json!({
                "stream_id": pump_id,
                "delta": delta_json(&delta),
            });
            bus.send(TopicEvent::new("voice.transcribe.delta", data));
        }
    });
    Ok(id)
}

fn delta_json(d: &TranscriptDelta) -> serde_json::Value {
    serde_json::to_value(d).unwrap_or_default()
}
```
> `bus.send(TopicEvent)` 的精确签名实现时按 `event_bus.rs` 的 broadcast sender 适配(Explore 确认 `EventBus` 持 `broadcast::Sender<TopicEvent>`)。`StreamRegistry` 作为进程级状态挂进 gateway 的 app state(沿用现有 voice state 注入路径)。

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p alephcore --lib gateway::voice::streaming::relay`
Expected: PASS。

- [ ] **Step 5: 实现 4 个 handler**(`handlers/voice.rs`,镜像现有 `handle_transcribe` 签名 `(JsonRpcRequest, Arc<RwLock<Config>>, vault, + StreamRegistry, EventBus)`)

```rust
// voice.stream.start  params: { language?: String } → { stream_id }
//   读 cfg.voice.streaming 组 StreamingTarget;若 !enabled → 返回 { stream_id: null }(Panel 回落批量)
// voice.stream.audio  params: { stream_id, pcm_base64 } → {}  (base64 解码 → audio_sender(id).send(bytes))
// voice.stream.stop   params: { stream_id } → {}  (reg.remove(id) → drop sender → 桥接 task 收尾发 CloseStream/END_OF_AUDIO)
```
完整 handler 代码按现有 `handle_transcribe` 的 `Params` 解析 + `JsonRpcResponse::success` 模式写;`pcm_base64` 用 `base64::engine::general_purpose::STANDARD.decode`。

- [ ] **Step 6: 注册方法**

在 `voice.transcribe` 的注册/dispatch 旁,加 `voice.stream.start` / `voice.stream.audio` / `voice.stream.stop` 三个方法到同一 router(`grep -rn '"voice.transcribe"' src/gateway` 定位)。

- [ ] **Step 7: 编译验证 + Commit**

Run: `cargo build -p alephcore`
Expected: 编译通过(server 二进制)。

```bash
git add src/gateway
git commit -m "feat(voice): stream relay + voice.stream.{start,audio,stop} RPC + delta topic"
```

---

# Phase 2 — Panel 两阶段渲染 + 水波坍缩

## Task 7: 字幕两阶段状态 reducer(纯函数 TDD)

**Files:**
- Create: `interfaces/webchat/src/views/voice/caption_state.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`(`mod caption_state;`)

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_updates_committed_and_interim() {
        let mut s = CaptionState::default();
        apply_delta(&mut s, Delta { committed: "你好".into(), interim: "世".into() });
        assert_eq!(s.committed, "你好");
        assert_eq!(s.interim, "世");
        assert!(!s.locked);
    }

    #[test]
    fn lock_drops_interim_and_marks_locked() {
        let mut s = CaptionState::default();
        apply_delta(&mut s, Delta { committed: "你好世界".into(), interim: "吗".into() });
        lock(&mut s);
        assert_eq!(s.committed, "你好世界");
        assert_eq!(s.interim, "");
        assert!(s.locked);
    }

    #[test]
    fn formatted_replaces_committed_after_lock() {
        let mut s = CaptionState::default();
        apply_delta(&mut s, Delta { committed: "额我想问下本地语音释放".into(), interim: String::new() });
        lock(&mut s);
        apply_formatted(&mut s, "请问如何实现本地语音模型的内存释放？");
        assert_eq!(s.committed, "请问如何实现本地语音模型的内存释放？");
        assert!(s.formatted);
    }
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p alephcore --lib`（panel 在同 workspace;或按 panel crate 的测试命令）—— 先确认编译失败 `cannot find CaptionState`。
> 注:panel 是独立 crate(`interfaces/webchat`)。实际命令:`cargo test -p aleph-panel --lib views::voice::caption_state`(crate 名以 `interfaces/webchat/Cargo.toml` 的 `[package].name` 为准,实现时确认)。

- [ ] **Step 3: 写最小实现**

```rust
//! Pure two-stage caption reducer. No web_sys → host-testable (project test redline).

#[derive(Default, Clone, PartialEq)]
pub(crate) struct CaptionState {
    pub committed: String, // locked text (solid/white)
    pub interim: String,   // floating hypothesis (gray)
    pub locked: bool,      // utterance ended → wave fired
    pub formatted: bool,   // AI-polished text swapped in
}

pub(crate) struct Delta { pub committed: String, pub interim: String }

pub(crate) fn apply_delta(s: &mut CaptionState, d: Delta) {
    s.committed = d.committed;
    s.interim = d.interim;
}

/// Utterance end: drop the floating interim, mark locked (Panel fires the wave).
pub(crate) fn lock(s: &mut CaptionState) {
    s.interim.clear();
    s.locked = true;
}

/// AI-formatted text arrives → replace committed (quiet fade swap).
pub(crate) fn apply_formatted(s: &mut CaptionState, polished: &str) {
    s.committed = polished.to_string();
    s.formatted = true;
}
```

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p aleph-panel --lib views::voice::caption_state`
Expected: PASS（3 个测试）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/voice/caption_state.rs interfaces/webchat/src/views/voice/mod.rs
git commit -m "feat(panel): pure two-stage caption reducer (committed/interim/locked/formatted)"
```

---

## Task 8: Panel 音频切块上行(s16le 16k base64 帧)

`audio.rs` 现有连续 PCM 采集(`MicSession`,ScriptProcessorNode)。新增:把同一份 PCM 重采样 16k mono、转 s16le、base64,经回调交给上层按帧上行。

**Files:**
- Modify: `interfaces/webchat/src/views/voice/audio.rs`

- [ ] **Step 1: 纯转换函数测试**(`audio.rs` tests,f32→s16le 可纯测)

```rust
#[cfg(test)]
mod frame_tests {
    use super::*;
    #[test]
    fn f32_to_s16le_clamps_and_scales() {
        let pcm = [0.0f32, 1.0, -1.0, 2.0, -2.0];
        let bytes = f32_to_s16le(&pcm);
        assert_eq!(bytes.len(), pcm.len() * 2);
        // 0.0 → 0
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        // 1.0 → 32767 (clamped), -1.0 → -32768, 2.0 → clamped 32767
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), 32767);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -32768);
        assert_eq!(i16::from_le_bytes([bytes[6], bytes[7]]), 32767);
    }
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p aleph-panel --lib views::voice::audio::frame_tests`
Expected: FAIL — `cannot find f32_to_s16le`.

- [ ] **Step 3: 实现转换 + 帧回调**

```rust
/// Convert mono f32 [-1,1] PCM to little-endian s16 bytes (clamped).
pub(crate) fn f32_to_s16le(pcm: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i32;
        out.extend_from_slice(&(v.clamp(-32768, 32767) as i16).to_le_bytes());
    }
    out
}
```
在 `MicSession` 增加:当上层处于流式监听时,把 ScriptProcessorNode 累积的样本(已有连续采集)按 ~100–200ms 切块 → 若设备采样率 ≠ 16k,做线性重采样到 16k(简单抽取/插值即可,STT 端稳健)→ `f32_to_s16le` → base64 → 调上层注入的 `on_frame: Fn(String)` 回调。新增 `MicSession::set_frame_sink(cb)` 与开关 `start_streaming()/stop_streaming()`。
> 现有 pre-roll/`take_segment_wav` 批量路径**保留不动**(批量兜底用)。

- [ ] **Step 4: 跑确认通过 + wasm 编译**

Run: `cargo test -p aleph-panel --lib views::voice::audio::frame_tests`
Then: `cargo build --target wasm32-unknown-unknown -p aleph-panel`
Expected: 测试 PASS;wasm 编译通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/voice/audio.rs
git commit -m "feat(panel): s16le 16k frame encode + streaming frame sink on MicSession"
```

---

## Task 9: Panel 流式会话接线(start/feed/stop + 订阅 delta + 水波触发)

把 Task 7/8 接进 `VoiceSession`:监听时开流、推帧、订阅 `voice.transcribe.delta`、VAD 判句时锁定(起水波)并把原文送 Agent(复用现有 `handle_utterance` 的 send 部分)。

**Files:**
- Modify: `interfaces/webchat/src/views/voice/mod.rs`

- [ ] **Step 1: 扩展 `Caption`,渲染两段**

`Caption` 加变体并渲染 committed(实色 span)+ interim(灰色 span `.voice-interim`),`locked` 时整行加 `.voice-lock`(触发水波 CSS)。`caption_text`/view 改为渲染 `CaptionState`。

- [ ] **Step 2: 开流 + 推帧**

进入 Listening(`MicSession::open` 之后):
```rust
// start streaming session
let lang = None::<String>;
let resp = dash.rpc_call("voice.stream.start", serde_json::json!({ "language": lang })).await;
let stream_id = resp.ok().and_then(|v| v.get("stream_id").and_then(|s| s.as_str()).map(str::to_string));
// stream_id == None → BYO streaming disabled → keep existing batch path (handle_utterance unchanged)
```
若 `stream_id` 有值:`mic.set_frame_sink(move |b64| spawn_local(dash.rpc_call("voice.stream.audio", json!({"stream_id": id, "pcm_base64": b64}))))` 并 `mic.start_streaming()`。

- [ ] **Step 3: 订阅 delta**

按 Panel 现有事件订阅(`chat.messages` 走的 `stream.*` event 机制,`grep` `"method":"event"` 或 `topic` 在 `interfaces/webchat/src/api`)新增对 `voice.transcribe.delta` 的处理:过滤 `stream_id` 匹配 → `apply_delta(&mut caption_state, ...)`。

- [ ] **Step 4: VAD 判句 → 锁定起水波 + 原文送 Agent + 关流**

在现有 `UtteranceEnd` 分支(`mod.rs:197`)流式模式下:
```rust
// 1) lock caption → CSS wave fires (committed gray→white, ~150ms)
lock(&mut caption_state);
// 2) raw committed text → Agent immediately (zero latency), reuse existing send
let raw = caption_state.committed.clone();
// (call the send half of handle_utterance with `raw` instead of transcribing a WAV)
// 3) close stream
spawn_local(dash.rpc_call("voice.stream.stop", json!({"stream_id": id})));
```
把 `handle_utterance` 的"转写"前半段在流式分支跳过(文本已由流给出),复用其"send + arm speak Effect"后半段(抽出 `fn send_utterance(text, ...)` 供两条路径共用,DRY)。

- [ ] **Step 5: wasm 编译验证**

Run: `cargo build --target wasm32-unknown-unknown -p aleph-panel`
Expected: 编译通过。
（行为验证在 Task 10 视觉 + Phase 末人工 E2E。）

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/voice/mod.rs
git commit -m "feat(panel): wire streaming voice session (open/feed/subscribe/lock + raw-to-agent)"
```

---

## Task 10: C·水波拂过 CSS + 灰/白字层 + a11y

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`

- [ ] **Step 1: 加样式**(沿用 voice spec §5 的 mock 关键帧,落进现有 voice token 区)

```css
.voice-interim { color: var(--color-muted, #8b8aa3); }
.voice-committed { color: var(--text-primary); }
/* C·水波拂过:locked 行触发,clip 推进 + sheen 扫掠,~150ms */
.voice-lock .voice-committed { animation: voice-wave-final .16s ease-out both; position: relative; }
.voice-lock .voice-committed::after {
  content: ""; position: absolute; inset: -40% auto -40% 0; width: 46px;
  background: linear-gradient(90deg, transparent, color-mix(in oklch, var(--color-primary) 60%, white) 50%, transparent);
  filter: blur(3px); mix-blend-mode: screen; animation: voice-wave-sheen .16s ease-out both;
}
@keyframes voice-wave-final { from { clip-path: inset(0 100% 0 0); } to { clip-path: inset(0 0 0 0); } }
@keyframes voice-wave-sheen { from { transform: translateX(-60px); opacity: .9; } to { transform: translateX(360px); opacity: 0; } }
@media (prefers-reduced-motion: reduce) {
  .voice-lock .voice-committed { animation: voice-fade-in .16s ease-out both; }
  .voice-lock .voice-committed::after { display: none; }
  @keyframes voice-fade-in { from { opacity: .3; } to { opacity: 1; } }
}
@media (prefers-reduced-transparency: reduce) {
  .voice-lock .voice-committed::after { mix-blend-mode: normal; }
}
```

- [ ] **Step 2: 视觉验收**(standalone HTML + chrome-devtools,沿用 spec §11 手段)

构建 standalone 页把 `.voice-stage` + 两段字幕 + `.voice-lock` 套上,跑 `just wasm` 或直接静态页;chrome-devtools 截图三材质×五色板下的水波,确认灰→白、~150ms、reduced-motion 退化为淡入。
Expected: 视觉与 mock(C·水波)一致,无溢出/性能掉帧。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "feat(panel): C-wave-wipe lock transition + interim/committed caption styles"
```

---

# Phase 3 — AI 规整(显示层润色)

## Task 11: core `voice.format` handler(快模型 + 精炼师 prompt)

**Files:**
- Create: `src/gateway/voice/format.rs`
- Modify: `src/gateway/handlers/voice.rs`(加 `voice.format` handler)+ 注册

- [ ] **Step 1: 写失败测试**(prompt 组装纯函数)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_prompt_used_when_config_empty() {
        let p = build_format_prompt("", "额 我那个 想问下");
        assert!(p.contains("言语精炼师") || p.contains("语音实时格式化"));
        assert!(p.ends_with("额 我那个 想问下"));
    }
    #[test]
    fn custom_prompt_overrides_default() {
        let p = build_format_prompt("自定义指令：", "原文");
        assert!(p.starts_with("自定义指令："));
        assert!(p.ends_with("原文"));
    }
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p alephcore --lib gateway::voice::format`
Expected: FAIL — `cannot find build_format_prompt`.

- [ ] **Step 3: 实现**

```rust
//! AI speech-formatting: one fast-model pass turning raw disfluent transcript
//! into clean written text. Display-level polish only — does NOT gate the Agent
//! (the raw text was already sent). R7/R9: intelligence lives in the prompt.

const DEFAULT_PROMPT: &str = "你是一个冷酷的语音实时格式化微型引擎。请将以下口语化的逐字稿转化为排版优雅、逻辑清晰、无语气词的正式书面语。\n【硬性要求】\n1. 绝对不能回答用户的提问，只能对文本进行润色和纠错。\n2. 剔除所有\"额、啊、那个、就是、然后\"等口语冗余。\n3. 补全错别字和缺失的标点。\n4. 如果文本本身已经很清晰，原样输出。\n输入：";

#[must_use]
pub fn build_format_prompt(custom: &str, raw: &str) -> String {
    let head = if custom.trim().is_empty() { DEFAULT_PROMPT } else { custom };
    format!("{head}{raw}")
}
```
`format.rs` 另写 `pub async fn format_text(raw, &FormatConfig, registry, vault) -> anyhow::Result<String>`:用 `ModelOverride::from_voice(&cfg.provider, &cfg.model)` 解析 provider → 一次性 `GenerationProvider::generate(GenerationRequest{ prompt, max_tokens: small, temperature: low })` → 返回 trim 文本;失败/空 → 返回原文(P7 不抛)。

`handlers/voice.rs` 加 `voice.format`:`params { text, prompt? }` → 读 `cfg.voice.format`(disabled → 原样返回)→ `format_text` → `{ formatted }`。注册同 Task 6。

- [ ] **Step 4: 跑确认通过 + 编译**

Run: `cargo test -p alephcore --lib gateway::voice::format`
Then: `cargo build -p alephcore`
Expected: 测试 PASS;编译通过。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/voice/format.rs src/gateway/handlers/voice.rs
git commit -m "feat(voice): voice.format RPC — fast-model speech regularization (display polish)"
```

---

## Task 12: Panel 调用 `voice.format` + 安静替换

**Files:**
- Modify: `interfaces/webchat/src/views/voice/mod.rs`

- [ ] **Step 1: 判句后并行规整**

在 Task 9 的锁定分支(送原文给 Agent **之后**,不阻塞)追加:
```rust
let raw = caption_state.committed.clone();
spawn_local(async move {
    if let Ok(v) = dash.rpc_call("voice.format", serde_json::json!({ "text": raw })).await {
        if let Some(p) = v.get("formatted").and_then(|s| s.as_str()).filter(|s| !s.is_empty()) {
            apply_formatted(&mut caption_state_signal, p); // quiet fade swap (.voice-committed.formatted)
        }
    }
});
```
（`caption_state` 经信号写入;水波已在锁定时跑过,这里只替换文本——加 `.formatted` 触发一个极短淡入,非二次大动画。）

- [ ] **Step 2: CSS 微淡入**(tailwind.css)

```css
.voice-committed.formatted { animation: voice-fade-in .12s ease-out both; }
```

- [ ] **Step 3: wasm 编译 + 人工 E2E(Phase 收尾)**

Run: `cargo build --target wasm32-unknown-unknown -p aleph-panel`
Expected: 编译通过。

人工 E2E 清单(真麦 + **两后端各一遍**,验中立):
- [ ] 自托管 WhisperLiveKit:边说边出灰字 → 判句水波白字 → ~200ms 后规整替换
- [ ] Deepgram 云:同上链路
- [ ] `[voice.streaming].enabled=false`:回落批量路径,行为同今天(无灰字)
- [ ] 规整超时/失败:水波白字保留原文,不卡
- [ ] 打断:Speaking 中开口仍能打断(现有逻辑不回归)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/voice/mod.rs interfaces/webchat/styles/tailwind.css
git commit -m "feat(panel): call voice.format on utterance-end + quiet formatted swap"
```

---

## Self-Review(写完计划的回查)

**Spec 覆盖:**
- D1/D2 供应商中立 → Task 1(trait)/2(deepgram)/3(whisperlive)/4(工厂)/5(等权预设+config) ✓
- D3 经 core 中转 → Task 6(relay + voice.stream.* + TopicEvent);Panel base64 帧走 rpc_call(Task 8/9) ✓
- D4 规整引擎(prompt+ModelOverride,不上 FunASR)→ Task 11 ✓
- D5 原文送 Agent + 规整做视觉 → Task 9 Step 4(原文先送)+ Task 12(规整后替换)✓
- D6 C·水波拂过 → Task 10 ✓
- 批量兜底(P7)→ Task 9 Step 2(`stream_id==None` 回落)/ Task 12 E2E ✓
- 降级矩阵 → Task 11(format 失败返原文)/ Task 9(stream_id None)/ E2E 清单 ✓
- **偏离 spec §5 的明示**:spec 写"回填规整文本到 session UserMessage / 记忆";本计划 MVP **只做显示层替换**(caption + Panel 乐观气泡),**不改写 append-only session log / memory**——那是后续独立项(避免动事件日志语义,符合 R10 极简)。已在此标注为有意 MVP 边界。

**占位符扫描:** 无 TBD/TODO/"类似 Task N";纯函数均给完整代码 + 测试;集成/wasm/视觉任务给出实现要点 + 精确插入点(`mod.rs:197` 等)+ 真实既有符号(`resolve_stt_source`/`ModelOverride::from_voice`/`TopicEvent::new`/`handle_transcribe`)。集成任务的"读现有 pattern X 接线"是刻意的(执行子代理读真实 API),非占位。

**类型一致性:** `TranscriptDelta{committed,interim,utterance_end}` 贯穿 Task 1/2/3/4/6;`CaptionState{committed,interim,locked,formatted}` + `apply_delta/lock/apply_formatted` 贯穿 Task 7/9/12;`StreamingTarget{provider,base_url,api_key,language}` 贯穿 Task 4/5/6;`build_format_prompt`/`format_text` Task 11→12。命名一致。

**已知执行期需就地确认(非占位,执行子代理负责):**
- panel crate 名(`aleph-panel` 假定;以 `interfaces/webchat/Cargo.toml` 为准)
- 方法注册的确切 dispatch 文件(`grep '"voice.transcribe"'`)
- `EventBus::send` 与 app-state 注入 `StreamRegistry` 的确切签名(`event_bus.rs` / gateway state)
- Panel 事件订阅 `voice.transcribe.delta` 的确切 API(`interfaces/webchat/src/api` 现有 `event` 订阅)
- `GenerationRequest`/`registry.resolve_with_fallback` 文本补全的确切构造(`src/generation/registry.rs`)
