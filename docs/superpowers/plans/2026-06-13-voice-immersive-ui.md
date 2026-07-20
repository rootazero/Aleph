# Voice Immersive UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Aleph Panel 实现 Siri 级沉浸语音模式：全屏流光球 + VAD 自动判句 + 可打断的句级流水线 TTS，零 gateway 改动。

**Architecture:** 纯 Panel（Leptos 0.8 CSR / crate `aleph-panel`）实现。三个纯函数核（VAD / 句切分 / 状态机）host 可测；音频管线（getUserMedia+AnalyserNode+MediaRecorder / TTS 队列）为 wasm 薄胶水；球为 CSS 分层渐变组件，由 `--voice-level` CSS 变量驱动；复用既有 RPC（`voice.transcribe`/`voice.synthesize`/`chat.send`）与流式消息信号。Spec: `docs/superpowers/specs/2026-06-13-voice-immersive-ui-design.md`。

**Tech Stack:** Leptos 0.8.15 CSR、Tailwind CSS v4（OKLCH token）、web-sys（需新增 AudioContext/AnalyserNode features）、既有 DashboardState RPC 通道。

**项目红线（每个任务都适用）:**
- Panel 改动必须跑 `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown` 验证（native check 过 ≠ wasm 过）
- 测试不碰 web_sys（host 跑 `cargo test -p aleph-panel --lib` 时 web_sys 调用会 panic）——纯函数模块零 web_sys import
- 严禁设 CARGO_TARGET_DIR；共享 target-dir flock 排队是预期
- worktree 内 `just wasm` 的 wasm-bindgen 相对路径会错配——产物在主仓 target，需要时手动绝对路径
- rust-analyzer "unlinked-file" 诊断在 worktree 是幻影，以 cargo 为准

**关键既有 API（来自代码勘察，行号基于 main@13a25a95e）:**
- 发送消息：`chat.push_user_message(&text)`（state.rs:441）→ `ChatApi::send(&dash, &text, sk.as_deref(), vec![], None, None, None).await` → `ChatSendResponse { run_id, session_key, streaming }`（api/chat.rs:22）→ `chat.session_key.set(..)` + `chat.start_assistant_message(&run_id)`（state.rs:466）
- 流式增量：`chat.messages: RwSignal<Vec<ChatMessage>>`（state.rs:236），id 为 `assistant-{run_id}` 的消息 `.content` 持续累积，`.is_streaming` 标记进行中
- TTS RPC：`dash.rpc_call("voice.synthesize", json!({"text": text}))` → `{ audio_base64?, audio_url?, mime_type }`（voice_playback.rs:17-43）
- STT RPC：`dash.rpc_call("voice.transcribe", json!({"audio_base64", "mime_type"}))` → `{ text }`（composer/voice.rs:87-94）
- 快捷键先例：`window_event_listener(keydown, ..)`（state/hotkey.rs:44-79，⌘K 模式）
- 覆盖层先例：`<Show when=..><div class="absolute inset-0 z-30 ..">`（chat/view.rs:160-175）；z 约定 10/30/50
- **禁用项**：沉浸模式内不要调用 `chat.mark_speak_run(..)`（voice.rs:124 的既有自动 TTS 路径），否则双重念读

---

### Task 1: CSS 基建 — voice token + 流光球样式 + 回退

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`

- [ ] **Step 1: 在 `:root {}` token 区（~146-165 行附近，`--glass-blur` 同区块）追加 voice token**

```css
/* ── Voice orb ── flow colors derive from accent (relative OKLCH), level driven by JS */
:root {
  --voice-flow-a: var(--color-primary);
  --voice-flow-b: oklch(from var(--color-primary) calc(l + 0.08) calc(c * 1.15) calc(h + 60));
  --voice-flow-c: oklch(from var(--color-primary) calc(l + 0.12) calc(c * 0.9) calc(h - 55));
  --voice-glow: oklch(from var(--color-primary) l c h / 0.45);
  --voice-level: 0; /* 0..1, written per-frame from AnalyserNode RMS */
  --voice-flow-speed: 7s;
}
```

- [ ] **Step 2: 在 @keyframes 区（~1001 行 `aleph-pop-in` 同区域）追加动效**

```css
@keyframes voice-morph {
  0%, 100% { border-radius: 46% 54% 52% 48% / 50% 46% 54% 50%; rotate: 0deg; }
  25%      { border-radius: 58% 42% 44% 56% / 42% 60% 40% 58%; rotate: 9deg; }
  50%      { border-radius: 40% 60% 58% 42% / 56% 40% 60% 44%; rotate: -5deg; }
  75%      { border-radius: 54% 46% 40% 60% / 46% 54% 46% 54%; rotate: 5deg; }
}
@keyframes voice-flow-spin { to { rotate: 360deg; } }
@keyframes voice-sheen-drift {
  0%, 100% { translate: 0 0; scale: 1; }
  50%      { translate: 9% -7%; scale: 1.12; }
}
@keyframes voice-hue-drift {
  0%, 100% { filter: hue-rotate(0deg) saturate(1.05); }
  50%      { filter: hue-rotate(28deg) saturate(1.15); }
}
```

- [ ] **Step 3: 组件类（utilities/components 自定义类区，跟 `.aleph-sidebar` 等同级）**

```css
/* Full-screen immersive stage */
.voice-stage {
  position: fixed; inset: 0; z-index: 40;
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  background:
    radial-gradient(ellipse at 50% 118%, var(--voice-glow), transparent 58%),
    var(--color-surface);
}
/* Orb: wrapper positions, inner morphs — NEVER merge (transform clobbering) */
.voice-orb {
  position: relative;
  width: var(--voice-orb-size, 11rem); height: var(--voice-orb-size, 11rem);
  overflow: hidden;
  animation: voice-morph 5s ease-in-out infinite, voice-hue-drift 26s ease-in-out infinite;
  scale: calc(1 + var(--voice-level) * 0.16);
  transition: scale 90ms ease-out;
  background: oklch(from var(--color-primary) 0.32 calc(c * 0.7) h);
  box-shadow: 0 0 calc(3rem + var(--voice-level) * 2.5rem) calc(0.9rem + var(--voice-level) * 0.8rem) var(--voice-glow);
}
.voice-orb-flow {
  position: absolute; inset: -30%; mix-blend-mode: screen; opacity: 0.9;
  background: conic-gradient(from 0deg, var(--voice-flow-a), var(--voice-flow-b), var(--voice-flow-c), var(--voice-flow-a));
  animation: voice-flow-spin var(--voice-flow-speed) linear infinite;
}
.voice-orb-sheen {
  position: absolute; inset: -30%; mix-blend-mode: screen;
  background: radial-gradient(circle at 35% 35%, oklch(1 0 0 / 0.75), transparent 45%);
  animation: voice-sheen-drift 4.4s ease-in-out infinite;
}
/* State modifiers: flow speed is the state's voice */
.voice-orb--listening  { --voice-flow-speed: 7s; }
.voice-orb--processing { --voice-flow-speed: 14s; }
.voice-orb--speaking   { --voice-flow-speed: 3.5s; }
.voice-orb--error .voice-orb-flow { background: conic-gradient(from 0deg, var(--color-danger), var(--voice-flow-a), var(--color-danger)); }
/* Mini orb for composer button */
.voice-orb--mini { --voice-orb-size: 1.75rem; box-shadow: 0 0 0.8rem 0.15rem var(--voice-glow); }
/* Caption + hint */
.voice-caption { color: var(--color-text-primary); font-size: 1rem; text-align: center; max-width: 36rem; min-height: 1.6em; padding: 0 1.5rem; }
.voice-hint { color: var(--color-text-tertiary); font-size: 0.75rem; text-align: center; }
```

- [ ] **Step 4: 无障碍回退（追加到既有 `prefers-reduced-motion`（~1020）与 `prefers-reduced-transparency`（~939）块内）**

```css
@media (prefers-reduced-motion: reduce) {
  .voice-orb, .voice-orb-flow, .voice-orb-sheen { animation: none; }
  .voice-orb { scale: 1; transition: none; }
  .voice-orb-flow { opacity: calc(0.55 + var(--voice-level) * 0.4); transition: opacity 120ms linear; }
}
@media (prefers-reduced-transparency: reduce) {
  .voice-orb-flow, .voice-orb-sheen { mix-blend-mode: normal; }
  .voice-stage { background: var(--color-surface); }
}
```

- [ ] **Step 5: 重建 CSS 并目检**

Run: `cd interfaces/webchat && npm run build:css`
Expected: dist/tailwind.css 重新生成无报错。

- [ ] **Step 6: standalone 视觉自检（不部署 daemon 的省时模式）**

写 `/tmp/voice_orb_check.html`：`<link>` 指向编译后 `dist/tailwind.css` 绝对路径，body 内放
`<div class="voice-stage"><div class="voice-orb voice-orb--listening" style="--voice-level:0.4"><div class="voice-orb-flow"></div><div class="voice-orb-sheen"></div></div></div>`，
浏览器打开确认：球居中、流光旋转、改 `data-accent="ocean"` 于 html 标签后流光变蓝青。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: voice orb css — accent-derived flow tokens, morph keyframes, a11y fallbacks"
```

---

### Task 2: VAD 纯函数状态机（TDD）

**Files:**
- Create: `interfaces/webchat/src/views/voice/vad.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（本任务先创建为仅 `pub(crate) mod vad;` 的占位）
- Modify: `interfaces/webchat/src/views/mod.rs`（追加 `pub(crate) mod voice;`）

约束：**本文件零 web_sys/js_sys import**（host 测试红线）。

- [ ] **Step 1: 写失败测试（文件底部 `#[cfg(test)]`）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CFG: VadConfig = VadConfig {
        start_rms: 0.06,
        end_silence_ms: 700,
        min_speech_ms: 300,
        frame_ms: 50,
    };

    /// Feed n frames of constant rms, return (final state, all events).
    fn feed(mut st: VadState, rms: f32, n: u32) -> (VadState, Vec<VadEvent>) {
        let mut evs = Vec::new();
        for _ in 0..n {
            let (next, ev) = vad_step(st, rms, &CFG);
            st = next;
            if let Some(e) = ev { evs.push(e); }
        }
        (st, evs)
    }

    #[test]
    fn silence_never_emits() {
        let (st, evs) = feed(VadState::default(), 0.01, 100);
        assert_eq!(st, VadState::Quiet);
        assert!(evs.is_empty());
    }

    #[test]
    fn speech_start_fires_once_on_threshold() {
        let (_, evs) = feed(VadState::default(), 0.2, 10);
        assert_eq!(evs, vec![VadEvent::SpeechStart]);
    }

    #[test]
    fn short_blip_below_min_speech_is_discarded() {
        // 100ms speech (2 frames) then long silence: no UtteranceEnd
        let (st, evs) = feed(VadState::default(), 0.2, 2);
        assert_eq!(evs, vec![VadEvent::SpeechStart]);
        let (st, evs) = feed(st, 0.01, 30);
        assert_eq!(st, VadState::Quiet);
        assert_eq!(evs, vec![VadEvent::Discarded]);
    }

    #[test]
    fn normal_utterance_ends_after_silence_hangover() {
        // 1s speech then silence: UtteranceEnd after 700ms (14 frames) of quiet
        let (st, _) = feed(VadState::default(), 0.2, 20);
        let (st, evs) = feed(st, 0.01, 13); // 650ms quiet: not yet
        assert!(evs.is_empty());
        assert!(matches!(st, VadState::Speech { .. }));
        let (st, evs) = feed(st, 0.01, 1); // 700ms reached
        assert_eq!(st, VadState::Quiet);
        assert_eq!(evs, vec![VadEvent::UtteranceEnd { speech_ms: 1000 }]);
    }

    #[test]
    fn mid_utterance_pause_shorter_than_hangover_resumes() {
        let (st, _) = feed(VadState::default(), 0.2, 10);   // 500ms speech
        let (st, evs) = feed(st, 0.01, 10);                 // 500ms pause < 700ms
        assert!(evs.is_empty());
        let (st, evs) = feed(st, 0.2, 10);                  // resume: speech_ms accumulates
        assert!(evs.is_empty());
        assert!(matches!(st, VadState::Speech { silence_ms: 0, .. }));
        let _ = st;
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib voice::vad`
Expected: 编译失败（类型未定义）。

- [ ] **Step 3: 实现**

```rust
//! Energy-threshold VAD with hangover. Pure — host-testable, zero web_sys.
//! Frames arrive at a fixed cadence (cfg.frame_ms); caller computes RMS.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VadConfig {
    /// RMS (0..1) above which a frame counts as speech.
    pub start_rms: f32,
    /// Continuous quiet that ends an utterance.
    pub end_silence_ms: u32,
    /// Utterances shorter than this are noise blips — discarded.
    pub min_speech_ms: u32,
    /// Caller's frame cadence.
    pub frame_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self { start_rms: 0.06, end_silence_ms: 700, min_speech_ms: 300, frame_ms: 50 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum VadState {
    #[default]
    Quiet,
    Speech { speech_ms: u32, silence_ms: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VadEvent {
    /// Voice crossed the threshold — caller starts MediaRecorder segment.
    SpeechStart,
    /// Hangover elapsed after real speech — caller stops segment and transcribes.
    UtteranceEnd { speech_ms: u32 },
    /// Hangover elapsed but speech was too short — caller drops the segment.
    Discarded,
}

pub(crate) fn vad_step(state: VadState, rms: f32, cfg: &VadConfig) -> (VadState, Option<VadEvent>) {
    let loud = rms >= cfg.start_rms;
    match state {
        VadState::Quiet if loud => (
            VadState::Speech { speech_ms: cfg.frame_ms, silence_ms: 0 },
            Some(VadEvent::SpeechStart),
        ),
        VadState::Quiet => (VadState::Quiet, None),
        VadState::Speech { speech_ms, .. } if loud => (
            VadState::Speech { speech_ms: speech_ms + cfg.frame_ms, silence_ms: 0 },
            None,
        ),
        VadState::Speech { speech_ms, silence_ms } => {
            let silence_ms = silence_ms + cfg.frame_ms;
            if silence_ms >= cfg.end_silence_ms {
                let ev = if speech_ms >= cfg.min_speech_ms {
                    VadEvent::UtteranceEnd { speech_ms }
                } else {
                    VadEvent::Discarded
                };
                (VadState::Quiet, Some(ev))
            } else {
                (VadState::Speech { speech_ms, silence_ms }, None)
            }
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib voice::vad`
Expected: 5 passed。

- [ ] **Step 5: wasm 编译验证 + Commit**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
git add interfaces/webchat/src/views/voice/ interfaces/webchat/src/views/mod.rs
git commit -m "panel: voice vad — pure energy-threshold state machine with hangover"
```

---

### Task 3: 增量句切分器（TDD）

**Files:**
- Create: `interfaces/webchat/src/views/voice/sentence.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（追加 `pub(crate) mod sentence;`）

约束：零 web_sys。输入是**累积全文**（chat.messages 的 content 每次给全量），切分器内部记 offset 增量消费。UTF-8 安全（P7：`char_indices`，绝不 `&s[..n]` 裸切字节）。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_sentence_on_terminal_punct() {
        let mut sp = SentenceSplitter::default();
        assert!(sp.push("今天有 3 个安排").is_empty());
        assert_eq!(sp.push("今天有 3 个安排。第一个"), vec!["今天有 3 个安排。"]);
    }

    #[test]
    fn handles_mixed_cjk_ascii_terminals() {
        let mut sp = SentenceSplitter::default();
        let out = sp.push("Hello there! 你好吗？还行");
        assert_eq!(out, vec!["Hello there!", "你好吗？"]);
    }

    #[test]
    fn short_fragment_merges_into_next() {
        let mut sp = SentenceSplitter::default();
        // "好。" alone is below MIN_CHARS — held and merged with the next sentence
        assert!(sp.push("好。").is_empty());
        assert_eq!(sp.push("好。我马上安排今天的事项。"), vec!["好。我马上安排今天的事项。"]);
    }

    #[test]
    fn code_fence_content_is_skipped() {
        let mut sp = SentenceSplitter::default();
        let text = "看这段代码。\n```rust\nfn main() { println!(\"x.y!\"); }\n```\n运行就好。";
        let out = sp.push(text);
        assert_eq!(out, vec!["看这段代码。", "运行就好。"]);
    }

    #[test]
    fn finish_flushes_tail_without_terminal() {
        let mut sp = SentenceSplitter::default();
        assert!(sp.push("最后一句没有标点").is_empty());
        assert_eq!(sp.finish(), Some("最后一句没有标点".to_string()));
    }

    #[test]
    fn incremental_pushes_never_duplicate() {
        let mut sp = SentenceSplitter::default();
        let mut all = Vec::new();
        for cut in ["你好。", "你好。今天", "你好。今天天气很好。", "你好。今天天气很好。出门记得带伞。"] {
            all.extend(sp.push(cut));
        }
        assert_eq!(all, vec!["你好。", "今天天气很好。", "出门记得带伞。"]);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib voice::sentence`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
//! Incremental sentence splitter for the TTS pipeline. Pure — host-testable.
//! `push` receives the FULL accumulated text each time (the chat message
//! content signal grows monotonically); internal byte offset tracks what
//! has already been consumed. Code-fence bodies are skipped for speech.

const TERMINALS: &[char] = &['。', '！', '？', '!', '?', '.', '\n', '；', ';'];
/// Sentences shorter than this (in chars) are held and merged forward.
const MIN_CHARS: usize = 4;

#[derive(Default)]
pub(crate) struct SentenceSplitter {
    /// Byte offset into the accumulated text already consumed.
    consumed: usize,
    /// Short fragment held back, waiting to merge with the next sentence.
    pending: String,
    in_code_fence: bool,
}

impl SentenceSplitter {
    /// Feed the full accumulated text; returns newly completed sentences.
    pub(crate) fn push(&mut self, full_text: &str) -> Vec<String> {
        let Some(new) = full_text.get(self.consumed..) else { return Vec::new() };
        let mut out = Vec::new();
        let mut seg_start = 0usize; // byte offset within `new`

        let mut iter = new.char_indices().peekable();
        while let Some((i, ch)) = iter.next() {
            // Track ``` fences on their own: toggle and cut the segment around them.
            if ch == '`' && new[i..].starts_with("```") {
                // flush text before the fence marker
                self.take_segment(&new[seg_start..i], &mut out);
                self.in_code_fence = !self.in_code_fence;
                // skip the marker itself
                let after = i + 3;
                // fast-forward iterator past the marker
                while iter.peek().is_some_and(|(j, _)| *j < after) { iter.next(); }
                seg_start = after;
                continue;
            }
            if self.in_code_fence {
                seg_start = i + ch.len_utf8();
                continue;
            }
            if TERMINALS.contains(&ch) {
                let end = i + ch.len_utf8();
                // Don't split "3.5" style decimals: '.' flanked by ascii digits.
                if ch == '.' {
                    let prev_digit = new[..i].chars().next_back().is_some_and(|c| c.is_ascii_digit());
                    let next_digit = new[end..].chars().next().is_some_and(|c| c.is_ascii_digit());
                    if prev_digit && next_digit {
                        continue;
                    }
                }
                self.take_segment(&new[seg_start..end], &mut out);
                seg_start = end;
            }
        }
        // Everything before seg_start is consumed; the tail stays for next push.
        self.consumed += seg_start;
        out
    }

    /// Stream ended: flush whatever is held (pending fragment + nothing else).
    pub(crate) fn finish(&mut self) -> Option<String> {
        let tail = std::mem::take(&mut self.pending);
        let tail = tail.trim().to_string();
        (!tail.is_empty()).then_some(tail)
    }

    /// Called with finish after the final full text to flush the unconsumed tail.
    pub(crate) fn finish_with(&mut self, full_text: &str) -> Option<String> {
        if let Some(rest) = full_text.get(self.consumed..) {
            if !self.in_code_fence {
                self.pending.push_str(rest);
            }
            self.consumed = full_text.len();
        }
        self.finish()
    }

    fn take_segment(&mut self, seg: &str, out: &mut Vec<String>) {
        let candidate = format!("{}{}", self.pending, seg);
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            self.pending.clear();
            return;
        }
        if trimmed.chars().count() < MIN_CHARS {
            self.pending = candidate;
        } else {
            out.push(trimmed.to_string());
            self.pending.clear();
        }
    }
}
```

注意：测试 `finish_flushes_tail_without_terminal` 调用的是 `finish()`，但无终止符的尾巴在 `push` 后仍未 consumed——实现里 `finish_with(full_text)` 才是完整冲洗入口。**测试改用 `finish_with`**：

```rust
    #[test]
    fn finish_flushes_tail_without_terminal() {
        let mut sp = SentenceSplitter::default();
        let text = "最后一句没有标点";
        assert!(sp.push(text).is_empty());
        assert_eq!(sp.finish_with(text), Some("最后一句没有标点".to_string()));
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib voice::sentence`
Expected: 6 passed。

- [ ] **Step 5: wasm 验证 + Commit**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
git add interfaces/webchat/src/views/voice/
git commit -m "panel: voice sentence splitter — incremental, code-fence aware, utf-8 safe"
```

---

### Task 4: 会话状态机纯核（TDD）

**Files:**
- Create: `interfaces/webchat/src/views/voice/machine.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（追加 `pub(crate) mod machine;`）

约束：零 web_sys。状态机只裁决"哪个转换合法 + 转换后副作用指令"，不执行副作用（组件层执行）。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_happy_loop() {
        let p = VoicePhase::Listening;
        let (p, a) = on_event(p, VoiceEvent::UtteranceSent);
        assert_eq!(p, VoicePhase::Processing);
        assert_eq!(a, Action::None);
        let (p, a) = on_event(p, VoiceEvent::FirstAudioReady);
        assert_eq!(p, VoicePhase::Speaking);
        assert_eq!(a, Action::None);
        let (p, a) = on_event(p, VoiceEvent::PlaybackDrained);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::None);
    }

    #[test]
    fn barge_in_only_valid_while_speaking() {
        let (p, a) = on_event(VoicePhase::Speaking, VoiceEvent::BargeIn);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::StopPlayback);
        // BargeIn while listening is a no-op (it's just normal speech)
        let (p, a) = on_event(VoicePhase::Listening, VoiceEvent::BargeIn);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::None);
    }

    #[test]
    fn errors_return_to_listening_with_caption() {
        let (p, a) = on_event(VoicePhase::Processing, VoiceEvent::TranscribeFailed);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::ShowError);
        let (p, a) = on_event(VoicePhase::Speaking, VoiceEvent::RunFailed);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::ShowError);
    }

    #[test]
    fn stale_events_are_rejected() {
        // playback events can't fire while still processing-before-audio… except
        // run can complete with NO tts-able text: PlaybackDrained from Processing is legal
        let (p, _) = on_event(VoicePhase::Processing, VoiceEvent::PlaybackDrained);
        assert_eq!(p, VoicePhase::Listening);
        // but FirstAudioReady when already back in Listening must not flip state
        let (p, a) = on_event(VoicePhase::Listening, VoiceEvent::FirstAudioReady);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::StopPlayback); // stale audio gets cleaned up
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib voice::machine`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
//! Voice session phase machine. Pure — decides transitions and side-effect
//! commands; the component layer executes them.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoicePhase {
    Listening,
    Processing,
    Speaking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceEvent {
    /// Utterance captured + sent for transcription/chat.
    UtteranceSent,
    /// First TTS sentence is playing.
    FirstAudioReady,
    /// TTS queue fully drained after run completion.
    PlaybackDrained,
    /// User spoke while assistant audio was playing.
    BargeIn,
    TranscribeFailed,
    RunFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    None,
    /// Stop audio and clear the TTS queue.
    StopPlayback,
    /// Show the "没听清/出错" caption.
    ShowError,
}

pub(crate) fn on_event(phase: VoicePhase, ev: VoiceEvent) -> (VoicePhase, Action) {
    use {Action as A, VoiceEvent as E, VoicePhase as P};
    match (phase, ev) {
        (P::Listening, E::UtteranceSent) => (P::Processing, A::None),
        (P::Processing, E::FirstAudioReady) => (P::Speaking, A::None),
        // A run may finish with nothing speakable — drain straight back.
        (P::Processing | P::Speaking, E::PlaybackDrained) => (P::Listening, A::None),
        (P::Speaking, E::BargeIn) => (P::Listening, A::StopPlayback),
        (P::Processing | P::Speaking, E::TranscribeFailed | E::RunFailed) => (P::Listening, A::ShowError),
        (P::Listening, E::TranscribeFailed) => (P::Listening, A::ShowError),
        // Stale audio arriving after we've already returned to Listening.
        (P::Listening, E::FirstAudioReady) => (P::Listening, A::StopPlayback),
        // Everything else: ignore.
        (p, _) => (p, A::None),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib voice::machine`
Expected: 4 passed。

- [ ] **Step 5: wasm 验证 + Commit**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
git add interfaces/webchat/src/views/voice/
git commit -m "panel: voice phase machine — pure transition table with effect commands"
```

---

### Task 5: 音频管线 wasm 胶水（MicSession + TtsPlayer）

**Files:**
- Create: `interfaces/webchat/src/views/voice/audio.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（追加 `pub(crate) mod audio;`）
- Modify: `interfaces/webchat/Cargo.toml`（web-sys features 追加）

这是 wasm 薄胶水，无 host 测试（红线），验证 = wasm 编译 + clippy + 后续人工 E2E。

- [ ] **Step 1: Cargo.toml web-sys features 追加（既有 features 数组内，~27-49 行）**

```toml
    # immersive voice — level metering + utterance capture
    "AudioContext", "AnalyserNode", "MediaStreamAudioSourceNode",
```

（`MediaDevices/MediaStream/MediaRecorder/BlobEvent/HtmlAudioElement` 等已存在，勿重复。）

- [ ] **Step 2: 实现 audio.rs**

```rust
//! Wasm audio glue for the immersive voice mode. No business logic here —
//! VAD/splitting/phase decisions live in the pure modules.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Microphone session: one getUserMedia stream shared by the level meter
/// (AnalyserNode, polled on an interval) and utterance capture (MediaRecorder
/// started/stopped per VAD verdict).
pub(crate) struct MicSession {
    stream: web_sys::MediaStream,
    _ctx: web_sys::AudioContext,
    analyser: web_sys::AnalyserNode,
    recorder: RefCell<Option<web_sys::MediaRecorder>>,
    chunks: Rc<RefCell<Vec<web_sys::Blob>>>,
    _on_data: RefCell<Option<Closure<dyn FnMut(web_sys::BlobEvent)>>>,
    buf: RefCell<Vec<u8>>,
}

impl MicSession {
    /// Open the mic with system AEC on (spec decision: 系统 AEC).
    pub(crate) async fn open() -> Result<Rc<Self>, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let devices = window.navigator().media_devices()?;
        let constraints = web_sys::MediaStreamConstraints::new();
        let audio = js_sys::Object::new();
        js_sys::Reflect::set(&audio, &"echoCancellation".into(), &true.into())?;
        js_sys::Reflect::set(&audio, &"noiseSuppression".into(), &true.into())?;
        constraints.set_audio(&audio.into());
        let stream: web_sys::MediaStream =
            JsFuture::from(devices.get_user_media_with_constraints(&constraints)?)
                .await?
                .dyn_into()?;

        let ctx = web_sys::AudioContext::new()?;
        let source = ctx.create_media_stream_source(&stream)?;
        let analyser = ctx.create_analyser()?;
        analyser.set_fft_size(1024);
        source.connect_with_audio_node(&analyser)?;

        Ok(Rc::new(Self {
            stream,
            _ctx: ctx,
            analyser,
            recorder: RefCell::new(None),
            chunks: Rc::new(RefCell::new(Vec::new())),
            _on_data: RefCell::new(None),
            buf: RefCell::new(vec![0u8; 1024]),
        }))
    }

    /// Current input level as RMS in 0..1 (time-domain bytes centered at 128).
    pub(crate) fn rms(&self) -> f32 {
        let mut buf = self.buf.borrow_mut();
        self.analyser.get_byte_time_domain_data(&mut buf);
        let sum: f32 = buf
            .iter()
            .map(|&b| {
                let v = (f32::from(b) - 128.0) / 128.0;
                v * v
            })
            .sum();
        (sum / buf.len() as f32).sqrt()
    }

    /// Begin capturing an utterance segment.
    pub(crate) fn start_segment(&self) -> Result<(), JsValue> {
        self.chunks.borrow_mut().clear();
        let recorder = web_sys::MediaRecorder::new_with_media_stream(&self.stream)?;
        let chunks = Rc::clone(&self.chunks);
        let on_data = Closure::<dyn FnMut(_)>::new(move |ev: web_sys::BlobEvent| {
            if let Some(blob) = ev.data() {
                chunks.borrow_mut().push(blob);
            }
        });
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
        recorder.start()?;
        *self._on_data.borrow_mut() = Some(on_data);
        *self.recorder.borrow_mut() = Some(recorder);
        Ok(())
    }

    /// Stop the segment and return (base64, mime). Mirrors composer/voice.rs's
    /// browser path: blob -> FileReader data URL -> strip prefix.
    pub(crate) async fn stop_segment(&self) -> Result<(String, String), JsValue> {
        let recorder = self
            .recorder
            .borrow_mut()
            .take()
            .ok_or_else(|| JsValue::from_str("no active segment"))?;
        let mime = recorder.mime_type();
        // onstop fires after the final dataavailable — await it via a Promise.
        let (tx, rx) = futures::channel::oneshot::channel::<()>();
        let tx = RefCell::new(Some(tx));
        let on_stop = Closure::once(move || {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        });
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
        recorder.stop()?;
        let _ = rx.await;
        drop(on_stop);

        let parts = js_sys::Array::new();
        for blob in self.chunks.borrow().iter() {
            parts.push(blob);
        }
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type(&mime);
        let merged = web_sys::Blob::new_with_blob_sequence_and_options(&parts, &bag)?;
        let data_url = read_blob_as_data_url(&merged).await?;
        let base64 = data_url
            .split_once(";base64,")
            .map(|(_, b)| b.to_string())
            .ok_or_else(|| JsValue::from_str("unexpected data url"))?;
        let mime = if mime.is_empty() { "audio/webm".to_string() } else { mime };
        Ok((base64, mime))
    }

    pub(crate) fn close(&self) {
        if let Some(rec) = self.recorder.borrow_mut().take() {
            let _ = rec.stop();
        }
        for track in self.stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        let _ = self._ctx.close();
    }
}

async fn read_blob_as_data_url(blob: &web_sys::Blob) -> Result<String, JsValue> {
    let reader = web_sys::FileReader::new()?;
    let (tx, rx) = futures::channel::oneshot::channel::<Result<String, JsValue>>();
    let tx = RefCell::new(Some(tx));
    let reader_c = reader.clone();
    let onload = Closure::once(move || {
        let res = reader_c
            .result()
            .and_then(|v| v.as_string().ok_or_else(|| JsValue::from_str("not a string")));
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(res);
        }
    });
    reader.set_onloadend(Some(onload.as_ref().unchecked_ref()));
    reader.read_as_data_url(blob)?;
    let out = rx.await.map_err(|_| JsValue::from_str("reader dropped"))??;
    drop(onload);
    Ok(out)
}

/// Sequential TTS sentence player with interrupt support.
pub(crate) struct TtsPlayer {
    queue: Rc<RefCell<VecDeque<String>>>,
    current: Rc<RefCell<Option<web_sys::HtmlAudioElement>>>,
    /// True once the run is complete AND the splitter flushed — drain means done.
    finalized: Rc<RefCell<bool>>,
    playing: Rc<RefCell<bool>>,
    /// Component callbacks.
    on_first_audio: Rc<dyn Fn()>,
    on_drained: Rc<dyn Fn()>,
    on_sentence: Rc<dyn Fn(String)>,
    started_any: Rc<RefCell<bool>>,
}

impl TtsPlayer {
    pub(crate) fn new(
        on_first_audio: impl Fn() + 'static,
        on_drained: impl Fn() + 'static,
        on_sentence: impl Fn(String) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            queue: Rc::new(RefCell::new(VecDeque::new())),
            current: Rc::new(RefCell::new(None)),
            finalized: Rc::new(RefCell::new(false)),
            playing: Rc::new(RefCell::new(false)),
            on_first_audio: Rc::new(on_first_audio),
            on_drained: Rc::new(on_drained),
            on_sentence: Rc::new(on_sentence),
            started_any: Rc::new(RefCell::new(false)),
        })
    }

    pub(crate) fn reset(&self) {
        self.stop_all();
        *self.finalized.borrow_mut() = false;
        *self.started_any.borrow_mut() = false;
    }

    pub(crate) fn enqueue(self: &Rc<Self>, dash: crate::state::DashboardState, sentence: String) {
        self.queue.borrow_mut().push_back(sentence);
        self.pump(dash);
    }

    /// Mark that no more sentences will arrive; drain fires when queue empties.
    pub(crate) fn finalize(self: &Rc<Self>, dash: crate::state::DashboardState) {
        *self.finalized.borrow_mut() = true;
        self.pump(dash);
    }

    pub(crate) fn stop_all(&self) {
        self.queue.borrow_mut().clear();
        if let Some(audio) = self.current.borrow_mut().take() {
            let _ = audio.pause();
            audio.set_onended(None);
            audio.set_src("");
        }
        *self.playing.borrow_mut() = false;
    }

    fn pump(self: &Rc<Self>, dash: crate::state::DashboardState) {
        if *self.playing.borrow() {
            return;
        }
        let Some(sentence) = self.queue.borrow_mut().pop_front() else {
            if *self.finalized.borrow() {
                (self.on_drained)();
            }
            return;
        };
        *self.playing.borrow_mut() = true;
        let this = Rc::clone(self);
        spawn_local(async move {
            let src = synthesize_to_src(&dash, &sentence).await;
            match src {
                Some(src) => {
                    if !*this.started_any.borrow() {
                        *this.started_any.borrow_mut() = true;
                        (this.on_first_audio)();
                    }
                    (this.on_sentence)(sentence);
                    this.play_then_pump(dash, &src);
                }
                None => {
                    // TTS failed: caption-only for this sentence, keep going (P7).
                    (this.on_sentence)(sentence);
                    *this.playing.borrow_mut() = false;
                    this.pump(dash);
                }
            }
        });
    }

    fn play_then_pump(self: &Rc<Self>, dash: crate::state::DashboardState, src: &str) {
        let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(src) else {
            *self.playing.borrow_mut() = false;
            self.pump(dash);
            return;
        };
        let this = Rc::clone(self);
        let on_ended = Closure::once_into_js(move || {
            *this.playing.borrow_mut() = false;
            *this.current.borrow_mut() = None;
            this.pump(dash);
        });
        audio.set_onended(Some(on_ended.unchecked_ref()));
        let _ = audio.play();
        *self.current.borrow_mut() = Some(audio);
    }
}

/// voice.synthesize -> playable src (data url or remote url). None on failure.
async fn synthesize_to_src(dash: &crate::state::DashboardState, text: &str) -> Option<String> {
    let resp = dash
        .rpc_call("voice.synthesize", serde_json::json!({ "text": text }))
        .await
        .ok()?;
    let mime = resp.get("mime_type").and_then(|v| v.as_str()).unwrap_or("audio/mpeg");
    if let Some(b64) = resp.get("audio_base64").and_then(|v| v.as_str()) {
        return Some(format!("data:{mime};base64,{b64}"));
    }
    resp.get("audio_url").and_then(|v| v.as_str()).map(str::to_string)
}
```

> 实施者注意：①`futures::channel::oneshot` 需确认 `futures`（或 `futures-channel`）已在 aleph-panel 依赖；若无，加 `futures = "0.3"`（wasm 兼容）或换用 `wasm_bindgen_futures` + `js_sys::Promise` 手工模式（仿 voice.rs 既有写法）。②`dash.rpc_call` 的真实签名/返回类型以 `src/state/`（DashboardState）现状为准——voice_playback.rs:17-43 是同一调用的既有先例，对齐它。③`MediaRecorder::mime_type()` 与 `BlobPropertyBag` API 名以 web-sys 0.3 文档为准。

- [ ] **Step 3: wasm 编译 + clippy**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings
```

Expected: 0 error 0 warning。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/voice/audio.rs interfaces/webchat/src/views/voice/mod.rs interfaces/webchat/Cargo.toml Cargo.lock
git commit -m "panel: voice audio glue — mic session with analyser metering, sequential tts player"
```

---

### Task 6: VoiceOrb 组件

**Files:**
- Create: `interfaces/webchat/src/views/voice/orb.rs`
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（追加 `pub(crate) mod orb;`）

- [ ] **Step 1: 实现**

```rust
//! The orb. Rendering kernel = layered divs + CSS (Task 1 classes); swap to
//! Canvas/shader later without touching callers — props are the contract.

use leptos::prelude::*;

use super::machine::VoicePhase;

#[component]
pub(crate) fn VoiceOrb(
    /// Current session phase — selects flow speed / error tint.
    #[prop(into)] phase: Signal<VoicePhase>,
    /// Mic/playback level 0..1 — drives scale and glow via --voice-level.
    #[prop(into)] level: Signal<f64>,
    /// True briefly after an error to flash the danger tint.
    #[prop(into, default = Signal::stored(false))] error_flash: Signal<bool>,
) -> impl IntoView {
    let class = move || {
        let state = if error_flash.get() {
            "voice-orb--error"
        } else {
            match phase.get() {
                VoicePhase::Listening => "voice-orb--listening",
                VoicePhase::Processing => "voice-orb--processing",
                VoicePhase::Speaking => "voice-orb--speaking",
            }
        };
        format!("voice-orb {state}")
    };
    let style = move || format!("--voice-level: {:.3}", level.get().clamp(0.0, 1.0));
    view! {
        <div class=class style=style>
            <div class="voice-orb-flow"></div>
            <div class="voice-orb-sheen"></div>
        </div>
    }
}
```

- [ ] **Step 2: wasm 编译验证**

Run: `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release`
Expected: 通过（组件暂无调用者，允许 `#[allow(dead_code)]` 临时压制——Task 7 接线后移除）。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/voice/
git commit -m "panel: voice orb component — css kernel behind phase/level props"
```

---

### Task 7: ImmersiveVoiceView — 状态机宿主与全链接线

**Files:**
- Modify: `interfaces/webchat/src/views/voice/mod.rs`（主组件 + `VoiceMode` context）
- Modify: `interfaces/webchat/src/app.rs`（provide context + 挂载覆盖层）

本任务是集成核心。组件持有：MicSession、VAD tick 循环、TtsPlayer、SentenceSplitter、phase 信号，并消费 Task 2-6 的全部产出。

- [ ] **Step 1: VoiceMode context + 主组件骨架（mod.rs）**

```rust
//! Immersive voice mode — full-screen overlay hosting the voice session loop.

pub(crate) mod audio;
pub(crate) mod machine;
pub(crate) mod orb;
pub(crate) mod sentence;
pub(crate) mod vad;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::state::{ChatState, DashboardState};
use audio::{MicSession, TtsPlayer};
use machine::{on_event, Action, VoiceEvent, VoicePhase};
use orb::VoiceOrb;
use sentence::SentenceSplitter;
use vad::{vad_step, VadConfig, VadEvent, VadState};

/// App-level switch for the immersive overlay. Provided in app.rs.
#[derive(Clone, Copy)]
pub(crate) struct VoiceMode {
    pub open: RwSignal<bool>,
}

impl VoiceMode {
    pub(crate) fn new() -> Self {
        Self { open: RwSignal::new(false) }
    }
}

/// Who the caption is quoting.
#[derive(Clone, PartialEq)]
enum Caption {
    Idle,
    User(String),
    Assistant(String),
    Error(String),
}

#[component]
pub(crate) fn ImmersiveVoiceView() -> impl IntoView {
    let voice_mode = expect_context::<VoiceMode>();
    view! {
        <Show when=move || voice_mode.open.get()>
            <VoiceSession />
        </Show>
    }
}
```

- [ ] **Step 2: VoiceSession 组件（mod.rs 续）——会话循环**

```rust
#[component]
fn VoiceSession() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let voice_mode = expect_context::<VoiceMode>();

    let phase = RwSignal::new(VoicePhase::Listening);
    let level = RwSignal::new(0.0_f64);
    let caption = RwSignal::new(Caption::Idle);
    let error_flash = RwSignal::new(false);
    let mic_denied = RwSignal::new(false);

    // Session-scoped non-reactive plumbing.
    let mic: Rc<RefCell<Option<Rc<MicSession>>>> = Rc::new(RefCell::new(None));
    let vad = Rc::new(RefCell::new(VadState::default()));
    let vad_cfg = VadConfig::default();
    let splitter = Rc::new(RefCell::new(SentenceSplitter::default()));
    let speak_run: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let consecutive_errors = Rc::new(RefCell::new(0u32));

    // Phase event dispatcher: pure machine decides, we execute the Action.
    let player_slot: Rc<RefCell<Option<Rc<TtsPlayer>>>> = Rc::new(RefCell::new(None));
    let dispatch = {
        let player_slot = Rc::clone(&player_slot);
        move |ev: VoiceEvent| {
            let (next, action) = on_event(phase.get_untracked(), ev);
            phase.set(next);
            match action {
                Action::None => {}
                Action::StopPlayback => {
                    if let Some(p) = player_slot.borrow().as_ref() {
                        p.stop_all();
                    }
                }
                Action::ShowError => {
                    caption.set(Caption::Error("没听清，再说一次？".into()));
                    error_flash.set(true);
                    set_timeout(move || error_flash.set(false), std::time::Duration::from_millis(900));
                }
            }
        }
    };

    // TTS player wired to machine events + assistant caption.
    let player = TtsPlayer::new(
        { let d = dispatch.clone(); move || d(VoiceEvent::FirstAudioReady) },
        { let d = dispatch.clone(); move || d(VoiceEvent::PlaybackDrained) },
        move |sentence| caption.set(Caption::Assistant(sentence)),
    );
    *player_slot.borrow_mut() = Some(Rc::clone(&player));

    // ── Mic open + 50ms VAD tick ──
    let tick_handle: Rc<RefCell<Option<IntervalHandle>>> = Rc::new(RefCell::new(None));
    {
        let mic = Rc::clone(&mic);
        let vad = Rc::clone(&vad);
        let tick_handle = Rc::clone(&tick_handle);
        let dispatch = dispatch.clone();
        let player = Rc::clone(&player);
        let splitter = Rc::clone(&splitter);
        let speak_run = Rc::clone(&speak_run);
        let consecutive_errors = Rc::clone(&consecutive_errors);
        spawn_local(async move {
            let session = match MicSession::open().await {
                Ok(s) => s,
                Err(_) => {
                    mic_denied.set(true);
                    return;
                }
            };
            *mic.borrow_mut() = Some(Rc::clone(&session));
            let handle = set_interval_with_handle(
                move || {
                    let rms = session.rms();
                    level.set(f64::from(rms.min(1.0)));
                    let (next, ev) = vad_step(*vad.borrow(), rms, &vad_cfg);
                    *vad.borrow_mut() = next;
                    match ev {
                        Some(VadEvent::SpeechStart) => {
                            // Barge-in if assistant is speaking; always start capture.
                            if phase.get_untracked() == VoicePhase::Speaking {
                                dispatch(VoiceEvent::BargeIn);
                            }
                            let _ = session.start_segment();
                        }
                        Some(VadEvent::Discarded) => {
                            let s = Rc::clone(&session);
                            spawn_local(async move { let _ = s.stop_segment().await; });
                        }
                        Some(VadEvent::UtteranceEnd { .. }) => {
                            handle_utterance(
                                Rc::clone(&session),
                                dash,
                                chat,
                                dispatch.clone(),
                                caption,
                                Rc::clone(&player),
                                Rc::clone(&splitter),
                                Rc::clone(&speak_run),
                                Rc::clone(&consecutive_errors),
                                voice_mode,
                            );
                        }
                        None => {}
                    }
                },
                std::time::Duration::from_millis(50),
            );
            if let Ok(h) = handle {
                *tick_handle.borrow_mut() = Some(h);
            }
        });
    }

    // ── Sentence pipeline: watch the streaming assistant message ──
    {
        let splitter = Rc::clone(&splitter);
        let speak_run = Rc::clone(&speak_run);
        let player = Rc::clone(&player);
        Effect::new(move |_| {
            let Some(run_id) = speak_run.borrow().clone() else { return };
            let target = format!("assistant-{run_id}");
            let (content, streaming) = chat.messages.with(|msgs| {
                msgs.iter()
                    .rev()
                    .find(|m| m.id == target)
                    .map(|m| (m.content.clone(), m.is_streaming))
                    .unwrap_or_default()
            });
            for s in splitter.borrow_mut().push(&content) {
                player.enqueue(dash, s);
            }
            if !streaming && !content.is_empty() {
                if let Some(tail) = splitter.borrow_mut().finish_with(&content) {
                    player.enqueue(dash, tail);
                }
                player.finalize(dash);
                *speak_run.borrow_mut() = None;
            }
        });
    }

    // ── Exit: esc handled by hotkey (Task 8); ✕ button + cleanup ──
    on_cleanup({
        let mic = Rc::clone(&mic);
        let tick_handle = Rc::clone(&tick_handle);
        let player = Rc::clone(&player);
        move || {
            if let Some(h) = tick_handle.borrow_mut().take() {
                h.clear();
            }
            if let Some(m) = mic.borrow_mut().take() {
                m.close();
            }
            player.stop_all();
        }
    });

    let status_text = move || match phase.get() {
        _ if mic_denied.get() => "需要麦克风权限：系统设置 → 隐私与安全 → 麦克风".to_string(),
        VoicePhase::Listening => "正在聆听".to_string(),
        VoicePhase::Processing => "正在思考".to_string(),
        VoicePhase::Speaking => "正在说话 · 开口即可打断".to_string(),
    };
    let caption_text = move || match caption.get() {
        Caption::Idle => String::new(),
        Caption::User(t) => format!("“{t}”"),
        Caption::Assistant(t) | Caption::Error(t) => t,
    };

    view! {
        <div class="voice-stage">
            <VoiceOrb phase=Signal::derive(move || phase.get()) level=Signal::derive(move || level.get()) error_flash=Signal::derive(move || error_flash.get()) />
            <div class="voice-caption mt-8">{caption_text}</div>
            <div class="voice-hint mt-2">{status_text}</div>
            <button
                class="voice-hint mt-10 hover:text-text-primary transition-colors"
                on:click=move |_| voice_mode.open.set(false)
            >
                "✕ esc 退出"
            </button>
        </div>
    }
}

/// One utterance: stop segment -> transcribe -> push to chat -> arm pipeline.
#[allow(clippy::too_many_arguments)]
fn handle_utterance(
    session: Rc<MicSession>,
    dash: DashboardState,
    chat: ChatState,
    dispatch: impl Fn(VoiceEvent) + Clone + 'static,
    caption: RwSignal<Caption>,
    player: Rc<TtsPlayer>,
    splitter: Rc<RefCell<SentenceSplitter>>,
    speak_run: Rc<RefCell<Option<String>>>,
    consecutive_errors: Rc<RefCell<u32>>,
    voice_mode: VoiceMode,
) {
    spawn_local(async move {
        let Ok((base64, mime)) = session.stop_segment().await else {
            dispatch(VoiceEvent::TranscribeFailed);
            return;
        };
        let resp = dash
            .rpc_call("voice.transcribe", serde_json::json!({ "audio_base64": base64, "mime_type": mime }))
            .await;
        let text = resp
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::trim).map(str::to_string))
            .filter(|t| !t.is_empty());
        let Some(text) = text else {
            let n = { let mut c = consecutive_errors.borrow_mut(); *c += 1; *c };
            dispatch(VoiceEvent::TranscribeFailed);
            if n >= 3 {
                caption.set(Caption::Error("连续没听清——可以 esc 退出用文字".into()));
            }
            return;
        };
        *consecutive_errors.borrow_mut() = 0;
        caption.set(Caption::User(text.clone()));
        dispatch(VoiceEvent::UtteranceSent);

        // Reset the per-run pipeline, then send as a normal chat message.
        player.reset();
        *splitter.borrow_mut() = SentenceSplitter::default();
        chat.push_user_message(&text);
        let sk = chat.session_key.get_untracked();
        match ChatApi::send(&dash, &text, sk.as_deref(), vec![], None, None, None).await {
            Ok(resp) => {
                chat.session_key.set(Some(resp.session_key.clone()));
                chat.start_assistant_message(&resp.run_id);
                // IMPORTANT: do NOT chat.mark_speak_run — we own TTS here.
                *speak_run.borrow_mut() = Some(resp.run_id);
            }
            Err(_) => {
                dispatch(VoiceEvent::RunFailed);
                let _ = voice_mode; // session stays open; user may retry by speaking
            }
        }
    });
}
```

> 实施者注意：①`set_interval_with_handle`/`IntervalHandle`/`set_timeout` 来自 `leptos::leptos_dom::helpers`，确认 0.8 的导入路径。②`ChatApi::send` 的参数（attachments/agent/provider/model）与返回字段名以 `api/chat.rs` 现状为准；`start_assistant_message` / `push_user_message` 的确切签名以 `state.rs:441-466` 为准。③`ChatMessage` 的 `is_streaming` 字段名以 `state.rs` 为准（勘察报告基于 main@13a25a95e）。④流式订阅前提：`subscribe_topic("stream.*")` 由 ChatView 挂载时建立（view.rs:46）——沉浸模式覆盖在 ChatView 之上（不卸载它），订阅天然存活；若改为 app 级挂载，需自行 subscribe。

- [ ] **Step 3: app.rs 挂载**

在 App 根组件（provide context 区域）：

```rust
use crate::views::voice::{ImmersiveVoiceView, VoiceMode};
// in App body, alongside other provide_context calls:
provide_context(VoiceMode::new());
```

在根 view 的最外层容器内（与既有顶层结构并列，z-40 覆盖一切常规 UI）：

```rust
<ImmersiveVoiceView />
```

- [ ] **Step 4: wasm 编译 + clippy + host 测试回归**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings
cargo test -p aleph-panel --lib
```

Expected: 全过（voice:: 纯函数测试 15 个 + 既有测试）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/voice/ interfaces/webchat/src/app.rs
git commit -m "panel: immersive voice session — vad loop, transcribe-send, sentence tts pipeline"
```

---

### Task 8: 入口接线 — composer 迷你球 + 快捷键

**Files:**
- Modify: `interfaces/webchat/src/views/chat/composer/voice.rs`（按钮换皮 + 点击语义）
- Modify: `interfaces/webchat/src/state/hotkey.rs`（⌘⇧V / Ctrl+Alt+V）

- [ ] **Step 1: VoiceInputButton 改造（voice.rs:366-419 区域）**

语义：**点击 = 进沉浸模式；长按（≥450ms）= 原录音转文字进输入框**。

```rust
// 在组件内新增：
let voice_mode = expect_context::<crate::views::voice::VoiceMode>();
let press_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
let long_press_fired = StoredValue::new(false);

// pointerdown: 起 450ms 计时器，到时触发原 start_recording 路径
let on_pointer_down = move |_| {
    long_press_fired.set_value(false);
    let h = set_timeout_with_handle(
        move || {
            long_press_fired.set_value(true);
            start_recording(); // 原有录音入口函数（保持既有 RecState 流程）
        },
        std::time::Duration::from_millis(450),
    );
    if let Ok(h) = h { press_timer.set_value(Some(h)); }
};
// pointerup: 未到长按阈值 → 取消计时器；若在录音中则按原停止逻辑，否则进沉浸模式
let on_pointer_up = move |_| {
    if let Some(h) = press_timer.get_value() { h.clear(); }
    press_timer.set_value(None);
    if long_press_fired.get_value() {
        stop_recording(); // 原有停止+转写路径
    } else if rec_state.get_untracked() == RecState::Idle {
        voice_mode.open.set(true);
    }
};
```

按钮 DOM：Idle 态渲染迷你流光球替代原麦克风 SVG（Recording/Transcribing 态保持原视觉以兼容长按流程）：

```rust
// Idle 态的按钮内容：
<div class="voice-orb voice-orb--mini voice-orb--listening">
    <div class="voice-orb-flow"></div>
    <div class="voice-orb-sheen"></div>
</div>
```

> 实施者注意：原组件的 click 处理需移除/合并进 pointerup（避免双触发）；`start_recording`/`stop_recording` 是对原函数体的指代——重构为可复用闭包时保持 RecState 状态机与 native/browser 回退逻辑原样。title 属性更新为"点击进入语音模式 · 长按说话转文字"。

- [ ] **Step 2: hotkey.rs 追加（仿 ⌘K 模式，hotkey.rs:56-78 同函数内）**

```rust
// Voice mode: ⌘⇧V (macOS) / Ctrl+Alt+V (Win/Linux — avoids plain-text-paste clash)
let is_mac_combo = ev.meta_key() && ev.shift_key() && ev.key().eq_ignore_ascii_case("v");
let is_pc_combo = ev.ctrl_key() && ev.alt_key() && ev.key().eq_ignore_ascii_case("v");
if is_mac_combo || is_pc_combo {
    ev.prevent_default();
    let vm = expect_context::<crate::views::voice::VoiceMode>(); // 或经 HotkeyState 穿线，对齐现有依赖注入方式
    vm.open.update(|o| *o = !*o);
}
// Escape closes voice mode (priority before palette/sidebar branches)
if ev.key() == "Escape" {
    let vm = expect_context::<crate::views::voice::VoiceMode>();
    if vm.open.get_untracked() {
        ev.prevent_default();
        vm.open.set(false);
        return;
    }
}
```

> 实施者注意：hotkey.rs 的 `install(state)` 若在 context 建立前调用，`expect_context` 会 panic——核对 app.rs 中 install 时序，必要时把 `VoiceMode` 作为字段加进 `HotkeyState` 穿线（对齐既有模式）。Escape 分支必须放在 palette Escape 之前并 early-return，保证层级语义（最上层覆盖层先响应）。

- [ ] **Step 3: wasm 编译 + clippy + 测试**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings
cargo test -p aleph-panel --lib
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/chat/composer/voice.rs interfaces/webchat/src/state/hotkey.rs
git commit -m "panel: voice entry — mini orb button (tap=immersive, hold=dictate) + hotkey"
```

---

### Task 9: 视觉验收 — 三材质 × 五色板 + 无障碍回退

**Files:** 无代码改动（发现问题则回改 Task 1 的 CSS 并 amend/新 commit）

- [ ] **Step 1: 全量构建**

Run: `just wasm`（worktree 内若 wasm-bindgen 路径错配，手动以绝对路径执行主仓 target 下的产物处理——见 plan 头部红线）
Expected: dist/ 四件套更新。

- [ ] **Step 2: standalone 视觉矩阵截图**

写 `/tmp/voice_visual_matrix.html` 引用编译后 `dist/tailwind.css`，内嵌沉浸舞台 + 三态球（listening/processing/speaking 各一，`--voice-level` 分别 0.1/0/0.6）。用 chrome-devtools MCP：对 `html` 依次设 `data-material ∈ {luxe, liquid, aurora}` × `data-accent ∈ {mauve, ocean, forest, sunset, rose}`（含 `.dark`），逐组截图。

验收标准：
- 流光主调跟随 accent（Ocean 蓝青 / Sunset 橙金……），无脏色
- 三态流速肉眼可辨（speaking 明显快于 processing）
- 暗色模式下球不发灰、光晕不糊成白块

- [ ] **Step 3: 无障碍回退验证**

chrome-devtools `emulate` 设 `prefers-reduced-motion: reduce` → 截图确认球静止、仅透明度随 level 变化；`prefers-reduced-transparency: reduce` → 确认实心球无混色伪影。

- [ ] **Step 4: 修色（如需）+ Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: voice orb visual polish from theme-matrix acceptance"
```

（无修改则跳过本 commit。）

---

### Task 10: 部署 + 人工 E2E 验收（HUMAN GATE）

**Files:** 无新代码。

- [ ] **Step 1: 烧录部署链（CLAUDE.md Panel↔Daemon 嵌入链）**

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
# dev daemon: ./target/release/aleph-server stop && cargo run --release -p alephcore --bin aleph-server start
# .app daemon: mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak} && cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/ && kill <pid>
```

部署后 `pgrep -f aleph-server` + `curl -s http://127.0.0.1:18790` 复验存活。

- [ ] **Step 2: 人工验收清单（用户执行）**

1. composer 看到迷你流光球；点击 → 全屏沉浸模式淡入，球呼吸
2. 直接说话："今天天气怎么样" → 不按任何键，停顿后自动转写（字幕闪现你的话）→ 球转思考态 → AI 第一句话开始播放（首响延迟感受记录）
3. AI 说话中途开口打断 → 播放立停、回聆听态
4. esc 退出 → 聊天流里完整可见刚才的语音对话（文字）
5. ⌘⇧V 再次进入；长按迷你球 → 原"说话转文字进输入框"仍工作
6. （可选）系统设置撤销麦克风权限 → 进入沉浸模式显示权限指引而非崩溃
7. 三主题/暗色切换下球的观感

- [ ] **Step 3: 验收结论记录**

通过 → 分支收尾（最终 code review → merge）；不通过 → 按反馈开修复任务。

---

## Self-Review 记录

- **Spec 覆盖**：§2 六决策 → Task 1(视觉)/2+5(VAD+AEC)/3+5(句级流水线)/4(状态机)/7(纯净剧场布局+字幕)/8(入口双语义+快捷键)；§6 错误表 → Task 7（mic_denied/连续 3 次转写失败/TTS 单句失败 caption-only/RunFailed）+ Task 1 Step 4（a11y）；§7 测试 1-6 → Task 2/3/4 单测、各任务 wasm 验证、Task 9 视觉、Task 10 人工。§6"切后台暂停聆听"一期以 mic close-on-cleanup 兜底（覆盖层关闭即释放），页面级 visibilitychange 监听列为验收观察项——若人工验收发现切后台仍录，加 visibilitychange 暂停（小改）。
- **占位符扫描**：无 TBD/TODO；"实施者注意"块均为 API 形状核对指引（行号可能漂移），非缺失实现。
- **类型一致性**：`VoicePhase`/`VoiceEvent`/`Action`（Task 4 定义，Task 6/7 消费）、`VadConfig/VadState/VadEvent`（Task 2 定义，Task 7 消费）、`SentenceSplitter::push/finish_with`（Task 3 定义，Task 7 消费签名一致）、`MicSession::{open,rms,start_segment,stop_segment,close}`/`TtsPlayer::{new,reset,enqueue,finalize,stop_all}`（Task 5 定义，Task 7 消费一致）已逐一核对。
