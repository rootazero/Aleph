# aleph-voice 本地语音 Sidecar（Tier 0+1）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增独立 Rust sidecar `aleph-voice`（sherpa-onnx 推理：SenseVoice STT + Kokoro v1.1-zh TTS），daemon 按需拉起、闲置释放、模型运行时下载，接入现有 OpenAI 兼容 provider 缝，所有渠道半双工本地语音可用，本地/云可经配置后台切换。

**Architecture:** 见 spec `docs/superpowers/specs/2026-06-12-local-voice-sidecar-design.md`。sidecar 是独立 workspace binary crate（axum loopback + Bearer token 握手 + 每引擎懒加载/闲置卸载状态机 + ModelManager 多源断点下载）。core 侧：`Config::load()` 归一化注入 `"local"` provider → factory `"local"` arm → `SttSource` 晚绑定改造（动态端口）→ TTS 下载中预检（不计失败）。

**Tech Stack:** Rust workspace member / sherpa-rs (sherpa-onnx) / axum 0.8 / symphonia + opus + ogg + rubato + hound / reqwest + sha2 / clap derive / Tauri externalBin。

**执行注意（来自仓库纪律）：**
- 在独立 git worktree 中执行（superpowers:using-git-worktrees）；本仓 main 领先 origin，worktree 必须基于本地 `HEAD` 而非 origin（`git worktree add <dir> HEAD`）。
- 仓库根 `.cargo/config.toml`（机器本地、不入库）钉了共享 target-dir + flock：并行 cargo 排队是预期，**严禁**另设 `CARGO_TARGET_DIR`。worktree 下 `wasm-bindgen`/产物相对路径会错配——本计划不碰 wasm，无此雷。
- 提交规范：English，`<scope>: <description>`，无 attribution 尾注。
- Task 1（spike）含**人工听感门**，必须停下等用户 verdict 才能进 Task 6；Task 2-5 不依赖 spike，可在等待期间先行。

---

## 文件结构总览

**新建（sidecar crate）：**

```
aleph-voice/
├── Cargo.toml
├── src/
│   ├── main.rs            # bin（required-features=["sherpa"]）：clap serve、READY 握手、tick 循环
│   ├── lib.rs             # pub mod engine/lifecycle/models/audio/server
│   ├── engine/
│   │   ├── mod.rs         # SttEngine/TtsEngine trait + SttResult/TtsAudio
│   │   ├── mock.rs        # MockStt/MockTts（测试与 server 单测用，常编）
│   │   └── sherpa.rs      # #[cfg(feature="sherpa")] SenseVoice + Kokoro 实现
│   ├── lifecycle.rs       # EngineSlot<E> 懒加载槽 + should_unload/should_exit 纯函数
│   ├── models/
│   │   ├── mod.rs         # ModelManager：多源下载/sha256/断点续传/解包/状态
│   │   └── manifest.rs    # ModelSpec 静态表（sha256 实值由 Task 1 spike 产出）
│   ├── audio/
│   │   ├── mod.rs         # decode_to_pcm_mono_16k / encode_wav / 重采样(rubato)
│   │   └── ogg_opus.rs    # OGG-Opus 编解码（TG 语音气泡格式）
│   └── server/
│       ├── mod.rs         # Router 组装 + AppState
│       ├── auth.rs        # Bearer token 中间件
│       └── handlers.rs    # transcriptions/speech/status/warmup 四端点
├── examples/              # Task 1 spike（required-features=["sherpa"]）
│   ├── tts_spike.rs
│   ├── stt_spike.rs
│   └── mem_spike.rs
└── tests/fixtures/tone.mp3  # Task 4 音频解码 fixture
```

**修改（core / 打包）：**

| 文件 | 改动 |
|---|---|
| `Cargo.toml`（根） | workspace members += `"aleph-voice"` |
| `src/config/types/voice_local.rs`（新） | `VoiceLocalConfig` + `normalize_voice_local()` |
| `src/config/types/mod.rs` | 导出 voice_local |
| `src/config/structs.rs:106-108` | `Config` 加 `voice_local` 字段（generation 字段后） |
| `src/config/load.rs` | 解析成功后调 `normalize_voice_local(&mut config)` |
| `src/gateway/voice/sidecar.rs`（新） | `VoiceSidecarSupervisor` + OnceLock 全局 |
| `src/gateway/voice/local_provider.rs`（新） | `LocalTranscription` + `LocalVoiceProvider` |
| `src/gateway/voice/mod.rs` | 注册两个新模块 |
| `src/generation/providers/factory.rs` | `"local"` arm |
| `src/gateway/voice/inbound.rs` | `resolve_stt_config` → `resolve_stt_source`（SttSource 晚绑定 + 云回退） |
| `src/gateway/inbound_router/mod.rs:116,279,587-598` | `stt_config` → `stt_source` |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs:776-786` | 改用 `resolve_stt_source` |
| `src/gateway/handlers/voice.rs:75` | 同上（panel RPC） |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1113-1149` | MediaProcessor 分支 LocalTranscription |
| `src/gateway/voice/outbound.rs` | `TtsOutcome` + `generate_tts_outcome`（下载中预检） |
| `src/gateway/reply_emitter/emitter/helpers.rs:83-134` | 改用 outcome，Downloading 不计失败 |
| `src/builtin_tools/voice_tools/local_voice.rs`（新）+ `mod.rs` | `local_voice` 工具 |
| `src/executor/builtin_registry/groups.rs:100` 附近 | TOOL_CATEGORIES += local_voice |
| `src/executor/builtin_registry/builder/optional_tools.rs:460` 附近 | reg() 注册 |
| `src/executor/builtin_registry/registry.rs:1508` 附近 | dispatch arm |
| `src/builtin_tools/voice_tools/voice_mode_set.rs:96-109` | enable 时 fire-and-forget warmup |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` 或 boot 早期 | `sidecar::init_global(cfg.voice_local)` |
| `justfile` | `voice-build`/`voice-test` recipe + `build` 链 + `_stage-shell-binaries` + `test-all` |
| `desktop/shell/tauri.conf.json:20` | externalBin += `"binaries/aleph-voice"` |

---

### Task 1: Tier-0 Spike — sherpa-rs 真机验证（含人工听感门）

Spike 性质：不走 TDD，产出 **verdict 文档 + manifest 实值**。任一验证门不过 → 回 spec 换模型（TTS 备胎 MeloTTS-zh_en / STT 备胎 whisper-onnx，同库内）。

**Files:**
- Modify: `Cargo.toml`（根，workspace members）
- Create: `aleph-voice/Cargo.toml`
- Create: `aleph-voice/src/lib.rs`（空壳）、`aleph-voice/src/main.rs`（占位）
- Create: `aleph-voice/examples/tts_spike.rs`、`stt_spike.rs`、`mem_spike.rs`
- Create: `Scripts/voice_models_fetch.sh`
- Create: `aleph-voice/src/models/manifest.rs`（含 spike 实测 sha256）
- Create: `docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md`

- [ ] **Step 1: workspace 注册 + crate 骨架**

根 `Cargo.toml` 的 `members` 数组（`"interfaces/webchat",` 之后）追加：

```toml
    "aleph-voice",
```

`aleph-voice/Cargo.toml`：

```toml
[package]
name = "aleph-voice"
description = "Local voice inference sidecar (STT/TTS) for Aleph"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[features]
default = ["sherpa"]
sherpa = ["dep:sherpa-rs"]

[[bin]]
name = "aleph-voice"
path = "src/main.rs"
required-features = ["sherpa"]

[[example]]
name = "tts_spike"
required-features = ["sherpa"]

[[example]]
name = "stt_spike"
required-features = ["sherpa"]

[[example]]
name = "mem_spike"
required-features = ["sherpa"]

[dependencies]
sherpa-rs = { version = "0.6", optional = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = "1.0"
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = "1.0"
async-trait = { workspace = true }
futures = { workspace = true }
uuid = { workspace = true }
clap = { version = "4.4", features = ["derive"] }
axum = { version = "0.8", features = ["multipart"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream"] }
sha2 = "0.10"
hound = "3.5"
opus = "0.3"
ogg = "0.9"
symphonia = { version = "0.5", features = ["mp3", "aac", "isomp4", "mkv"] }
rubato = "0.16"
bzip2 = "0.4"
tar = "0.4"
dirs = "6.0"

[dev-dependencies]
tempfile = "3.8"
tower = { version = "0.5", features = ["util"] }
```

> 注：若 `cargo check` 报某 workspace 依赖（如 `tracing-subscriber`）无 `workspace = true` 定义，按报错改为显式版本（根 `[workspace.dependencies]` 以实际为准）。

`aleph-voice/src/lib.rs`：

```rust
//! aleph-voice — local voice inference sidecar library.
//! Modules land in Tasks 2-7; this file starts empty on purpose.
```

`aleph-voice/build.rs`（CalVer 单一来源：VERSION 文件 → `ALEPH_VERSION`，与 alephcore 同规——禁用 `CARGO_PKG_VERSION`）：

```rust
fn main() {
    let version = std::fs::read_to_string("../VERSION")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "0.0.0".to_string());
    println!("cargo:rustc-env=ALEPH_VERSION={version}");
    println!("cargo:rerun-if-changed=../VERSION");
}
```

`aleph-voice/src/main.rs` 占位（Task 8 重写）：

```rust
fn main() {
    eprintln!("aleph-voice: placeholder binary (Task 8 wires the real serve loop)");
}
```

- [ ] **Step 2: 验证 crate 编译（首次会拉 sherpa-onnx C++ 构建，需 cmake，耗时数分钟）**

Run: `command -v cmake && cargo check -p aleph-voice 2>&1 | tail -5`
Expected: `Finished` 行。若 sherpa-rs 0.6 不存在/版本不符 → `cargo search sherpa-rs` 取最新版回填 Cargo.toml（记入 verdict 文档）。

- [ ] **Step 3: 模型下载脚本**

`Scripts/voice_models_fetch.sh`：

```bash
#!/usr/bin/env bash
# Fetch SenseVoice + Kokoro model packages for the aleph-voice spike.
# Usage: ./Scripts/voice_models_fetch.sh [github|hf-mirror]
set -euo pipefail

DEST="${ALEPH_HOME:-$HOME/.aleph}/models/voice"
mkdir -p "$DEST"
SOURCE="${1:-github}"

SENSE_FILE="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2"
KOKORO_FILE="kokoro-multi-lang-v1_1.tar.bz2"

case "$SOURCE" in
  github)
    SENSE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$SENSE_FILE"
    KOKORO_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$KOKORO_FILE"
    ;;
  hf-mirror)
    SENSE_URL="https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/$SENSE_FILE"
    KOKORO_URL="https://hf-mirror.com/csukuangfj/kokoro-multi-lang-v1_1/resolve/main/$KOKORO_FILE"
    ;;
  *) echo "unknown source: $SOURCE"; exit 1 ;;
esac

for pair in "sense-voice-small|$SENSE_URL|$SENSE_FILE" "kokoro-v1.1-zh|$KOKORO_URL|$KOKORO_FILE"; do
  IFS='|' read -r id url file <<< "$pair"
  echo "==> $id"
  curl -L -C - -o "$DEST/$file" "$url"
  echo "sha256:"
  shasum -a 256 "$DEST/$file"
  mkdir -p "$DEST/$id"
  tar -xjf "$DEST/$file" -C "$DEST/$id" --strip-components=1
  ls "$DEST/$id" | head -20
done
echo "Done. Models under $DEST"
```

Run: `chmod +x Scripts/voice_models_fetch.sh && ./Scripts/voice_models_fetch.sh github`
Expected: 两个目录解出（sense-voice 含 `model.int8.onnx`+`tokens.txt`；kokoro 含 `model.onnx`+`voices.bin`+`tokens.txt`+`espeak-ng-data/`+lexicon/dict 文件）。**记录两行 sha256 与实际文件清单**（manifest 与 verdict 要用）。若 URL 404 → 到 `github.com/k2-fsa/sherpa-onnx/releases` 检索同名最新资产，更正脚本并记录。

- [ ] **Step 4: TTS spike**

`aleph-voice/examples/tts_spike.rs`：

```rust
//! Kokoro v1.1-zh spike: synthesize zh/en/mixed sentences, report timing.
//! Run: cargo run -p aleph-voice --example tts_spike --release
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice/kokoro-v1.1-zh");
    let p = |f: &str| home.join(f).to_string_lossy().into_owned();

    let t0 = Instant::now();
    // NOTE: struct/field names follow sherpa-rs 0.6 docs.rs; reconcile with the
    // installed version if compile fails, and record the final shape in the verdict doc.
    let mut tts = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
        model: p("model.onnx"),
        voices: p("voices.bin"),
        tokens: p("tokens.txt"),
        data_dir: p("espeak-ng-data"),
        lexicon: format!("{},{}", p("lexicon-us-en.txt"), p("lexicon-zh.txt")),
        dict_dir: p("dict"),
        length_scale: 1.0,
        ..Default::default()
    });
    println!("load: {:?}", t0.elapsed());

    let cases = [
        ("zh", "你好，我是 Aleph，本地语音引擎已经就绪。"),
        ("en", "Hello, this is the local text to speech engine."),
        ("mixed", "我们用 sherpa-onnx 跑 Kokoro 模型，首包延迟 first packet latency 很关键。"),
    ];
    // Try a few speaker ids — Chinese voices live at some sid range; record which sound right.
    for sid in [0_i32, 1, 50, 100] {
        for (tag, text) in &cases {
            let t = Instant::now();
            let audio = tts.create(text, sid, 1.0)?;
            let ms = t.elapsed().as_millis();
            let out = format!("/tmp/aleph_spike_tts_sid{sid}_{tag}.wav");
            write_wav(&out, &audio.samples, audio.sample_rate)?;
            println!("sid={sid} {tag}: {}ms, {} samples @ {}Hz -> {out}", ms, audio.samples.len(), audio.sample_rate);
        }
    }
    Ok(())
}

fn write_wav(path: &str, samples: &[f32], rate: u32) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}
```

Run: `cargo run -p aleph-voice --example tts_spike --release`
Expected: 各句合成毫秒数 + `/tmp/aleph_spike_tts_*.wav` 落盘。**编译失败 = sherpa-rs API 形状与计划假设不符**：以 `~/.cargo/registry` 内该版本源码或 docs.rs 为准修正（KokoroTts 构造名 / create 签名 / config 字段），把最终真实 API 形状记入 verdict 文档（Task 6 直接消费）。

- [ ] **Step 5: STT spike**

`aleph-voice/examples/stt_spike.rs`：

```rust
//! SenseVoice spike: transcribe the TTS spike wavs (zh/en/mixed), report timing.
//! Run: cargo run -p aleph-voice --example stt_spike --release
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice/sense-voice-small");
    let p = |f: &str| home.join(f).to_string_lossy().into_owned();

    let t0 = Instant::now();
    // NOTE: reconcile names with installed sherpa-rs (see Step 4 note).
    let mut stt = sherpa_rs::sense_voice::SenseVoice::new(sherpa_rs::sense_voice::SenseVoiceConfig {
        model: p("model.int8.onnx"),
        tokens: p("tokens.txt"),
        language: "auto".into(),
        use_itn: true,
        ..Default::default()
    })?;
    println!("load: {:?}", t0.elapsed());

    for tag in ["zh", "en", "mixed"] {
        let path = format!("/tmp/aleph_spike_tts_sid0_{tag}.wav");
        let mut r = hound::WavReader::open(&path)?;
        let rate = r.spec().sample_rate;
        let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let t = Instant::now();
        let text = stt.transcribe(rate, samples);
        println!("{tag}: {}ms -> {:?}", t.elapsed().as_millis(), text);
    }
    Ok(())
}
```

Run: `cargo run -p aleph-voice --example stt_spike --release`
Expected: 三段各 ~几十至几百 ms，文本与原句基本一致（这是闭环：TTS 念 → STT 听回）。另用一段**真人录音**（QuickTime 录 10s 中英混说存 wav）替换 path 跑一次，记录真实准确度。

- [ ] **Step 6: 内存释放 spike**

`aleph-voice/examples/mem_spike.rs`：

```rust
//! Verify deterministic memory release on engine drop (spec gate #3).
//! Run: cargo run -p aleph-voice --example mem_spike --release
fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn main() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice");
    let k = |f: &str| home.join("kokoro-v1.1-zh").join(f).to_string_lossy().into_owned();
    println!("baseline: {:.1} MB", rss_mb());
    {
        let mut tts = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
            model: k("model.onnx"), voices: k("voices.bin"), tokens: k("tokens.txt"),
            data_dir: k("espeak-ng-data"),
            lexicon: format!("{},{}", k("lexicon-us-en.txt"), k("lexicon-zh.txt")),
            dict_dir: k("dict"), length_scale: 1.0, ..Default::default()
        });
        let _ = tts.create("预热一句。", 0, 1.0)?;
        println!("tts loaded: {:.1} MB", rss_mb());
    } // drop
    println!("tts dropped: {:.1} MB", rss_mb());
    Ok(())
}
```

Run: `cargo run -p aleph-voice --example mem_spike --release`
Expected: loaded 比 baseline 高数百 MB；**dropped 回落到 baseline 附近**（差值 < 50MB 即判定确定性回收成立；allocator 缓存少量保留可接受）。

- [ ] **Step 7: 写 manifest 实值**

`aleph-voice/src/models/manifest.rs`（sha256/URL 用 Step 3 实测值替换尖括号示意——**此文件提交时不得残留尖括号**）：

```rust
//! Static model manifest. sha256 values were measured by the Tier-0 spike
//! (docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md) — update both
//! together when bumping model versions.

/// A downloadable model package (bzip2 tarball, unpacked into `<root>/<id>/`).
pub struct ModelSpec {
    /// Directory name under the models root; also the config-facing model id.
    pub id: &'static str,
    /// Download sources in priority order (github → hf-mirror).
    pub urls: &'static [&'static str],
    /// sha256 of the tarball.
    pub sha256: &'static str,
    /// File that proves a complete unpack.
    pub marker: &'static str,
}

pub const SENSE_VOICE_SMALL: ModelSpec = ModelSpec {
    id: "sense-voice-small",
    urls: &[
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2",
        "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2",
    ],
    sha256: "<spike-measured>",
    marker: "model.int8.onnx",
};

pub const KOKORO_V11_ZH: ModelSpec = ModelSpec {
    id: "kokoro-v1.1-zh",
    urls: &[
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2",
        "https://hf-mirror.com/csukuangfj/kokoro-multi-lang-v1_1/resolve/main/kokoro-multi-lang-v1_1.tar.bz2",
    ],
    sha256: "<spike-measured>",
    marker: "model.onnx",
};

/// Look up a spec by config-facing id.
pub fn spec_for(id: &str) -> Option<&'static ModelSpec> {
    match id {
        "sense-voice-small" => Some(&SENSE_VOICE_SMALL),
        "kokoro-v1.1-zh" => Some(&KOKORO_V11_ZH),
        _ => None,
    }
}
```

并在 `aleph-voice/src/lib.rs` 加 `pub mod models;`、新建 `aleph-voice/src/models/mod.rs` 暂只 `pub mod manifest;`（Task 5 扩成 ModelManager）。

Run: `cargo check -p aleph-voice --no-default-features && grep -c "spike-measured" aleph-voice/src/models/manifest.rs`
Expected: 编译过且 grep 计数为 **0**。

- [ ] **Step 8: 🛑 人工听感门（HUMAN GATE — 暂停执行）**

请用户 `open /tmp/aleph_spike_tts_sid*_zh.wav` 等逐个试听，回答：
1. 中文听感是否可接受？哪个 sid 最佳（定为默认 `tts_voice`，并记 sid↔名称映射）？
2. STT 真人录音准确度是否可接受？
3. 四个 spec 验证门（听感/转写/内存回落/sherpa-rs 覆盖度）verdict 各为 PASS/FAIL？

任一 FAIL → 停止，回 spec 换模型重跑本 Task。全 PASS → 继续。

- [ ] **Step 9: 写 verdict 文档并提交**

`docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md` 记录：四门 verdict、实测延迟/加载时长/RSS 曲线、两包 sha256 与体积、kokoro 实际文件清单、**sherpa-rs 最终真实 API 形状**（含与计划假设的差异）、选定默认 sid 与音色映射表、sherpa 类型是否 `Send`（`fn is_send<T: Send>() {}` 编译探针验证，Task 6 要用）。

```bash
git add Cargo.toml aleph-voice/ Scripts/voice_models_fetch.sh docs/superpowers/spikes/
git commit -m "voice: tier-0 spike — sherpa-rs kokoro/sensevoice verdicts + model manifest"
```

---

### Task 2: 引擎 trait + Mock

**Files:**
- Create: `aleph-voice/src/engine/mod.rs`
- Create: `aleph-voice/src/engine/mock.rs`
- Modify: `aleph-voice/src/lib.rs`

- [ ] **Step 1: 写 trait 与 mock（含失败测试一并写出）**

`aleph-voice/src/engine/mod.rs`：

```rust
//! Engine abstraction — isolates sherpa-rs behind small traits so the
//! protocol/lifecycle layers never see the backend (swap-friendly, mock-testable).

pub mod mock;
#[cfg(feature = "sherpa")]
pub mod sherpa;

/// Transcription result.
#[derive(Debug, Clone)]
pub struct SttResult {
    pub text: String,
    pub language: Option<String>,
}

/// Synthesized audio: mono f32 PCM.
#[derive(Debug, Clone)]
pub struct TtsAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Speech-to-text engine. Input is 16 kHz mono f32 PCM.
/// Sync on purpose — callers run it inside `spawn_blocking`.
pub trait SttEngine: Send + Sync {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> anyhow::Result<SttResult>;
}

/// Text-to-speech engine.
pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str, voice: &str, speed: f32) -> anyhow::Result<TtsAudio>;
}
```

`aleph-voice/src/engine/mock.rs`：

```rust
//! Deterministic mock engines for server/lifecycle tests.

use super::{SttEngine, SttResult, TtsAudio, TtsEngine};

/// Echoes the sample count so tests can assert the decode path ran.
pub struct MockStt;

impl SttEngine for MockStt {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> anyhow::Result<SttResult> {
        Ok(SttResult {
            text: format!("mock transcript ({} samples)", samples.len()),
            language: language.map(str::to_string),
        })
    }
}

/// Produces 100 ms of 440 Hz sine at 24 kHz regardless of input.
pub struct MockTts;

impl TtsEngine for MockTts {
    fn synthesize(&self, _text: &str, _voice: &str, _speed: f32) -> anyhow::Result<TtsAudio> {
        let rate = 24_000u32;
        let samples = (0..rate / 10)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect();
        Ok(TtsAudio { samples, sample_rate: rate })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_stt_reports_sample_count_and_language() {
        let r = MockStt.transcribe(&[0.0; 320], Some("zh")).unwrap();
        assert!(r.text.contains("320"));
        assert_eq!(r.language.as_deref(), Some("zh"));
    }

    #[test]
    fn mock_tts_emits_100ms_audio() {
        let a = MockTts.synthesize("hi", "0", 1.0).unwrap();
        assert_eq!(a.sample_rate, 24_000);
        assert_eq!(a.samples.len(), 2_400);
    }
}
```

`lib.rs` 加 `pub mod engine;`。

- [ ] **Step 2: 跑测试**

Run: `cargo test -p aleph-voice --no-default-features --lib`
Expected: 2 passed。

- [ ] **Step 3: Commit**

```bash
git add aleph-voice/src/engine/ aleph-voice/src/lib.rs
git commit -m "voice: engine traits (SttEngine/TtsEngine) + deterministic mocks"
```

---

### Task 3: 生命周期 — EngineSlot 懒加载槽 + 闲置决策纯函数

**Files:**
- Create: `aleph-voice/src/lifecycle.rs`
- Modify: `aleph-voice/src/lib.rs`（`pub mod lifecycle;`）

- [ ] **Step 1: 先写失败测试（文件尾 `#[cfg(test)]`，与实现同文件提交，先空实现跑红再补——TDD 红绿在本步内完成记录）**

实现 + 测试一体（`aleph-voice/src/lifecycle.rs`）：

```rust
//! Engine lifecycle: lazy load-on-demand, idle unload, deep-idle process exit.
//!
//! Pure decision functions take explicit `now_ms` so tests need no clocks.
//! `EngineSlot` queues concurrent loaders behind one async mutex — the spec's
//! "Loading 期间请求排队 hold" falls out of the lock for free.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Epoch milliseconds now (single definition; tests pass values directly).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Should an idle engine be unloaded? `last_used_ms == 0` means never used.
pub fn should_unload(last_used_ms: u64, now_ms: u64, ttl_secs: u64) -> bool {
    last_used_ms != 0 && now_ms.saturating_sub(last_used_ms) >= ttl_secs * 1000
}

/// Should the whole process exit? Only when nothing happened for `idle_exit_secs`.
pub fn should_exit(last_activity_ms: u64, now_ms: u64, idle_exit_secs: u64) -> bool {
    now_ms.saturating_sub(last_activity_ms) >= idle_exit_secs * 1000
}

/// Lazy-loaded engine holder. Load runs in `spawn_blocking`; concurrent callers
/// queue on the mutex and reuse the freshly loaded engine.
pub struct EngineSlot<E: ?Sized + Send + Sync> {
    state: tokio::sync::Mutex<Option<Arc<E>>>,
    last_used_ms: AtomicU64,
}

impl<E: ?Sized + Send + Sync + 'static> EngineSlot<E> {
    pub fn new() -> Self {
        Self { state: tokio::sync::Mutex::new(None), last_used_ms: AtomicU64::new(0) }
    }

    /// Get the engine, loading it via `load` if absent. Marks use time.
    pub async fn get_or_load<F>(&self, now: u64, load: F) -> anyhow::Result<Arc<E>>
    where
        F: FnOnce() -> anyhow::Result<Arc<E>> + Send + 'static,
    {
        let mut guard = self.state.lock().await;
        if guard.is_none() {
            let loaded = tokio::task::spawn_blocking(load).await??;
            *guard = Some(loaded);
        }
        self.last_used_ms.store(now, Ordering::Relaxed);
        Ok(guard.as_ref().expect("just set").clone())
    }

    /// Drop the engine if idle past `ttl_secs`. Returns true when unloaded.
    pub async fn maybe_unload(&self, ttl_secs: u64, now: u64) -> bool {
        let mut guard = self.state.lock().await;
        if guard.is_some() && should_unload(self.last_used_ms.load(Ordering::Relaxed), now, ttl_secs) {
            *guard = None;
            return true;
        }
        false
    }

    pub async fn is_loaded(&self) -> bool {
        self.state.lock().await.is_some()
    }

    pub fn last_used_ms(&self) -> u64 {
        self.last_used_ms.load(Ordering::Relaxed)
    }
}

impl<E: ?Sized + Send + Sync + 'static> Default for EngineSlot<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn unload_decision_table() {
        assert!(!should_unload(0, 999_999, 1)); // never used
        assert!(!should_unload(1_000, 100_999, 120)); // 99.999s < 120s
        assert!(should_unload(1_000, 121_000, 120)); // exactly ttl
    }

    #[test]
    fn exit_decision() {
        assert!(!should_exit(1_000, 1_000 + 1_799_999, 1_800));
        assert!(should_exit(1_000, 1_000 + 1_800_000, 1_800));
    }

    #[tokio::test]
    async fn loads_once_under_concurrency() {
        let slot: Arc<EngineSlot<crate::engine::mock::MockStt>> = Arc::new(EngineSlot::new());
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        let mk = || {
            LOADS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok(Arc::new(crate::engine::mock::MockStt))
        };
        let (a, b) = tokio::join!(slot.get_or_load(1, mk), slot.get_or_load(2, mk));
        a.unwrap();
        b.unwrap();
        assert_eq!(LOADS.load(Ordering::SeqCst), 1, "second caller must reuse the load");
        assert!(slot.is_loaded().await);
    }

    #[tokio::test]
    async fn unloads_after_ttl_and_reloads() {
        let slot: EngineSlot<crate::engine::mock::MockStt> = EngineSlot::new();
        slot.get_or_load(1_000, || Ok(Arc::new(crate::engine::mock::MockStt))).await.unwrap();
        assert!(!slot.maybe_unload(120, 1_000 + 119_000).await);
        assert!(slot.maybe_unload(120, 1_000 + 120_000).await);
        assert!(!slot.is_loaded().await);
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p aleph-voice --no-default-features --lib lifecycle`
Expected: 4 passed。

- [ ] **Step 3: Commit**

```bash
git add aleph-voice/src/lifecycle.rs aleph-voice/src/lib.rs
git commit -m "voice: engine lifecycle — lazy slot with idle unload + exit decisions"
```

---

### Task 4: 音频模块 — 解码/重采样/编码

**Files:**
- Create: `aleph-voice/src/audio/mod.rs`
- Create: `aleph-voice/src/audio/ogg_opus.rs`
- Create: `aleph-voice/tests/fixtures/tone.mp3`（ffmpeg 生成，~2KB）
- Modify: `aleph-voice/src/lib.rs`（`pub mod audio;`）

- [ ] **Step 1: 生成 mp3 fixture**

Run: `mkdir -p aleph-voice/tests/fixtures && ffmpeg -y -f lavfi -i sine=frequency=440:duration=0.3 -ar 16000 -ac 1 -b:a 32k aleph-voice/tests/fixtures/tone.mp3 && ls -la aleph-voice/tests/fixtures/`
Expected: tone.mp3 约 1-3KB。无 ffmpeg → `brew install ffmpeg` 或先创建空文件并将对应测试标 `#[ignore]`（记入 commit message）。

- [ ] **Step 2: ogg_opus 编解码**

`aleph-voice/src/audio/ogg_opus.rs`：

```rust
//! Minimal OGG-Opus mux/demux. Encode targets Telegram voice notes
//! (mono, VoIP-tuned); decode covers inbound TG voice messages.

use anyhow::{bail, Context};

const OGG_OPUS_SERIAL: u32 = 0x416c6570; // "Alep"

/// Encode mono f32 PCM into an OGG-Opus byte stream.
/// `sample_rate` must be one of 8/12/16/24/48 kHz (Kokoro 24k, inbound 16k both fit).
pub fn encode(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let mut enc = opus::Encoder::new(sample_rate, opus::Channels::Mono, opus::Application::Voip)
        .context("create opus encoder")?;
    let pre_skip = enc.get_lookahead().unwrap_or(0) as u16;

    let mut out = Vec::new();
    {
        let mut writer = ogg::PacketWriter::new(&mut out);
        // OpusHead (RFC 7845 §5.1)
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&sample_rate.to_le_bytes()); // input rate (informational)
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family
        writer.write_packet(head, OGG_OPUS_SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;
        // OpusTags
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(11u32).to_le_bytes());
        tags.extend_from_slice(b"aleph-voice");
        tags.extend_from_slice(&0u32.to_le_bytes());
        writer.write_packet(tags, OGG_OPUS_SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;

        // 20 ms frames; granule position counts 48 kHz samples (RFC 7845 §4).
        let frame = (sample_rate / 50) as usize;
        let granule_per_frame = 960u64; // 20ms @ 48k
        let mut granule = u64::from(pre_skip);
        let mut buf = vec![0u8; 4000];
        let total_frames = samples.len().div_ceil(frame);
        for (i, chunk) in samples.chunks(frame).enumerate() {
            let mut padded;
            let input = if chunk.len() == frame {
                chunk
            } else {
                padded = chunk.to_vec();
                padded.resize(frame, 0.0);
                &padded[..]
            };
            let n = enc.encode_float(input, &mut buf).context("opus encode")?;
            granule += granule_per_frame;
            let end = if i + 1 == total_frames {
                ogg::PacketWriteEndInfo::EndStream
            } else {
                ogg::PacketWriteEndInfo::NormalPacket
            };
            writer.write_packet(buf[..n].to_vec(), OGG_OPUS_SERIAL, end, granule)?;
        }
    }
    Ok(out)
}

/// Decode an OGG-Opus stream to 16 kHz mono f32 PCM (opus decoder resamples natively).
pub fn decode_to_16k(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
    let mut dec = opus::Decoder::new(16_000, opus::Channels::Mono).context("create opus decoder")?;
    let mut pcm = Vec::new();
    let mut header_packets = 0u8;
    let mut buf = vec![0f32; 16_000]; // 1s max frame headroom
    while let Some(packet) = reader.read_packet()? {
        if header_packets < 2 {
            if header_packets == 0 && !packet.data.starts_with(b"OpusHead") {
                bail!("not an OGG-Opus stream");
            }
            header_packets += 1;
            continue;
        }
        let n = dec.decode_float(&packet.data, &mut buf, false).context("opus decode")?;
        pcm.extend_from_slice(&buf[..n]);
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_duration_and_energy() {
        let rate = 16_000u32;
        let samples: Vec<f32> = (0..rate) // 1s 440Hz
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect();
        let encoded = encode(&samples, rate).unwrap();
        assert!(encoded.starts_with(b"OggS"));
        let decoded = decode_to_16k(&encoded).unwrap();
        let dur = decoded.len() as f32 / 16_000.0;
        assert!((dur - 1.0).abs() < 0.15, "duration {dur}s drifted");
        let energy: f32 = decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32;
        assert!(energy > 0.01, "decoded audio is near-silent: {energy}");
    }

    #[test]
    fn rejects_non_opus() {
        assert!(decode_to_16k(b"OggS....garbage").is_err());
        assert!(decode_to_16k(b"plainly not ogg").is_err());
    }
}
```

- [ ] **Step 3: 解码入口 + 重采样 + wav 编码**

`aleph-voice/src/audio/mod.rs`：

```rust
//! Audio I/O: anything-in → 16 kHz mono f32 PCM; PCM → wav / ogg-opus out.
//!
//! Formats: wav/mp3/m4a-aac/flac via symphonia; ogg-opus + webm-opus via the
//! opus decoder (symphonia demuxes mkv/webm but has no opus codec).

pub mod ogg_opus;

use anyhow::{bail, Context};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::CODEC_TYPE_OPUS;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Decode arbitrary input audio to 16 kHz mono f32 PCM.
/// `name_hint` is a filename or mime used to seed format probing.
pub fn decode_to_pcm_mono_16k(bytes: &[u8], name_hint: &str) -> anyhow::Result<Vec<f32>> {
    if bytes.len() >= 36 && bytes.starts_with(b"OggS") && bytes[28..].windows(8).take(64).any(|w| w == b"OpusHead") {
        return ogg_opus::decode_to_16k(bytes);
    }
    decode_via_symphonia(bytes, name_hint)
}

fn decode_via_symphonia(bytes: &[u8], name_hint: &str) -> anyhow::Result<Vec<f32>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    let lower = name_hint.to_ascii_lowercase();
    for (needle, ext) in [
        ("wav", "wav"), ("mp3", "mp3"), ("m4a", "m4a"), ("mp4", "mp4"),
        ("aac", "aac"), ("flac", "flac"), ("webm", "webm"), ("ogg", "ogg"),
    ] {
        if lower.contains(needle) {
            hint.with_extension(ext);
            break;
        }
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .context("unrecognized audio container")?;
    let mut format = probed.format;
    let track = format.default_track().context("no audio track")?.clone();
    let src_rate = track.codec_params.sample_rate.unwrap_or(48_000);
    let channels = track.codec_params.channels.map_or(1, |c| c.count().max(1));

    // webm/mkv carrying opus: symphonia demuxes, opus crate decodes (to 16k directly).
    if track.codec_params.codec == CODEC_TYPE_OPUS {
        let mut dec = opus::Decoder::new(TARGET_SAMPLE_RATE, opus::Channels::Mono)?;
        let mut pcm = Vec::new();
        let mut buf = vec![0f32; 16_000];
        while let Ok(packet) = format.next_packet() {
            if packet.track_id() != track.id {
                continue;
            }
            if let Ok(n) = dec.decode_float(packet.buf(), &mut buf, false) {
                pcm.extend_from_slice(&buf[..n]);
            }
        }
        if pcm.is_empty() {
            bail!("opus stream decoded to zero samples");
        }
        return Ok(pcm);
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .context("unsupported audio codec")?;
    let mut mono: Vec<f32> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue, // tolerate trailing junk
        };
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sbuf.copy_interleaved_ref(decoded);
        let frames = sbuf.samples().chunks_exact(channels);
        mono.extend(frames.map(|f| f.iter().sum::<f32>() / channels as f32));
    }
    if mono.is_empty() {
        bail!("audio decoded to zero samples");
    }
    resample_to_16k(&mono, src_rate)
}

/// High-quality sinc resample to 16 kHz (no-op when already there).
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> anyhow::Result<Vec<f32>> {
    if src_rate == TARGET_SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 1024usize;
    let mut rs = SincFixedIn::<f32>::new(
        f64::from(TARGET_SAMPLE_RATE) / f64::from(src_rate),
        2.0,
        params,
        chunk,
        1,
    )?;
    let mut out = Vec::with_capacity(samples.len() * TARGET_SAMPLE_RATE as usize / src_rate as usize + chunk);
    let mut pos = 0usize;
    while pos < samples.len() {
        let end = (pos + chunk).min(samples.len());
        let mut block = samples[pos..end].to_vec();
        block.resize(chunk, 0.0);
        let processed = rs.process(&[block], None)?;
        out.extend_from_slice(&processed[0]);
        pos = end;
    }
    Ok(out)
}

/// Encode mono f32 PCM as 16-bit WAV.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec)?;
        for s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        w.finalize()?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn wav_roundtrip_through_decode_entry() {
        let pcm = sine(16_000, 0.5);
        let wav = encode_wav(&pcm, 16_000).unwrap();
        let back = decode_to_pcm_mono_16k(&wav, "x.wav").unwrap();
        assert!((back.len() as i64 - pcm.len() as i64).abs() < 32);
    }

    #[test]
    fn resamples_48k_wav_to_16k() {
        let pcm48 = sine(48_000, 0.5);
        let wav = encode_wav(&pcm48, 48_000).unwrap();
        let back = decode_to_pcm_mono_16k(&wav, "x.wav").unwrap();
        let dur = back.len() as f32 / 16_000.0;
        assert!((dur - 0.5).abs() < 0.05, "duration {dur}");
    }

    #[test]
    fn ogg_opus_input_routes_to_opus_decoder() {
        let pcm = sine(16_000, 0.4);
        let encoded = ogg_opus::encode(&pcm, 16_000).unwrap();
        let back = decode_to_pcm_mono_16k(&encoded, "voice.ogg").unwrap();
        assert!(back.len() > 16_000 / 4);
    }

    #[test]
    fn mp3_fixture_decodes() {
        let bytes = include_bytes!("../../tests/fixtures/tone.mp3");
        let pcm = decode_to_pcm_mono_16k(bytes, "tone.mp3").unwrap();
        assert!(pcm.len() > 1_000, "mp3 decoded {} samples", pcm.len());
    }

    #[test]
    fn garbage_input_errors() {
        assert!(decode_to_pcm_mono_16k(b"definitely not audio", "x.bin").is_err());
    }
}
```

- [ ] **Step 4: 跑测试（编译失败优先怀疑 symphonia/ogg/opus API 细节，按编译器指引修）**

Run: `cargo test -p aleph-voice --no-default-features --lib audio`
Expected: 7 passed（含 ogg_opus 2 个）。

- [ ] **Step 5: Commit**

```bash
git add aleph-voice/src/audio/ aleph-voice/tests/fixtures/ aleph-voice/src/lib.rs
git commit -m "voice: audio module — universal decode to 16k pcm, wav/ogg-opus encode"
```

### Task 5: ModelManager — 多源下载 / sha256 / 断点续传 / 解包

**Files:**
- Modify: `aleph-voice/src/models/mod.rs`（Task 1 只有 `pub mod manifest;`）

- [ ] **Step 1: 实现 + 测试**

`aleph-voice/src/models/mod.rs`：

```rust
//! Model package management: download (multi-source, resumable), verify
//! (pinned sha256), unpack (tar.bz2), and expose per-model state.

pub mod manifest;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use manifest::ModelSpec;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Externally visible state of one model package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    Missing,
    Downloading { percent: u8 },
    Unpacking,
    Ready,
    Error { message: String },
}

pub struct ModelManager {
    root: PathBuf,
    client: reqwest::Client,
    states: std::sync::Mutex<HashMap<String, ModelState>>,
    /// Per-model ensure() serialization so concurrent requests download once.
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: reqwest::Client::new(),
            states: std::sync::Mutex::new(HashMap::new()),
            locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Current state. Checks disk lazily: a present marker means Ready even
    /// after process restart (downloads survive restarts by design).
    pub fn state(&self, spec: &ModelSpec) -> ModelState {
        if let Some(s) = self.states.lock().unwrap_or_else(|e| e.into_inner()).get(spec.id) {
            return s.clone();
        }
        if self.dir(spec.id).join(spec.marker).exists() {
            ModelState::Ready
        } else {
            ModelState::Missing
        }
    }

    fn set_state(&self, id: &str, s: ModelState) {
        self.states.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), s);
    }

    /// Ensure the model is downloaded+unpacked. Safe to call concurrently.
    pub async fn ensure(&self, spec: &ModelSpec) -> anyhow::Result<PathBuf> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks.entry(spec.id.to_string()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
        };
        let _guard = lock.lock().await;

        let dir = self.dir(spec.id);
        if dir.join(spec.marker).exists() {
            self.set_state(spec.id, ModelState::Ready);
            return Ok(dir);
        }
        tokio::fs::create_dir_all(&self.root).await?;
        let part = self.root.join(format!("{}.tar.bz2.part", spec.id));

        let mut last_err = anyhow::anyhow!("no sources configured");
        for url in spec.urls {
            self.set_state(spec.id, ModelState::Downloading { percent: 0 });
            match self.download_resumable(url, &part, spec.id).await {
                Ok(()) => match verify_sha256(&part, spec.sha256).await {
                    Ok(()) => {
                        self.set_state(spec.id, ModelState::Unpacking);
                        let unpack_res = unpack_tar_bz2(&part, &dir, spec.marker).await;
                        let _ = tokio::fs::remove_file(&part).await;
                        match unpack_res {
                            Ok(()) => {
                                self.set_state(spec.id, ModelState::Ready);
                                return Ok(dir);
                            }
                            Err(e) => last_err = e.context(format!("unpack from {url}")),
                        }
                    }
                    Err(e) => {
                        // Corrupt download: drop the partial so the next source restarts clean.
                        let _ = tokio::fs::remove_file(&part).await;
                        last_err = e.context(format!("checksum from {url}"));
                    }
                },
                Err(e) => last_err = e.context(format!("download from {url}")),
            }
        }
        let message = format!("{last_err:#}");
        self.set_state(spec.id, ModelState::Error { message: message.clone() });
        bail!(message)
    }

    /// Download with HTTP Range resume into `dest`, updating percent state.
    async fn download_resumable(&self, url: &str, dest: &Path, id: &str) -> anyhow::Result<()> {
        let existing = tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0);
        let mut req = self.client.get(url);
        if existing > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        // Server ignored Range → restart from scratch.
        let mut written = if resumed { existing } else { 0 };
        let total = resp
            .content_length()
            .map(|l| l + if resumed { existing } else { 0 })
            .unwrap_or(0);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(resumed)
            .write(true)
            .truncate(!resumed)
            .open(dest)
            .await?;
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
            if total > 0 {
                let pct = ((written * 100) / total).min(99) as u8;
                self.set_state(id, ModelState::Downloading { percent: pct });
            }
        }
        file.flush().await?;
        Ok(())
    }
}

async fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let got = format!("{:x}", hasher.finalize());
        if got != expected.to_lowercase() {
            bail!("sha256 mismatch: got {got}, expected {expected}");
        }
        Ok(())
    })
    .await?
}

/// Unpack a .tar.bz2 into `dest`, stripping the single top-level directory
/// (sherpa packages all nest one root folder). Atomic via tmp dir + rename.
async fn unpack_tar_bz2(archive: &Path, dest: &Path, marker: &str) -> anyhow::Result<()> {
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    let marker = marker.to_string();
    tokio::task::spawn_blocking(move || {
        let tmp = dest.with_extension("unpack-tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let file = std::fs::File::open(&archive)?;
        let bz = bzip2::read::BzDecoder::new(file);
        tar::Archive::new(bz).unpack(&tmp)?;
        // strip-components=1: find the single root dir (or use tmp directly).
        let mut entries: Vec<_> = std::fs::read_dir(&tmp)?.filter_map(Result::ok).collect();
        let src_root = if entries.len() == 1 && entries[0].path().is_dir() {
            entries.remove(0).path()
        } else {
            tmp.clone()
        };
        if !src_root.join(&marker).exists() {
            bail!("unpacked archive missing marker file {marker}");
        }
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&src_root, &dest).context("move unpacked model into place")?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::get, Router};

    /// Build a tiny valid tar.bz2 containing `root/<marker>` and return (bytes, sha256).
    fn fixture_archive(marker: &str) -> (Vec<u8>, String) {
        let mut tar_bytes = Vec::new();
        {
            let enc = bzip2::write::BzEncoder::new(&mut tar_bytes, bzip2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let data = b"model-bytes";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, format!("pkg/{marker}"), &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let sha = format!("{:x}", Sha256::digest(&tar_bytes));
        (tar_bytes, sha)
    }

    /// Range-aware fixture server (serves `bytes` honoring Range headers).
    async fn serve(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        async fn handler(State(data): State<Arc<Vec<u8>>>, headers: HeaderMap) -> impl IntoResponse {
            let range = headers
                .get(reqwest::header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("bytes="))
                .and_then(|s| s.trim_end_matches('-').parse::<usize>().ok());
            match range {
                Some(start) if start < data.len() => (
                    axum::http::StatusCode::PARTIAL_CONTENT,
                    data[start..].to_vec(),
                )
                    .into_response(),
                _ => (axum::http::StatusCode::OK, data.as_ref().clone()).into_response(),
            }
        }
        let app = Router::new().route("/m.tar.bz2", get(handler)).with_state(Arc::new(bytes));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/m.tar.bz2"), h)
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    #[tokio::test]
    async fn downloads_verifies_unpacks() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (url, _h) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m1",
            urls: Box::leak(vec![leak(url)].into_boxed_slice()),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        let dir = mgr.ensure(&spec).await.unwrap();
        assert!(dir.join("model.onnx").exists());
        assert_eq!(mgr.state(&spec), ModelState::Ready);
    }

    #[tokio::test]
    async fn resumes_partial_download() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (url, _h) = serve(bytes.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        // Pre-write a truncated .part to simulate an interrupted download.
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("m2.tar.bz2.part"), &bytes[..bytes.len() / 2]).unwrap();
        let spec = ModelSpec {
            id: "m2",
            urls: Box::leak(vec![leak(url)].into_boxed_slice()),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        let dir = mgr.ensure(&spec).await.unwrap();
        assert!(dir.join("model.onnx").exists(), "resume must complete the file");
    }

    #[tokio::test]
    async fn checksum_mismatch_falls_to_next_source_then_errors() {
        let (bytes, _) = fixture_archive("model.onnx");
        let (url1, _h1) = serve(bytes.clone()).await;
        let (url2, _h2) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m3",
            urls: Box::leak(vec![leak(url1), leak(url2)].into_boxed_slice()),
            sha256: "deadbeef", // wrong on purpose
            marker: "model.onnx",
        };
        let err = mgr.ensure(&spec).await.unwrap_err();
        assert!(format!("{err:#}").contains("sha256 mismatch"));
        assert!(matches!(mgr.state(&spec), ModelState::Error { .. }));
    }

    #[tokio::test]
    async fn bad_source_falls_through_to_good_one() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (good, _h) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m4",
            urls: Box::leak(vec![leak("http://127.0.0.1:1/nope.tar.bz2".into()), leak(good)].into_boxed_slice()),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        mgr.ensure(&spec).await.unwrap();
        assert_eq!(mgr.state(&spec), ModelState::Ready);
    }
}
```

> `ModelSpec` 字段以 Task 1 提交的 manifest.rs 为准（id/urls/sha256/marker 四字段——若 Task 1 写了 `archive` 字段则删除之，本实现恒为 tarball）。

- [ ] **Step 2: 跑测试**

Run: `cargo test -p aleph-voice --no-default-features --lib models`
Expected: 4 passed。

- [ ] **Step 3: Commit**

```bash
git add aleph-voice/src/models/
git commit -m "voice: model manager — multi-source resumable download, sha256 pin, atomic unpack"
```

---

### Task 6: sherpa 引擎实现（feature = "sherpa"）

**前置**：Task 1 verdict 文档（真实 API 形状、`Send` 探针结论、默认 sid 与音色映射表）。

**Files:**
- Create: `aleph-voice/src/engine/sherpa.rs`

- [ ] **Step 1: 实现（以下按 sherpa-rs 0.6 假设编写；以 spike verdict 的真实 API 为准逐处修正）**

```rust
//! sherpa-onnx backed engines. The only file that touches sherpa-rs.
//!
//! API shapes were validated by the Tier-0 spike
//! (docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md) — keep them in sync.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;

use super::{SttEngine, SttResult, TtsAudio, TtsEngine};

/// Kokoro v1.1-zh voice name → speaker id, from the spike verdict table.
/// Unknown names fall back to the spike-chosen default sid with a warn.
const VOICE_TABLE: &[(&str, i32)] = &[
    // FILL FROM SPIKE VERDICT, e.g.: ("zf_001", 0), ("zm_010", 55), ("af_maple", 80),
];
const DEFAULT_SID: i32 = 0; // replace with spike-chosen default

fn resolve_voice(voice: &str) -> i32 {
    if let Ok(sid) = voice.parse::<i32>() {
        return sid;
    }
    VOICE_TABLE
        .iter()
        .find(|(name, _)| *name == voice)
        .map(|(_, sid)| *sid)
        .unwrap_or_else(|| {
            tracing::warn!(voice, "unknown voice name, using default sid");
            DEFAULT_SID
        })
}

/// SenseVoice STT. sherpa handles its own internal resampling from 16k input.
pub struct SherpaStt {
    // sherpa-rs methods take &mut self; the engine trait is &self → interior Mutex.
    inner: Mutex<sherpa_rs::sense_voice::SenseVoice>,
}

impl SherpaStt {
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        let p = |f: &str| model_dir.join(f).to_string_lossy().into_owned();
        let inner = sherpa_rs::sense_voice::SenseVoice::new(sherpa_rs::sense_voice::SenseVoiceConfig {
            model: p("model.int8.onnx"),
            tokens: p("tokens.txt"),
            language: "auto".into(),
            use_itn: true,
            ..Default::default()
        })
        .context("load SenseVoice")?;
        Ok(Self { inner: Mutex::new(inner) })
    }
}

impl SttEngine for SherpaStt {
    fn transcribe(&self, samples: &[f32], _language: Option<&str>) -> anyhow::Result<SttResult> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let text = guard.transcribe(crate::audio::TARGET_SAMPLE_RATE, samples.to_vec());
        Ok(SttResult { text, language: None })
    }
}

/// Kokoro multi-lang TTS.
pub struct SherpaTts {
    inner: Mutex<sherpa_rs::tts::KokoroTts>,
}

impl SherpaTts {
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        let p = |f: &str| model_dir.join(f).to_string_lossy().into_owned();
        let inner = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
            model: p("model.onnx"),
            voices: p("voices.bin"),
            tokens: p("tokens.txt"),
            data_dir: p("espeak-ng-data"),
            lexicon: format!("{},{}", p("lexicon-us-en.txt"), p("lexicon-zh.txt")),
            dict_dir: p("dict"),
            length_scale: 1.0,
            ..Default::default()
        });
        Ok(Self { inner: Mutex::new(inner) })
    }
}

impl TtsEngine for SherpaTts {
    fn synthesize(&self, text: &str, voice: &str, speed: f32) -> anyhow::Result<TtsAudio> {
        let sid = resolve_voice(voice);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let audio = guard.create(text, sid, speed).context("kokoro synthesize")?;
        Ok(TtsAudio { samples: audio.samples, sample_rate: audio.sample_rate })
    }
}

// Compile-time probe: trait requires Send+Sync; if sherpa types aren't, this
// file fails to compile and the spike-documented fallback applies (dedicated
// engine thread + channel). Mutex<T> is Sync iff T: Send.
#[allow(dead_code)]
fn _assert_engine_bounds() {
    fn is_engine<T: Send + Sync>() {}
    is_engine::<SherpaStt>();
    is_engine::<SherpaTts>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_resolution_table_and_fallbacks() {
        assert_eq!(resolve_voice("42"), 42);
        assert_eq!(resolve_voice("definitely-unknown"), DEFAULT_SID);
    }

    /// Real-model smoke test — run manually: cargo test -p aleph-voice -- --ignored
    #[test]
    #[ignore = "requires downloaded models under ~/.aleph/models/voice"]
    fn real_kokoro_and_sensevoice_roundtrip() {
        let root = dirs::home_dir().unwrap().join(".aleph/models/voice");
        let tts = SherpaTts::load(&root.join("kokoro-v1.1-zh")).unwrap();
        let audio = tts.synthesize("你好，本地语音。", "0", 1.0).unwrap();
        assert!(audio.samples.len() > 1_000);
        let pcm16k = crate::audio::resample_to_16k(&audio.samples, audio.sample_rate).unwrap();
        let stt = SherpaStt::load(&root.join("sense-voice-small")).unwrap();
        let result = stt.transcribe(&pcm16k, None).unwrap();
        assert!(result.text.contains("语音"), "got: {}", result.text);
    }
}
```

> VOICE_TABLE 与 DEFAULT_SID **必须**从 spike verdict 填实——提交前 `grep -n "FILL FROM SPIKE" aleph-voice/src/engine/sherpa.rs` 应为空。若 verdict 记录 sherpa 类型非 `Send`：把 `Mutex<引擎>` 换成专用线程 + `std::sync::mpsc` 请求/响应通道的 `EngineThread` 包装（trait 不变，改动局限本文件）。

- [ ] **Step 2: 编译 + 表测试 + 真模型 ignored 测试**

Run: `cargo test -p aleph-voice --lib engine::sherpa && cargo test -p aleph-voice --lib -- --ignored engine::sherpa`
Expected: 表测试 passed；ignored 真模型 roundtrip passed（模型已在 spike 下载）。

- [ ] **Step 3: Commit**

```bash
git add aleph-voice/src/engine/sherpa.rs
git commit -m "voice: sherpa engines — sensevoice stt + kokoro tts behind engine traits"
```

---

### Task 7: HTTP 服务 — auth + 四端点 + 下载/加载门控

**Files:**
- Create: `aleph-voice/src/server/mod.rs`
- Create: `aleph-voice/src/server/auth.rs`
- Create: `aleph-voice/src/server/handlers.rs`
- Modify: `aleph-voice/src/lib.rs`（`pub mod server;`）

- [ ] **Step 1: AppState 与 Router**

`aleph-voice/src/server/mod.rs`：

```rust
//! Loopback HTTP server: OpenAI-compatible STT/TTS endpoints + status/warmup.

pub mod auth;
pub mod handlers;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use crate::engine::{SttEngine, TtsEngine};
use crate::lifecycle::EngineSlot;
use crate::models::manifest::ModelSpec;
use crate::models::ModelManager;

/// Factory closures let tests inject mocks and main inject sherpa loads.
pub type SttFactory = Arc<dyn Fn() -> anyhow::Result<Arc<dyn SttEngine>> + Send + Sync>;
pub type TtsFactory = Arc<dyn Fn() -> anyhow::Result<Arc<dyn TtsEngine>> + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub token: String,
    pub models: Arc<ModelManager>,
    pub stt_spec: &'static ModelSpec,
    pub tts_spec: &'static ModelSpec,
    pub stt_slot: Arc<EngineSlot<dyn SttEngine>>,
    pub tts_slot: Arc<EngineSlot<dyn TtsEngine>>,
    pub stt_factory: SttFactory,
    pub tts_factory: TtsFactory,
    pub default_voice: String,
    /// Epoch ms of last request — feeds the deep-idle process exit.
    pub last_activity_ms: Arc<AtomicU64>,
    pub started_ms: u64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/audio/transcriptions", post(handlers::transcriptions))
        .route("/v1/audio/speech", post(handlers::speech))
        .route("/v1/voice/status", get(handlers::status))
        .route("/v1/voice/warmup", post(handlers::warmup))
        // axum's default body cap is 2 MB — voice files routinely exceed it.
        // 25 MB mirrors the existing whisper.rs MAX_AUDIO_BYTES ceiling.
        .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer))
        .with_state(state)
}
```

- [ ] **Step 2: auth 中间件**

`aleph-voice/src/server/auth.rs`：

```rust
//! Bearer-token gate. The token is minted per-spawn and handed to the
//! supervisor via the READY line — loopback-only defense in depth.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::AppState;

pub async fn require_bearer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == state.token);
    if !ok {
        return (StatusCode::UNAUTHORIZED, "invalid or missing bearer token").into_response();
    }
    state.last_activity_ms.store(crate::lifecycle::now_ms(), std::sync::atomic::Ordering::Relaxed);
    next.run(req).await
}
```

- [ ] **Step 3: 四个 handler**

`aleph-voice/src/server/handlers.rs`：

```rust
//! Endpoint handlers. Model gating: Missing/Error → kick ensure + 503;
//! Downloading/Unpacking → 503 with percent; Ready → serve (slot lazy-loads
//! the engine; concurrent loaders queue behind the slot mutex, 15 s cap).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Multipart, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::AppState;
use crate::models::manifest::ModelSpec;
use crate::models::ModelState;

const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Gate a request on model readiness. Err = ready-made 503 response.
fn gate(state: &AppState, spec: &'static ModelSpec) -> Result<(), Response> {
    match state.models.state(spec) {
        ModelState::Ready => Ok(()),
        ModelState::Downloading { percent } => Err(downloading_503(percent)),
        ModelState::Unpacking => Err(downloading_503(99)),
        ModelState::Missing | ModelState::Error { .. } => {
            let models = state.models.clone();
            tokio::spawn(async move {
                if let Err(e) = models.ensure(spec).await {
                    tracing::warn!(model = spec.id, error = %e, "model ensure failed");
                }
            });
            Err(downloading_503(0))
        }
    }
}

fn downloading_503(percent: u8) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"status": "downloading", "percent": percent})),
    )
        .into_response()
}

fn error_json(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": {"message": msg.into()}}))).into_response()
}

/// POST /v1/audio/transcriptions — OpenAI multipart compatible.
pub async fn transcriptions(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    if let Err(resp) = gate(&state, state.stt_spec) {
        return resp;
    }
    let mut file: Option<(Vec<u8>, String)> = None;
    let mut language: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "file" => {
                let name = field.file_name().unwrap_or("audio.bin").to_string();
                match field.bytes().await {
                    Ok(b) => file = Some((b.to_vec(), name)),
                    Err(e) => return error_json(StatusCode::BAD_REQUEST, format!("read file: {e}")),
                }
            }
            "language" => language = field.text().await.ok().filter(|s| !s.is_empty()),
            _ => {} // model / response_format accepted and ignored
        }
    }
    let Some((bytes, name)) = file else {
        return error_json(StatusCode::BAD_REQUEST, "missing 'file' field");
    };

    let pcm = match tokio::task::spawn_blocking(move || crate::audio::decode_to_pcm_mono_16k(&bytes, &name)).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return error_json(StatusCode::BAD_REQUEST, format!("decode audio: {e}")),
        Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("decode task: {e}")),
    };

    let factory = state.stt_factory.clone();
    let engine = match tokio::time::timeout(
        LOAD_TIMEOUT,
        state.stt_slot.get_or_load(crate::lifecycle::now_ms(), move || factory()),
    )
    .await
    {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load stt: {e}")),
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "loading"}))).into_response(),
    };

    let lang = language.clone();
    match tokio::task::spawn_blocking(move || engine.transcribe(&pcm, lang.as_deref())).await {
        Ok(Ok(r)) => Json(json!({"text": r.text, "language": r.language})).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("transcribe: {e}")),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("transcribe task: {e}")),
    }
}

#[derive(Deserialize)]
pub struct SpeechRequest {
    pub input: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub response_format: Option<String>,
    // `model` accepted and ignored — the sidecar runs what it's configured with.
    #[serde(default)]
    #[allow(dead_code)]
    pub model: Option<String>,
}

/// POST /v1/audio/speech — OpenAI JSON compatible. Formats: wav | opus.
pub async fn speech(State(state): State<AppState>, Json(req): Json<SpeechRequest>) -> Response {
    if let Err(resp) = gate(&state, state.tts_spec) {
        return resp;
    }
    if req.input.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "input is empty");
    }
    let format = req.response_format.as_deref().unwrap_or("opus");
    if !matches!(format, "wav" | "opus") {
        return error_json(StatusCode::BAD_REQUEST, format!("unsupported response_format '{format}' (wav|opus)"));
    }

    let factory = state.tts_factory.clone();
    let engine = match tokio::time::timeout(
        LOAD_TIMEOUT,
        state.tts_slot.get_or_load(crate::lifecycle::now_ms(), move || factory()),
    )
    .await
    {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("load tts: {e}")),
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "loading"}))).into_response(),
    };

    let voice = req.voice.clone().unwrap_or_else(|| state.default_voice.clone());
    let speed = req.speed.unwrap_or(1.0).clamp(0.25, 4.0);
    let text = req.input.clone();
    let fmt = format.to_string();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<u8>, &'static str)> {
        let audio = engine.synthesize(&text, &voice, speed)?;
        match fmt.as_str() {
            "wav" => Ok((crate::audio::encode_wav(&audio.samples, audio.sample_rate)?, "audio/wav")),
            _ => Ok((crate::audio::ogg_opus::encode(&audio.samples, audio.sample_rate)?, "audio/ogg")),
        }
    })
    .await;

    match result {
        Ok(Ok((bytes, content_type))) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("synthesize: {e}")),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("synthesize task: {e}")),
    }
}

/// GET /v1/voice/status — engine + model states for the supervisor/tool.
pub async fn status(State(state): State<AppState>) -> Response {
    let now = crate::lifecycle::now_ms();
    Json(json!({
        "stt": {
            "model": state.stt_spec.id,
            "model_state": state.models.state(state.stt_spec),
            "engine_loaded": state.stt_slot.is_loaded().await,
        },
        "tts": {
            "model": state.tts_spec.id,
            "model_state": state.models.state(state.tts_spec),
            "engine_loaded": state.tts_slot.is_loaded().await,
        },
        "uptime_secs": now.saturating_sub(state.started_ms) / 1000,
    }))
    .into_response()
}

/// POST /v1/voice/warmup — fire-and-forget: ensure models then load engines.
pub async fn warmup(State(state): State<AppState>) -> Response {
    let s = state.clone();
    tokio::spawn(async move {
        for (spec, which) in [(s.stt_spec, "stt"), (s.tts_spec, "tts")] {
            if let Err(e) = s.models.ensure(spec).await {
                tracing::warn!(model = spec.id, error = %e, "warmup ensure failed");
                return;
            }
            let now = crate::lifecycle::now_ms();
            let res = match which {
                "stt" => {
                    let f = s.stt_factory.clone();
                    s.stt_slot.get_or_load(now, move || f()).await.map(|_| ())
                }
                _ => {
                    let f = s.tts_factory.clone();
                    s.tts_slot.get_or_load(now, move || f()).await.map(|_| ())
                }
            };
            if let Err(e) = res {
                tracing::warn!(which, error = %e, "warmup engine load failed");
            }
        }
        tracing::info!("warmup complete");
    });
    (StatusCode::ACCEPTED, Json(json!({"started": true}))).into_response()
}
```

- [ ] **Step 4: 服务层测试（mock 引擎 + tower oneshot）**

附在 `aleph-voice/src/server/mod.rs` 尾部：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mock::{MockStt, MockTts};
    use crate::models::manifest::ModelSpec;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    static READY_SPEC: ModelSpec = ModelSpec { id: "ready-model", urls: &[], sha256: "0", marker: "marker.onnx" };
    static MISSING_SPEC: ModelSpec = ModelSpec {
        id: "missing-model",
        urls: &["http://127.0.0.1:1/x.tar.bz2"],
        sha256: "0",
        marker: "marker.onnx",
    };

    fn test_state(root: &std::path::Path, stt_spec: &'static ModelSpec, tts_spec: &'static ModelSpec) -> AppState {
        // Mark READY_SPEC present on disk.
        let d = root.join("ready-model");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("marker.onnx"), b"x").unwrap();
        AppState {
            token: "tok".into(),
            models: Arc::new(ModelManager::new(root.to_path_buf())),
            stt_spec,
            tts_spec,
            stt_slot: Arc::new(EngineSlot::new()),
            tts_slot: Arc::new(EngineSlot::new()),
            stt_factory: Arc::new(|| Ok(Arc::new(MockStt) as Arc<dyn SttEngine>)),
            tts_factory: Arc::new(|| Ok(Arc::new(MockTts) as Arc<dyn TtsEngine>)),
            default_voice: "zf_001".into(),
            last_activity_ms: Arc::new(AtomicU64::new(0)),
            started_ms: 0,
        }
    }

    fn authed(req: axum::http::request::Builder) -> axum::http::request::Builder {
        req.header(header::AUTHORIZATION, "Bearer tok")
    }

    #[tokio::test]
    async fn rejects_missing_or_bad_token() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        for auth in [None, Some("Bearer wrong")] {
            let mut req = Request::builder().uri("/v1/voice/status").method("GET");
            if let Some(a) = auth {
                req = req.header(header::AUTHORIZATION, a);
            }
            let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn transcribes_wav_via_mock() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let pcm: Vec<f32> = vec![0.1; 1600];
        let wav = crate::audio::encode_wav(&pcm, 16_000).unwrap();
        let boundary = "XBOUND";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = authed(Request::builder().uri("/v1/audio/transcriptions").method("POST"))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["text"].as_str().unwrap().contains("samples"));
    }

    #[tokio::test]
    async fn speech_emits_wav_and_opus() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        for (fmt, ct, magic) in [("wav", "audio/wav", b"RIFF".as_slice()), ("opus", "audio/ogg", b"OggS".as_slice())] {
            let req = authed(Request::builder().uri("/v1/audio/speech").method("POST"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"input":"你好","response_format":"{fmt}"}}"#)))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "fmt={fmt}");
            assert_eq!(resp.headers()[header::CONTENT_TYPE], ct);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert!(bytes.starts_with(magic), "fmt={fmt}");
        }
    }

    #[tokio::test]
    async fn rejects_unsupported_format() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/audio/speech").method("POST"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"input":"hi","response_format":"mp3"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_model_returns_503_downloading() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &MISSING_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/audio/transcriptions").method("POST"))
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=B")
            .body(Body::from("--B--\r\n"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "downloading");
    }

    #[tokio::test]
    async fn status_reports_states() {
        let tmp = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp.path(), &READY_SPEC, &READY_SPEC));
        let req = authed(Request::builder().uri("/v1/voice/status").method("GET"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["stt"]["model_state"]["state"], "ready");
        assert_eq!(v["stt"]["engine_loaded"], false);
    }
}
```

dev-dependencies 追加（`aleph-voice/Cargo.toml`）：

```toml
http-body-util = "0.1"
```

- [ ] **Step 5: 跑测试**

Run: `cargo test -p aleph-voice --no-default-features --lib server`
Expected: 6 passed。

- [ ] **Step 6: Commit**

```bash
git add aleph-voice/src/server/ aleph-voice/src/lib.rs aleph-voice/Cargo.toml
git commit -m "voice: http server — openai-compatible stt/tts endpoints with auth + model gating"
```

---

### Task 8: main.rs — serve 装配（READY 握手 / tick 循环 / 深闲自退）

**Files:**
- Modify: `aleph-voice/src/main.rs`（替换占位）

- [ ] **Step 1: 实现**

```rust
//! aleph-voice entry point. Spawned by aleph-server's VoiceSidecarSupervisor.
//!
//! Contract: bind 127.0.0.1:0, mint a per-spawn token, print exactly one
//! `READY {"v":1,"port":N,"token":"..."}` line to STDOUT (logs go to stderr),
//! then serve until deep-idle exit (exit code 0) or SIGTERM.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use clap::Parser;

use aleph_voice::engine::sherpa::{SherpaStt, SherpaTts};
use aleph_voice::engine::{SttEngine, TtsEngine};
use aleph_voice::lifecycle::{self, EngineSlot};
use aleph_voice::models::manifest;
use aleph_voice::models::ModelManager;
use aleph_voice::server::{router, AppState};

#[derive(Parser, Debug)]
#[command(name = "aleph-voice", version = env!("ALEPH_VERSION"))]
struct Args {
    /// Models root directory.
    #[arg(long)]
    models_dir: Option<std::path::PathBuf>,
    #[arg(long, default_value = "sense-voice-small")]
    stt_model: String,
    #[arg(long, default_value = "kokoro-v1.1-zh")]
    tts_model: String,
    #[arg(long, default_value = "zf_001")]
    tts_voice: String,
    #[arg(long, default_value_t = 600)]
    idle_unload_stt_secs: u64,
    #[arg(long, default_value_t = 120)]
    idle_unload_tts_secs: u64,
    #[arg(long, default_value_t = 1800)]
    idle_exit_secs: u64,
    /// auto | github | hf-mirror — reorders manifest sources.
    #[arg(long, default_value = "auto")]
    download_source: String,
}

fn main() -> anyhow::Result<()> {
    // Logs MUST go to stderr — stdout carries the READY handshake line.
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    ).init();

    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let models_dir = args.models_dir.clone().unwrap_or_else(|| {
        std::env::var_os("ALEPH_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".aleph")))
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("models/voice")
    });
    let stt_spec = manifest::spec_for(&args.stt_model)
        .ok_or_else(|| anyhow::anyhow!("unknown stt model '{}'", args.stt_model))?;
    let tts_spec = manifest::spec_for(&args.tts_model)
        .ok_or_else(|| anyhow::anyhow!("unknown tts model '{}'", args.tts_model))?;
    // NOTE: --download-source filtering keys off URL substrings; "auto" keeps
    // manifest order (github → hf-mirror). Filtering to an empty list falls
    // back to full order.
    // (Source reordering is cosmetic; ModelManager already falls through.)
    let _ = &args.download_source;

    let models = Arc::new(ModelManager::new(models_dir));
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());

    let stt_dir = models.dir(stt_spec.id);
    let tts_dir = models.dir(tts_spec.id);
    let state = AppState {
        token: token.clone(),
        models: models.clone(),
        stt_spec,
        tts_spec,
        stt_slot: Arc::new(EngineSlot::new()),
        tts_slot: Arc::new(EngineSlot::new()),
        stt_factory: Arc::new(move || Ok(Arc::new(SherpaStt::load(&stt_dir)?) as Arc<dyn SttEngine>)),
        tts_factory: Arc::new(move || Ok(Arc::new(SherpaTts::load(&tts_dir)?) as Arc<dyn TtsEngine>)),
        default_voice: args.tts_voice.clone(),
        last_activity_ms: Arc::new(AtomicU64::new(lifecycle::now_ms())),
        started_ms: lifecycle::now_ms(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    // Handshake: exactly one line on stdout, then flush.
    {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        writeln!(out, "READY {}", serde_json::json!({"v": 1, "port": port, "token": token}))?;
        out.flush()?;
    }
    tracing::info!(port, "aleph-voice listening");

    // Idle tick: unload idle engines; exit on deep idle.
    {
        let state = state.clone();
        let (stt_ttl, tts_ttl, exit_secs) =
            (args.idle_unload_stt_secs, args.idle_unload_tts_secs, args.idle_exit_secs);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                tick.tick().await;
                let now = lifecycle::now_ms();
                if state.tts_slot.maybe_unload(tts_ttl, now).await {
                    tracing::info!("tts engine unloaded (idle)");
                }
                if state.stt_slot.maybe_unload(stt_ttl, now).await {
                    tracing::info!("stt engine unloaded (idle)");
                }
                let last = state.last_activity_ms.load(Ordering::Relaxed);
                if lifecycle::should_exit(last, now, exit_secs) {
                    tracing::info!("deep idle ({exit_secs}s) — exiting to free all memory");
                    std::process::exit(0);
                }
            }
        });
    }

    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await?;
    Ok(())
}
```

- [ ] **Step 2: 手动冒烟（真引擎 + 真模型）**

```bash
cargo build -p aleph-voice --release
./target/release/aleph-voice --idle-exit-secs 120 &
# 复制 stdout 的 READY 行中的 port/token：
PORT=<port>; TOKEN=<token>
curl -s -X POST http://127.0.0.1:$PORT/v1/audio/speech \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"input":"你好，本地语音引擎冒烟测试。","response_format":"wav"}' -o /tmp/smoke.wav
open /tmp/smoke.wav
curl -s -X POST http://127.0.0.1:$PORT/v1/audio/transcriptions \
  -H "Authorization: Bearer $TOKEN" -F file=@/tmp/smoke.wav | python3 -m json.tool
curl -s http://127.0.0.1:$PORT/v1/voice/status -H "Authorization: Bearer $TOKEN" | python3 -m json.tool
```

Expected: wav 可播放、转写文本对得上、status 显示 `engine_loaded: true`。等 2 分钟后 status 显示 TTS `engine_loaded: false`（闲置卸载）。`kill %1` 收尾。

- [ ] **Step 3: Commit**

```bash
git add aleph-voice/src/main.rs
git commit -m "voice: serve entrypoint — ready handshake, idle tick loop, deep-idle exit"
```

### Task 9: core — `VoiceLocalConfig` + 加载期归一化（本地/云切换的单一来源）

切换语义（spec §4.6）：归一化**只填空**——`enabled=true` 且 default 未设时注入 `"local"`；用户显式配置（含云端）永远优先。云端 provider 配置零改动。

**Files:**
- Create: `src/config/types/voice_local.rs`
- Modify: `src/config/types/mod.rs`（模块导出，按该文件现有 `pub mod` 列表追加）
- Modify: `src/config/structs.rs`（`generation` 字段后加 `voice_local`）
- Modify: `src/config/load.rs`（解析成功处调归一化）

- [ ] **Step 1: 新类型 + 归一化纯函数 + 测试**

`src/config/types/voice_local.rs`：

```rust
//! Local voice sidecar configuration ([voice.local] in config.toml).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the aleph-voice local inference sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoiceLocalConfig {
    /// Master switch. Off by default — enabling injects a "local" provider
    /// into the generation provider maps at load time (fill-empty-only).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_stt_model")]
    pub stt_model: String,
    #[serde(default = "default_tts_model")]
    pub tts_model: String,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    /// TTS output container: "opus" (Telegram-native) or "wav".
    #[serde(default = "default_tts_format")]
    pub tts_format: String,
    #[serde(default = "default_idle_tts")]
    pub idle_unload_tts_secs: u64,
    #[serde(default = "default_idle_stt")]
    pub idle_unload_stt_secs: u64,
    #[serde(default = "default_idle_exit")]
    pub idle_exit_secs: u64,
    /// auto | github | hf-mirror.
    #[serde(default = "default_download_source")]
    pub download_source: String,
    /// Override the sidecar binary path (default: sibling of aleph-server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<PathBuf>,
}

fn default_stt_model() -> String { "sense-voice-small".into() }
fn default_tts_model() -> String { "kokoro-v1.1-zh".into() }
fn default_tts_voice() -> String { "zf_001".into() }
fn default_tts_format() -> String { "opus".into() }
const fn default_idle_tts() -> u64 { 120 }
const fn default_idle_stt() -> u64 { 600 }
const fn default_idle_exit() -> u64 { 1800 }
fn default_download_source() -> String { "auto".into() }

impl Default for VoiceLocalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stt_model: default_stt_model(),
            tts_model: default_tts_model(),
            tts_voice: default_tts_voice(),
            tts_format: default_tts_format(),
            idle_unload_tts_secs: default_idle_tts(),
            idle_unload_stt_secs: default_idle_stt(),
            idle_exit_secs: default_idle_exit(),
            download_source: default_download_source(),
            binary_path: None,
        }
    }
}

/// Provider name the sidecar registers under.
pub const LOCAL_PROVIDER_NAME: &str = "local";
/// `GenerationProviderConfig.provider_type` for the sidecar.
pub const LOCAL_PROVIDER_TYPE: &str = "local";

/// Load-time normalization: when local voice is enabled, inject a synthetic
/// "local" provider into the speech/transcription maps and point the unset
/// defaults at it. Fill-empty-only — explicit user config (cloud included)
/// always wins. Idempotent: safe across hot reloads.
pub fn normalize_voice_local(cfg: &mut crate::config::structs::Config) {
    if !cfg.voice_local.enabled {
        return;
    }
    use crate::generation::GenerationType;

    let synth = |cap: GenerationType, model: &str| {
        let mut p = crate::GenerationProviderConfig::new(LOCAL_PROVIDER_TYPE);
        // Placeholder key keeps existing api_key-presence walks selecting it;
        // the real per-spawn token is injected by the supervisor at call time.
        p.api_key = Some("local-sidecar".into());
        p.capabilities = vec![cap];
        p.models = vec![model.to_string()];
        p
    };

    cfg.generation
        .speech_providers
        .entry(LOCAL_PROVIDER_NAME.into())
        .or_insert_with(|| synth(GenerationType::Speech, &cfg.voice_local.tts_model.clone()));
    cfg.generation
        .transcription_providers
        .entry(LOCAL_PROVIDER_NAME.into())
        .or_insert_with(|| synth(GenerationType::Transcription, &cfg.voice_local.stt_model.clone()));

    if cfg.generation.default_speech_provider.is_none() {
        cfg.generation.default_speech_provider = Some(LOCAL_PROVIDER_NAME.into());
    }
    if cfg.generation.default_transcription_provider.is_none() {
        cfg.generation.default_transcription_provider = Some(LOCAL_PROVIDER_NAME.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::structs::Config;

    #[test]
    fn disabled_is_a_noop() {
        let mut cfg = Config::default();
        normalize_voice_local(&mut cfg);
        assert!(cfg.generation.speech_providers.is_empty());
        assert!(cfg.generation.default_speech_provider.is_none());
    }

    #[test]
    fn enabled_fills_empty_defaults_and_entries() {
        let mut cfg = Config::default();
        cfg.voice_local.enabled = true;
        normalize_voice_local(&mut cfg);
        assert_eq!(cfg.generation.default_speech_provider.as_deref(), Some("local"));
        assert_eq!(cfg.generation.default_transcription_provider.as_deref(), Some("local"));
        let p = &cfg.generation.speech_providers["local"];
        assert_eq!(p.provider_type, "local");
        assert_eq!(p.models, vec!["kokoro-v1.1-zh".to_string()]);
        // Validation must accept the synthetic reference (spec: validate passes).
        cfg.generation.validate().unwrap();
    }

    #[test]
    fn explicit_cloud_defaults_win() {
        let mut cfg = Config::default();
        cfg.voice_local.enabled = true;
        cfg.generation.default_speech_provider = Some("openai_tts".into());
        cfg.generation
            .speech_providers
            .insert("openai_tts".into(), crate::GenerationProviderConfig::new("openai_tts"));
        normalize_voice_local(&mut cfg);
        // Cloud default untouched — switching back to cloud is pure config.
        assert_eq!(cfg.generation.default_speech_provider.as_deref(), Some("openai_tts"));
        // Local entry still registered (per-channel override can still pick it).
        assert!(cfg.generation.speech_providers.contains_key("local"));
        // Transcription default was unset → filled with local.
        assert_eq!(cfg.generation.default_transcription_provider.as_deref(), Some("local"));
    }

    #[test]
    fn idempotent_and_preserves_user_local_entry() {
        let mut cfg = Config::default();
        cfg.voice_local.enabled = true;
        normalize_voice_local(&mut cfg);
        let mut user_entry = cfg.generation.speech_providers["local"].clone();
        user_entry.models = vec!["custom".into()];
        cfg.generation.speech_providers.insert("local".into(), user_entry);
        normalize_voice_local(&mut cfg);
        assert_eq!(cfg.generation.speech_providers["local"].models, vec!["custom".to_string()]);
    }

    #[test]
    fn toml_section_parses() {
        let toml = r#"
            [voice.local]
            enabled = true
            tts_voice = "zf_088"
            idle_exit_secs = 900
        "#;
        #[derive(serde::Deserialize)]
        struct Wrap { voice: Voice }
        #[derive(serde::Deserialize)]
        struct Voice { local: VoiceLocalConfig }
        let w: Wrap = ::toml::from_str(toml).unwrap();
        assert!(w.voice.local.enabled);
        assert_eq!(w.voice.local.tts_voice, "zf_088");
        assert_eq!(w.voice.local.idle_exit_secs, 900);
        assert_eq!(w.voice.local.idle_unload_tts_secs, 120);
    }
}
```

> TOML 路径说明：`[voice.local]` 要求 Config 上字段名为 `voice`、内含 `local`。为保持顶层字段语义清晰，**采用 `#[serde(rename = "voice")]` 包一层**：见 Step 2。

- [ ] **Step 2: Config 接字段**

`src/config/structs.rs`（`pub generation: GenerationConfig,` 之后插入）：

```rust
    /// Local voice sidecar ([voice.local]) — aleph-voice STT/TTS.
    #[serde(default, rename = "voice")]
    pub voice_local: VoiceSection,
```

同文件（或 voice_local.rs 内）加包装节并 re-export：

```rust
/// `[voice]` config section wrapper (currently only `local`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct VoiceSection {
    #[serde(default)]
    pub local: VoiceLocalConfig,
}
```

> 将 `VoiceSection` 定义放进 `src/config/types/voice_local.rs`，structs.rs 通过现有 import 风格引入（看文件头部其他 types 的 use 写法照搬）。**注意**：上文归一化函数与测试中的 `cfg.voice_local.enabled` 相应为 `cfg.voice_local.local.enabled`——统一改为经 `impl Config { pub fn local_voice(&self) -> &VoiceLocalConfig { &self.voice_local.local } }` 访问以免到处 `.local.`；归一化函数体内同步调整。
> `Config` 若有手写 `Default` 或字面量构造点：`grep -rn "Config {" src/ --include="*.rs" | grep -v "//"` 逐个补 `voice_local: Default::default(),`（编译器会逐一指出，**勿漏 tests/**）。

- [ ] **Step 3: load.rs 接归一化**

定位：`grep -n "pub fn load\|fn finalize\|-> .*Result<Config" src/config/load.rs | head`。在**每个**返回成功解析 `Config` 的出口（通常 `load()` 与 `load_from_path()`/`from_str` 收口处、validate 之前）插入：

```rust
crate::config::types::voice_local::normalize_voice_local(&mut config);
```

验证 hot-reload 同样覆盖：`grep -rn "Config::load" src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs src/bin/aleph-server/commands/start/builder/subsystems.rs` —— 若 reload 路径直接 `Config::load()`（subsystems.rs:244 是），归一化自动生效，无需另改；若 generation_init 的 panel hot-reload 用内存中 Arc 的 config 重建（读 :56-120 确认），同样已是归一化后的实例。

- [ ] **Step 4: 跑测试 + 全量检查**

Run: `cargo test -p alephcore --lib voice_local && cargo check -p alephcore`
Expected: 5 passed；check 无错（字面量构造点全补齐）。

- [ ] **Step 5: Commit**

```bash
git add src/config/
git commit -m "config: [voice.local] section + load-time local provider normalization"
```

---

### Task 10: core — `VoiceSidecarSupervisor`（懒 spawn / READY 握手 / 崩溃环路守卫）

**Files:**
- Create: `src/gateway/voice/sidecar.rs`
- Modify: `src/gateway/voice/mod.rs`（`pub mod sidecar;`，按现有列表追加）
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs`（boot 早期 init_global）

- [ ] **Step 1: 实现 + 测试**

`src/gateway/voice/sidecar.rs`：

```rust
//! aleph-voice sidecar supervisor: lazy spawn, READY handshake, crash-loop guard.
//!
//! Process-global singleton (OnceLock) mirroring the SwiftBridge precedent —
//! STT (media processor / inbound router) and TTS (reply emitter) paths share
//! one child process. No eager start: first voice demand spawns it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde::Deserialize;
use tokio::io::AsyncBufReadExt;

use crate::config::types::voice_local::VoiceLocalConfig;
use crate::sync_primitives::Arc;

/// Resolved connection info for one sidecar incarnation.
#[derive(Debug, Clone)]
pub struct SidecarEndpoint {
    /// e.g. "http://127.0.0.1:54321/v1" — joins the existing OpenAI-compat
    /// client code, which appends "/audio/transcriptions" etc.
    pub base_url: String,
    /// Per-spawn bearer token (used as the provider api_key).
    pub token: String,
}

/// Remote model state subset we care about (mirrors the sidecar status DTO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteModelState {
    Ready,
    Downloading { percent: u8 },
    Other(String),
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const CRASH_LIMIT: usize = 3;
const COOLDOWN: Duration = Duration::from_secs(300);

/// Pure decision: are we in a crash loop? (>= CRASH_LIMIT crashes inside CRASH_WINDOW)
pub fn crash_loop_active(crashes: &VecDeque<Instant>, now: Instant) -> bool {
    crashes
        .iter()
        .filter(|t| now.duration_since(**t) <= CRASH_WINDOW)
        .count()
        >= CRASH_LIMIT
}

struct Inner {
    child: Option<tokio::process::Child>,
    endpoint: Option<SidecarEndpoint>,
    crashes: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
}

pub struct VoiceSidecarSupervisor {
    cfg: VoiceLocalConfig,
    inner: tokio::sync::Mutex<Inner>,
    handshake_timeout: Duration,
}

static GLOBAL: std::sync::OnceLock<Arc<VoiceSidecarSupervisor>> = std::sync::OnceLock::new();

/// Install the global supervisor at boot (no-op if already installed).
pub fn init_global(cfg: VoiceLocalConfig) -> Arc<VoiceSidecarSupervisor> {
    GLOBAL.get_or_init(|| Arc::new(VoiceSidecarSupervisor::new(cfg))).clone()
}

/// The global supervisor, if local voice was enabled at boot.
pub fn global() -> Option<Arc<VoiceSidecarSupervisor>> {
    GLOBAL.get().cloned()
}

impl VoiceSidecarSupervisor {
    pub fn new(cfg: VoiceLocalConfig) -> Self {
        Self {
            cfg,
            inner: tokio::sync::Mutex::new(Inner {
                child: None,
                endpoint: None,
                crashes: VecDeque::new(),
                cooldown_until: None,
            }),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn with_handshake_timeout(mut self, t: Duration) -> Self {
        self.handshake_timeout = t;
        self
    }

    pub fn config(&self) -> &VoiceLocalConfig {
        &self.cfg
    }

    /// Endpoint if the sidecar is currently alive — never spawns.
    pub async fn peek_endpoint(&self) -> Option<SidecarEndpoint> {
        let mut inner = self.inner.lock().await;
        if Self::child_alive(&mut inner) {
            inner.endpoint.clone()
        } else {
            None
        }
    }

    /// Get a live endpoint, spawning the sidecar if needed.
    pub async fn ensure_endpoint(&self) -> anyhow::Result<SidecarEndpoint> {
        let mut inner = self.inner.lock().await;
        if Self::child_alive(&mut inner) {
            if let Some(ep) = inner.endpoint.clone() {
                return Ok(ep);
            }
        }
        let now = Instant::now();
        if let Some(until) = inner.cooldown_until {
            if now < until {
                bail!(
                    "local voice sidecar in crash-loop cooldown ({}s left)",
                    (until - now).as_secs()
                );
            }
            inner.cooldown_until = None;
        }
        if crash_loop_active(&inner.crashes, now) {
            inner.cooldown_until = Some(now + COOLDOWN);
            bail!("local voice sidecar crash loop detected — cooling down {}s", COOLDOWN.as_secs());
        }

        let bin = self.binary_path()?;
        tracing::info!(bin = %bin.display(), "spawning aleph-voice sidecar");
        let mut child = tokio::process::Command::new(&bin)
            .arg("--stt-model").arg(&self.cfg.stt_model)
            .arg("--tts-model").arg(&self.cfg.tts_model)
            .arg("--tts-voice").arg(&self.cfg.tts_voice)
            .arg("--idle-unload-stt-secs").arg(self.cfg.idle_unload_stt_secs.to_string())
            .arg("--idle-unload-tts-secs").arg(self.cfg.idle_unload_tts_secs.to_string())
            .arg("--idle-exit-secs").arg(self.cfg.idle_exit_secs.to_string())
            .arg("--download-source").arg(&self.cfg.download_source)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {}", bin.display()))?;

        let stdout = child.stdout.take().context("sidecar stdout unavailable")?;
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(self.handshake_timeout, async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json) = line.strip_prefix("READY ") {
                    return Some(json.to_string());
                }
            }
            None
        })
        .await;

        let endpoint = match ready {
            Ok(Some(json)) => {
                #[derive(Deserialize)]
                struct Ready { port: u16, token: String }
                let r: Ready = serde_json::from_str(&json).context("parse READY line")?;
                SidecarEndpoint {
                    base_url: format!("http://127.0.0.1:{}/v1", r.port),
                    token: r.token,
                }
            }
            Ok(None) | Err(_) => {
                let _ = child.start_kill();
                inner.crashes.push_back(now);
                while inner.crashes.len() > 8 {
                    inner.crashes.pop_front();
                }
                bail!("sidecar did not print READY within {:?}", self.handshake_timeout);
            }
        };

        // Drain remaining stdout so the pipe never blocks the child.
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        inner.child = Some(child);
        inner.endpoint = Some(endpoint.clone());
        Ok(endpoint)
    }

    /// try_wait-based liveness; records non-zero exits as crashes.
    fn child_alive(inner: &mut Inner) -> bool {
        let Some(child) = inner.child.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    inner.crashes.push_back(Instant::now());
                    while inner.crashes.len() > 8 {
                        inner.crashes.pop_front();
                    }
                    tracing::warn!(%status, "aleph-voice exited abnormally");
                } else {
                    tracing::info!("aleph-voice exited cleanly (deep idle)");
                }
                inner.child = None;
                inner.endpoint = None;
                false
            }
            Err(_) => false,
        }
    }

    fn binary_path(&self) -> anyhow::Result<std::path::PathBuf> {
        if let Some(ref p) = self.cfg.binary_path {
            if p.exists() {
                return Ok(p.clone());
            }
            bail!("voice.local.binary_path does not exist: {}", p.display());
        }
        let exe = std::env::current_exe().context("current_exe")?;
        let dir = exe.parent().context("exe parent dir")?;
        let candidate = dir.join(format!("aleph-voice{}", std::env::consts::EXE_SUFFIX));
        if candidate.exists() {
            return Ok(candidate);
        }
        bail!(
            "aleph-voice binary not found next to aleph-server ({}); set voice.local.binary_path",
            candidate.display()
        )
    }

    /// Fire warmup: ensure running + POST /voice/warmup.
    pub async fn warmup(&self) -> anyhow::Result<()> {
        let ep = self.ensure_endpoint().await?;
        let client = reqwest::Client::new();
        client
            .post(format!("{}/voice/warmup", ep.base_url))
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(5))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// TTS model state via /voice/status (preflight for the downloading case).
    pub async fn tts_model_state(&self) -> anyhow::Result<RemoteModelState> {
        let ep = self.ensure_endpoint().await?;
        let client = reqwest::Client::new();
        let v: serde_json::Value = client
            .get(format!("{}/voice/status", ep.base_url))
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_model_state(&v["tts"]["model_state"]))
    }
}

/// Parse the sidecar's tagged ModelState JSON into our subset.
pub fn parse_model_state(v: &serde_json::Value) -> RemoteModelState {
    match v["state"].as_str() {
        Some("ready") => RemoteModelState::Ready,
        Some("downloading") => RemoteModelState::Downloading {
            percent: v["percent"].as_u64().unwrap_or(0) as u8,
        },
        Some(other) => RemoteModelState::Other(other.to_string()),
        None => RemoteModelState::Other("unknown".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fake_sidecar_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-voice.sh");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\n{body}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (dir, path)
    }

    fn cfg_with_bin(path: std::path::PathBuf) -> VoiceLocalConfig {
        VoiceLocalConfig { enabled: true, binary_path: Some(path), ..Default::default() }
    }

    #[test]
    fn crash_loop_decision() {
        let now = Instant::now();
        let recent: VecDeque<Instant> = (0..3).map(|_| now).collect();
        assert!(crash_loop_active(&recent, now));
        let stale: VecDeque<Instant> = (0..3).map(|_| now - Duration::from_secs(120)).collect();
        assert!(!crash_loop_active(&stale, now));
        let two: VecDeque<Instant> = (0..2).map(|_| now).collect();
        assert!(!crash_loop_active(&two, now));
    }

    #[test]
    fn parses_model_states() {
        let v: serde_json::Value = serde_json::json!({"state": "downloading", "percent": 42});
        assert_eq!(parse_model_state(&v), RemoteModelState::Downloading { percent: 42 });
        let v: serde_json::Value = serde_json::json!({"state": "ready"});
        assert_eq!(parse_model_state(&v), RemoteModelState::Ready);
    }

    #[tokio::test]
    async fn handshake_parses_ready_and_reuses_endpoint() {
        let (_d, path) =
            fake_sidecar_script(r#"echo 'READY {"v":1,"port":59999,"token":"tok123"}'; sleep 30"#);
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path));
        let ep = sup.ensure_endpoint().await.unwrap();
        assert_eq!(ep.base_url, "http://127.0.0.1:59999/v1");
        assert_eq!(ep.token, "tok123");
        // Second call reuses the live child (no respawn → same endpoint).
        let ep2 = sup.ensure_endpoint().await.unwrap();
        assert_eq!(ep2.token, "tok123");
        assert!(sup.peek_endpoint().await.is_some());
    }

    #[tokio::test]
    async fn no_ready_line_times_out_and_records_crash() {
        let (_d, path) = fake_sidecar_script("sleep 30");
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path))
            .with_handshake_timeout(Duration::from_millis(200));
        let err = sup.ensure_endpoint().await.unwrap_err();
        assert!(format!("{err:#}").contains("READY"));
        assert!(sup.peek_endpoint().await.is_none());
    }

    #[tokio::test]
    async fn crash_loop_triggers_cooldown() {
        let (_d, path) = fake_sidecar_script("exit 1");
        let sup = VoiceSidecarSupervisor::new(cfg_with_bin(path))
            .with_handshake_timeout(Duration::from_millis(150));
        for _ in 0..3 {
            let _ = sup.ensure_endpoint().await;
        }
        let err = sup.ensure_endpoint().await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("crash loop") || msg.contains("cooldown"), "got: {msg}");
    }

    #[tokio::test]
    async fn missing_binary_yields_actionable_error() {
        let cfg = VoiceLocalConfig {
            enabled: true,
            binary_path: Some("/nonexistent/aleph-voice".into()),
            ..Default::default()
        };
        let err = VoiceSidecarSupervisor::new(cfg).ensure_endpoint().await.unwrap_err();
        assert!(format!("{err:#}").contains("binary_path"));
    }
}
```

> 测试中 `живой` 为笔误示意——写成英文 `live`。`crate::sync_primitives::Arc` 是仓库惯例（见 voice_mode_set.rs:13）；若该模块仅 re-export `std::sync::Arc`，直接沿用。

- [ ] **Step 2: boot 接线 init_global**

`src/bin/aleph-server/commands/start/builder/subsystems.rs`，在 STT 接线块（:772-786）**之前**加：

```rust
    // Local voice sidecar supervisor — installed only when enabled; the
    // sidecar itself is spawned lazily on first voice demand.
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        if cfg.local_voice().enabled {
            alephcore::gateway::voice::sidecar::init_global(cfg.local_voice().clone());
            if !daemon {
                println!("  Local voice: sidecar supervisor armed (lazy spawn)");
            }
        }
    }
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p alephcore --lib sidecar`
Expected: 6 passed（5 单测 + parse）。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/voice/sidecar.rs src/gateway/voice/mod.rs src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "gateway: voice sidecar supervisor — lazy spawn, ready handshake, crash-loop guard"
```

---

### Task 11: core — Local provider 双包装 + `SttSource` 晚绑定三站改造

**Files:**
- Create: `src/gateway/voice/local_provider.rs`
- Modify: `src/gateway/voice/mod.rs`
- Modify: `src/generation/providers/factory.rs`（`"local"` arm）
- Modify: `src/gateway/voice/inbound.rs`（`resolve_stt_config` → `resolve_stt_source`）
- Modify: `src/gateway/inbound_router/mod.rs:116,279-281,587-598`
- Modify: `src/gateway/handlers/voice.rs:75` 附近
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:776-786`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1113-1149`

- [ ] **Step 1: 双包装实现**

`src/gateway/voice/local_provider.rs`：

```rust
//! Thin providers that bridge core's existing voice seams to the aleph-voice
//! sidecar. Ports are dynamic per spawn, so these resolve (base_url, token)
//! from the supervisor at call time instead of static config.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::generation::{
    GenerationData, GenerationError, GenerationOutput, GenerationProvider, GenerationRequest,
    GenerationResult, GenerationType,
};
use crate::media::cache::CachedMedia;
use crate::media::transcription::{TranscriptionResult, TranscriptionService};

use super::sidecar;

/// TranscriptionService backed by the sidecar (MediaProcessor path).
pub struct LocalTranscription;

#[async_trait]
impl TranscriptionService for LocalTranscription {
    async fn transcribe(
        &self,
        audio: &CachedMedia,
        language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult> {
        let sup = sidecar::global()
            .ok_or_else(|| anyhow::anyhow!("local voice not initialized (voice.local.enabled?)"))?;
        let ep = sup.ensure_endpoint().await?;
        let bytes = tokio::fs::read(&audio.local_path).await?;
        let filename = audio
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.bin")
            .to_string();
        // Reuse the shared whisper-dialect HTTP core (same multipart shape).
        let cfg = super::inbound::SttConfig {
            api_key: ep.token,
            base_url: ep.base_url,
            model: sup.config().stt_model.clone(),
        };
        let text = super::inbound::transcribe_bytes(bytes, &filename, &audio.mime_type, language, &cfg)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(TranscriptionResult { text, language: None })
    }
}

/// GenerationProvider backed by the sidecar (TTS path through the registry).
pub struct LocalVoiceProvider {
    capability: GenerationType,
}

impl LocalVoiceProvider {
    pub const fn new(capability: GenerationType) -> Self {
        Self { capability }
    }

    async fn tts(&self, request: GenerationRequest) -> GenerationResult<GenerationOutput> {
        let sup = sidecar::global().ok_or_else(|| {
            GenerationError::provider_error("local", "local voice not initialized")
        })?;
        let ep = sup
            .ensure_endpoint()
            .await
            .map_err(|e| GenerationError::provider_error("local", format!("{e:#}")))?;
        let cfg = sup.config();
        let voice = request
            .params
            .voice
            .clone()
            .unwrap_or_else(|| cfg.tts_voice.clone());
        let body = serde_json::json!({
            "model": cfg.tts_model,
            "input": request.prompt,
            "voice": voice,
            "response_format": cfg.tts_format,
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/audio/speech", ep.base_url))
            .bearer_auth(&ep.token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| GenerationError::provider_error("local", format!("request: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            let v: serde_json::Value = resp.json().await.unwrap_or_default();
            let pct = v["percent"].as_u64().unwrap_or(0);
            return Err(GenerationError::provider_error(
                "local",
                format!("model downloading ({pct}%)"),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GenerationError::provider_error("local", format!("HTTP {status}: {body}")));
        }
        let content_type = match cfg.tts_format.as_str() {
            "wav" => "audio/wav",
            _ => "audio/ogg",
        };
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| GenerationError::provider_error("local", format!("body: {e}")))?;
        let mut output = GenerationOutput::new(GenerationType::Speech, GenerationData::Bytes(bytes.to_vec()));
        output.metadata.content_type = Some(content_type.to_string());
        Ok(output)
    }
}

impl GenerationProvider for LocalVoiceProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            match request.generation_type {
                GenerationType::Speech => self.tts(request).await,
                other => Err(GenerationError::unsupported_feature(
                    "only speech is served by the local sidecar provider",
                    &format!("{other:?}"),
                    "local",
                )),
            }
        })
    }

    fn name(&self) -> &str {
        "local"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![self.capability]
    }
}
```

> **签名核对步**（执行时必做，5 分钟）：打开 `src/generation/error.rs` 与 `src/generation/types.rs`、`src/generation/providers/openai_tts/mod.rs`——
> ① `GenerationError::provider_error(..)` / `unsupported_feature(..)` 构造名与参数序以真实定义为准（factory 与 openai_tts 内有现成用法可照搬）；
> ② `GenerationRequest.prompt`/`params.voice`、`GenerationOutput::new` + `metadata.content_type` 字段路径以 types.rs 为准（outbound.rs:110-116 已证明 `output.data`/`output.metadata.content_type` 存在）。

- [ ] **Step 2: factory `"local"` arm**

`src/generation/providers/factory.rs` 的 `create_provider` match（与 `"openai_tts"` 等并列处）加：

```rust
        // Local voice sidecar (aleph-voice). Endpoint is dynamic per spawn —
        // the provider resolves it from the supervisor at call time.
        "local" => Ok(std::sync::Arc::new(
            crate::gateway::voice::local_provider::LocalVoiceProvider::new(gen_type),
        )),
```

确认该 match 末尾的 capability 校验（factory.rs:274-278 附近）对 `gen_type` ∈ {Speech, Transcription} 均放行（`LocalVoiceProvider::supported_types` 返回构造时的 capability，天然一致）。

- [ ] **Step 3: `SttSource` 晚绑定（inbound.rs）**

`src/gateway/voice/inbound.rs`：将 `resolve_stt_config` 改造为（保留 `SttConfig` 与 `transcribe_bytes` 原样）：

```rust
/// STT unavailability, typed so callers can react without string-matching.
#[derive(Debug)]
pub enum SttUnavailable {
    /// Local sidecar still downloading models.
    Downloading { percent: u8 },
    Error(String),
}

/// Late-bound STT source. Local resolves (port, token) per message because the
/// sidecar port changes per spawn; Static is the existing cloud behavior.
#[derive(Clone)]
pub enum SttSource {
    Static(SttConfig),
    /// Local sidecar, with an optional pre-resolved cloud fallback (spec §4.6:
    /// local 故障/下载中自动回退云；云为 default 时不反向回退).
    Local { fallback: Option<Box<SttConfig>> },
}

impl SttSource {
    /// Materialize a concrete SttConfig, consulting the sidecar when local.
    pub async fn materialize(&self) -> Result<SttConfig, SttUnavailable> {
        match self {
            Self::Static(cfg) => Ok(cfg.clone()),
            Self::Local { fallback } => {
                match Self::local_config().await {
                    Ok(cfg) => Ok(cfg),
                    Err(unavailable) => match fallback {
                        Some(cloud) => {
                            tracing::warn!(
                                reason = ?unavailable,
                                "local STT unavailable — falling back to cloud provider"
                            );
                            Ok((**cloud).clone())
                        }
                        None => Err(unavailable),
                    },
                }
            }
        }
    }

    async fn local_config() -> Result<SttConfig, SttUnavailable> {
        let sup = super::sidecar::global()
            .ok_or_else(|| SttUnavailable::Error("local voice not initialized".into()))?;
        let ep = sup
            .ensure_endpoint()
            .await
            .map_err(|e| SttUnavailable::Error(format!("{e:#}")))?;
        Ok(SttConfig {
            api_key: ep.token,
            base_url: ep.base_url,
            model: sup.config().stt_model.clone(),
        })
    }
}

/// Resolve the active STT source from generation config + vault.
/// Local (provider_type == "local") wins per the normalized defaults; a cloud
/// candidate (if any) rides along as the fallback. Pure cloud setups produce
/// `Static` exactly as before.
pub fn resolve_stt_source(
    gen_cfg: &crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
) -> Option<SttSource> {
    let chosen = choose_transcription_provider(gen_cfg, vault, false)?;
    if chosen.1.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE {
        let fallback = choose_transcription_provider(gen_cfg, vault, true)
            .map(|(key, pcfg)| Box::new(static_stt_config(key, pcfg)));
        Some(SttSource::Local { fallback })
    } else {
        let (key, pcfg) = chosen;
        Some(SttSource::Static(static_stt_config(key, pcfg)))
    }
}
```

实现私有助手（把原 `resolve_stt_config` 函数体拆为两半，逻辑零变化）：

```rust
/// Selection walk shared by primary + fallback resolution.
/// `skip_local = true` excludes provider_type == "local" entries.
fn choose_transcription_provider<'a>(
    gen_cfg: &'a crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
    skip_local: bool,
) -> Option<(String, &'a crate::GenerationProviderConfig)> {
    let resolve_key = |name: &str, pcfg: &crate::GenerationProviderConfig| -> Option<String> {
        if let Some(ref key) = pcfg.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
            let val = secret.expose().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    };
    let eligible = |pcfg: &crate::GenerationProviderConfig| {
        pcfg.enabled
            && !(skip_local
                && pcfg.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE)
    };

    gen_cfg
        .default_transcription_provider
        .as_ref()
        .and_then(|default_name| {
            gen_cfg
                .transcription_providers
                .get_key_value(default_name)
                .filter(|(_, pcfg)| eligible(pcfg))
                .and_then(|(name, pcfg)| resolve_key(name, pcfg).map(|key| (key, pcfg)))
        })
        .or_else(|| {
            gen_cfg.transcription_providers.iter().find_map(|(name, pcfg)| {
                if eligible(pcfg) {
                    resolve_key(name, pcfg).map(|key| (key, pcfg))
                } else {
                    None
                }
            })
        })
}

/// The original static SttConfig construction (url normalize + model pick).
fn static_stt_config(key: String, pcfg: &crate::GenerationProviderConfig) -> SttConfig {
    let base = pcfg.base_url.as_deref().unwrap_or("https://api.openai.com");
    let resolved = crate::generation::providers::url_normalize::resolve_base_url(base);
    let stt_endpoint = resolved.primary_endpoint(crate::generation::GenerationType::Transcription);
    let stt_base = stt_endpoint.trim_end_matches("/audio/transcriptions").to_string();
    let stt_model = pcfg.models.first().cloned().unwrap_or_else(|| "whisper-1".to_string());
    SttConfig { api_key: key, base_url: stt_base, model: stt_model }
}
```

**删除**旧 `pub fn resolve_stt_config`（仅两个调用方，本 Task 全部迁移；熵减）。inbound.rs 测试模块补：

```rust
    #[test]
    fn resolve_prefers_local_with_cloud_fallback() {
        use crate::config::types::generation::GenerationConfig;
        let vault = crate::gateway::security::SharedTokenManager::new_in_memory();
        let mut gen = GenerationConfig::default();
        // local (normalized shape) + one cloud provider with inline key
        let mut local = crate::GenerationProviderConfig::new("local");
        local.api_key = Some("local-sidecar".into());
        gen.transcription_providers.insert("local".into(), local);
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers.insert("openai_whisper".into(), cloud);
        gen.default_transcription_provider = Some("local".into());

        match resolve_stt_source(&gen, &vault) {
            Some(SttSource::Local { fallback: Some(f) }) => assert_eq!(f.api_key, "sk-cloud"),
            other => panic!("expected Local with fallback, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn resolve_pure_cloud_stays_static() {
        use crate::config::types::generation::GenerationConfig;
        let vault = crate::gateway::security::SharedTokenManager::new_in_memory();
        let mut gen = GenerationConfig::default();
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers.insert("openai_whisper".into(), cloud);
        match resolve_stt_source(&gen, &vault) {
            Some(SttSource::Static(cfg)) => assert_eq!(cfg.api_key, "sk-cloud"),
            _ => panic!("expected Static"),
        }
    }
```

> `SharedTokenManager::new_in_memory()` 若不存在，grep `SharedTokenManager` 现有测试找惯用构造（inbound/handlers 的测试里有先例）；以现状为准。

- [ ] **Step 4: 三个消费点迁移**

① `src/gateway/inbound_router/mod.rs`：

- :116 `pub(super) stt_config: Option<super::voice::inbound::SttConfig>` → `pub(super) stt_source: Option<super::voice::inbound::SttSource>`；:172 初始化同步改名。
- :279 `with_stt_config` → 改：

```rust
    pub fn with_stt_source(mut self, source: super::voice::inbound::SttSource) -> Self {
        self.stt_source = Some(source);
        self
    }
```

- :587-598 使用点改为：

```rust
        let has_stt = self.stt_source.is_some();
        // ...
        if let Some(ref stt_source) = self.stt_source {
            match stt_source.materialize().await {
                Ok(stt_config) => {
                    // 原 process_inbound_voice 调用原样，参数 &stt_config
                }
                Err(super::voice::inbound::SttUnavailable::Downloading { percent }) => {
                    // 语音模型下载中：不转写、不报错；保留附件并给出对话内提示
                    if ctx.message.text.is_empty() {
                        ctx.message.text =
                            format!("[语音模型下载中 {percent}%，请稍候重试或改用文字]");
                    }
                }
                Err(super::voice::inbound::SttUnavailable::Error(e)) => {
                    tracing::warn!(error = %e, "local STT unavailable, no fallback");
                }
            }
        }
```

（精确融入现有控制流：materialize 成功分支内放置原有 `process_inbound_voice` 整段逻辑，失败分支不改写消息结构、走原"未转写"路径。以 :587-640 实际代码为准做最小内联。）

② `src/bin/aleph-server/commands/start/builder/subsystems.rs:776-786`：

```rust
    if let Some(ref cfg_arc) = app_config {
        let cfg = cfg_arc.read().await;
        if let Some(stt) =
            alephcore::gateway::voice::inbound::resolve_stt_source(&cfg.generation, &vault)
        {
            inbound_router = inbound_router.with_stt_source(stt);
            if !daemon {
                println!("  Inbound router: voice STT enabled (local-aware source)");
            }
        }
    }
```

③ `src/gateway/handlers/voice.rs:75` 附近（panel `voice.transcribe` RPC）：`resolve_stt_config(...)` → `resolve_stt_source(...)` 后接 `.materialize().await`，`SttUnavailable::Downloading` 映射为带百分比的用户可读 RPC 错误（沿用该文件现有错误返回风格），`Error` 同理。import 行同步更新。

- [ ] **Step 5: MediaProcessor 分支（agent_init/mod.rs:1113-1149）**

将 `if let Some((key, pcfg)) = tcfg { ... }` 块替换为：

```rust
                if let Some((key, pcfg)) = tcfg {
                    if pcfg.provider_type
                        == alephcore::config::types::voice_local::LOCAL_PROVIDER_TYPE
                    {
                        if !daemon {
                            println!("  MediaProcessor: local voice transcription enabled (sidecar)");
                        }
                        Some(Box::new(
                            alephcore::gateway::voice::local_provider::LocalTranscription,
                        ) as Box<dyn TranscriptionService>)
                    } else {
                        let whisper = WhisperTranscription::new(
                            key,
                            pcfg.base_url.clone(),
                            pcfg.models.first().cloned(),
                        );
                        if !daemon {
                            println!("  MediaProcessor: Whisper transcription enabled (from transcription provider)");
                        }
                        Some(Box::new(whisper) as Box<dyn TranscriptionService>)
                    }
                } else {
                    None
                }
```

（`key` 在 local 分支未用——保持解构以免改动选择 walk；加 `let _ = &key;` 或前缀下划线按 clippy 指引。）

- [ ] **Step 6: 编译 + 测试**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib "voice::inbound" && cargo test -p alephcore --lib local_provider`
Expected: check 过（编译器引导补齐 import/改名涟漪——**逐个修，勿漏 tests/**）；新增 2 测试 passed。

- [ ] **Step 7: Commit**

```bash
git add src/gateway/voice/ src/generation/providers/factory.rs src/gateway/inbound_router/mod.rs \
  src/gateway/handlers/voice.rs src/bin/aleph-server/commands/start/builder/
git commit -m "voice: local providers + late-bound stt source with cloud fallback"
```

---

### Task 12: core — `TtsOutcome` 下载中预检（不计失败 + 文本提示）

**Files:**
- Modify: `src/gateway/voice/outbound.rs`
- Modify: `src/gateway/reply_emitter/emitter/helpers.rs:83-134`

- [ ] **Step 1: outbound 加 outcome 层（旧 `generate_tts` 保留为薄壳，inbound_router:735 与 handlers/voice.rs:283 两处调用方零改动）**

`src/gateway/voice/outbound.rs` 追加：

```rust
/// TTS attempt outcome — lets the reply emitter distinguish "model still
/// downloading" (not a failure, don't count) from real failures (count,
/// 3-strike auto-disable preserved).
pub enum TtsOutcome {
    Generated(Attachment),
    /// Local sidecar still fetching models; carry progress for the user hint.
    Downloading { percent: Option<u8> },
    Failed,
}

/// Pure decision: map a remote model state probe to a preflight outcome.
/// `None` means "proceed with generation".
pub fn preflight_outcome(
    state: Option<&crate::gateway::voice::sidecar::RemoteModelState>,
) -> Option<TtsOutcome> {
    match state {
        Some(crate::gateway::voice::sidecar::RemoteModelState::Downloading { percent }) => {
            Some(TtsOutcome::Downloading { percent: Some(*percent) })
        }
        _ => None,
    }
}

/// Like [`generate_tts`] but with a typed outcome. Preflights the local
/// sidecar's model state when the resolved provider is "local".
pub async fn generate_tts_outcome(
    text: &str,
    voice_state: &VoiceState,
    generation_registry: &GenerationProviderRegistry,
    generation_config: &GenerationConfig,
) -> TtsOutcome {
    let resolved_local = voice_state.provider.as_deref() == Some("local")
        || (voice_state.provider.is_none()
            && generation_config.default_speech_provider.as_deref() == Some("local"));
    if resolved_local {
        if let Some(sup) = crate::gateway::voice::sidecar::global() {
            match sup.tts_model_state().await {
                Ok(state) => {
                    if let Some(outcome) = preflight_outcome(Some(&state)) {
                        return outcome;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "local TTS preflight failed");
                    return TtsOutcome::Failed;
                }
            }
        }
    }
    match generate_tts(text, voice_state, generation_registry, generation_config).await {
        Some(attachment) => TtsOutcome::Generated(attachment),
        None => TtsOutcome::Failed,
    }
}
```

测试模块追加：

```rust
    #[test]
    fn preflight_maps_downloading_only() {
        use crate::gateway::voice::sidecar::RemoteModelState as S;
        assert!(matches!(
            preflight_outcome(Some(&S::Downloading { percent: 42 })),
            Some(TtsOutcome::Downloading { percent: Some(42) })
        ));
        assert!(preflight_outcome(Some(&S::Ready)).is_none());
        assert!(preflight_outcome(Some(&S::Other("error".into()))).is_none());
        assert!(preflight_outcome(None).is_none());
    }
```

- [ ] **Step 2: helpers.rs 消费 outcome**

`src/gateway/reply_emitter/emitter/helpers.rs:83` 起，把 `if let Some(attachment) = generate_tts(...)` 结构替换为：

```rust
            use crate::gateway::voice::outbound::TtsOutcome;
            match crate::gateway::voice::outbound::generate_tts_outcome(
                text,
                &voice_state,
                registry,
                &gen_config,
            )
            .await
            {
                TtsOutcome::Generated(attachment) => {
                    // （原成功分支整段不动：record_success + voice-only OutboundMessage + send）
                }
                TtsOutcome::Downloading { percent } => {
                    // Model download in progress — NOT a provider failure:
                    // don't touch the 3-strike counter (spec §5).
                    let pct = percent.map(|p| format!("{p}%")).unwrap_or_else(|| "…".into());
                    let hinted = format!("{text}\n\n(语音模型下载中 {pct}，本条先以文本回复)");
                    self.send_to_channel(&hinted).await;
                }
                TtsOutcome::Failed => {
                    // （原失败分支整段不动：record_failure + auto-disable warn + 文本回退）
                }
            }
```

（“原…分支整段不动”指把现有 :91-122 与 :123-134 的代码原样搬进对应 arm——执行时以文件现状为准做纯结构性搬移。）

- [ ] **Step 3: 编译 + 测试**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib outbound`
Expected: check 过；原 5 个 timeout 测试 + 新 preflight 测试 passed。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/voice/outbound.rs src/gateway/reply_emitter/emitter/helpers.rs
git commit -m "voice: typed tts outcome — downloading preflight skips failure counting"
```

### Task 13: core — `local_voice` 工具（R8）+ `voice_mode_set` 预热钩子

**Files:**
- Create: `src/builtin_tools/voice_tools/local_voice.rs`
- Modify: `src/builtin_tools/voice_tools/mod.rs`（导出）
- Modify: `src/executor/builtin_registry/groups.rs`（:100 附近 `"voice_mode_set",` 后加 `"local_voice",`）
- Modify: `src/executor/builtin_registry/builder/optional_tools.rs`（:460 附近照 voice_mode_set 的 reg() 块新增）
- Modify: `src/executor/builtin_registry/registry.rs`（:1508 附近照 voice_mode_set dispatch arm 新增）
- Modify: `src/builtin_tools/voice_tools/voice_mode_set.rs`（execute 内加 warmup 钩子）

- [ ] **Step 1: 工具实现 + 测试**

`src/builtin_tools/voice_tools/local_voice.rs`：

```rust
//! local_voice tool — conversational status/warmup for the aleph-voice sidecar
//! (R8: 对话即管理面板).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gateway::voice::sidecar;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LocalVoiceAction {
    /// Report sidecar/model/engine state without spawning anything.
    Status,
    /// Spawn the sidecar (if needed) and pre-load models + engines.
    Warmup,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LocalVoiceArgs {
    pub action: LocalVoiceAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVoiceOutput {
    pub success: bool,
    pub message: String,
    /// Raw sidecar /v1/voice/status JSON when reachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}

pub struct LocalVoiceTool;

impl LocalVoiceTool {
    pub const fn new() -> Self {
        Self
    }

    pub async fn execute(&self, args: LocalVoiceArgs) -> LocalVoiceOutput {
        let Some(sup) = sidecar::global() else {
            return LocalVoiceOutput {
                success: false,
                message: "Local voice is disabled. Set [voice.local] enabled = true in config and restart.".into(),
                status: None,
            };
        };
        match args.action {
            LocalVoiceAction::Status => match sup.peek_endpoint().await {
                None => LocalVoiceOutput {
                    success: true,
                    message: "Sidecar not running (starts on first voice use or warmup). Models persist on disk.".into(),
                    status: None,
                },
                Some(ep) => {
                    let fetched = reqwest::Client::new()
                        .get(format!("{}/voice/status", ep.base_url))
                        .bearer_auth(&ep.token)
                        .timeout(std::time::Duration::from_secs(3))
                        .send()
                        .await;
                    match fetched {
                        Ok(resp) if resp.status().is_success() => {
                            let v: serde_json::Value = resp.json().await.unwrap_or_default();
                            LocalVoiceOutput {
                                success: true,
                                message: "Sidecar running.".into(),
                                status: Some(v),
                            }
                        }
                        other => LocalVoiceOutput {
                            success: false,
                            message: format!("Sidecar unreachable: {other:?}"),
                            status: None,
                        },
                    }
                }
            },
            LocalVoiceAction::Warmup => match sup.warmup().await {
                Ok(()) => LocalVoiceOutput {
                    success: true,
                    message: "Warmup started: models downloading/loading in the background.".into(),
                    status: None,
                },
                Err(e) => LocalVoiceOutput {
                    success: false,
                    message: format!("Warmup failed: {e:#}"),
                    status: None,
                },
            },
        }
    }
}

#[async_trait]
impl AlephTool for LocalVoiceTool {
    const NAME: &'static str = "local_voice";
    const DESCRIPTION: &'static str = "Inspect or warm up the local voice (STT/TTS) sidecar. \
        Use action=status when the user asks about local voice readiness or model download progress; \
        action=warmup to pre-load models so the next voice interaction is instant.";

    type Args = LocalVoiceArgs;
    type Output = LocalVoiceOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"local_voice(action="status")"#.to_string(),
            r#"local_voice(action="warmup")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.execute(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_state_is_friendly_not_fatal() {
        // No init_global in tests → graceful "disabled" message.
        let out = LocalVoiceTool::new()
            .execute(LocalVoiceArgs { action: LocalVoiceAction::Status })
            .await;
        assert!(!out.success);
        assert!(out.message.contains("voice.local"));
    }

    #[test]
    fn action_parses_lowercase() {
        let a: LocalVoiceArgs = serde_json::from_str(r#"{"action":"warmup"}"#).unwrap();
        assert_eq!(a.action, LocalVoiceAction::Warmup);
    }
}
```

> ⚠️ 测试隔离：`sidecar::GLOBAL` 是进程级 OnceLock——`disabled_state_is_friendly_not_fatal` 与任何调用 `init_global` 的测试同进程时顺序敏感。Task 10 的测试**刻意不用** `init_global`（直接 `VoiceSidecarSupervisor::new`），保持此约定。

- [ ] **Step 2: 四处注册（照 voice_mode_set 现成模式逐处镜像）**

① `src/builtin_tools/voice_tools/mod.rs`：

```rust
pub mod local_voice;
pub use local_voice::{LocalVoiceArgs, LocalVoiceOutput, LocalVoiceTool};
```

② `groups.rs` `system_config` 类目 `"voice_mode_set",` 之后：`"local_voice",`

③ `optional_tools.rs`（voice_mode_set 的 reg() 块之后）：

```rust
    reg(
        tools,
        "local_voice",
        "Inspect or warm up the local voice (STT/TTS) sidecar: status shows model download \
         progress and engine state; warmup pre-loads models for instant voice. Use when the \
         user asks about local voice readiness or wants to prepare voice mode.",
        schema::<crate::builtin_tools::voice_tools::LocalVoiceArgs>("local_voice"),
    );
    info!("Registered local_voice tool in BuiltinToolRegistry");
```

④ `registry.rs`（voice_mode_set arm 之后）：

```rust
            "local_voice" => Box::pin(async move {
                let tool = crate::builtin_tools::voice_tools::LocalVoiceTool::new();
                tool.call_json(arguments).await
            }),
```

（`call_json` 为 AlephTool 惯例入口——以 voice_mode_set arm 的实际写法为准镜像。）

- [ ] **Step 3: voice_mode_set 预热钩子**

`src/builtin_tools/voice_tools/voice_mode_set.rs` `execute()` 中、`update_voice_state` await 之后（:110 附近）插入：

```rust
        // Spec §4.5: enabling voice fires an async warmup so models are loaded
        // by the time the user speaks. Fire-and-forget; failures only log.
        if enabled {
            if let Some(sup) = crate::gateway::voice::sidecar::global() {
                tokio::spawn(async move {
                    if let Err(e) = sup.warmup().await {
                        tracing::debug!(error = %e, "voice warmup on enable failed");
                    }
                });
            }
        }
```

现有 7 个测试不初始化 global → 钩子是 no-op，应全数保持绿色（这本身就是"未启用本地语音时零影响"的回归验证）。

- [ ] **Step 4: 测试 + 提交**

Run: `cargo test -p alephcore --lib local_voice && cargo test -p alephcore --lib voice_mode_set`
Expected: 新 2 + 原 7 全绿。

```bash
git add src/builtin_tools/voice_tools/ src/executor/builtin_registry/
git commit -m "tools: local_voice status/warmup tool + warmup hook on voice enable"
```

---

### Task 14: 打包 — justfile + Tauri externalBin + 测试链

**Files:**
- Modify: `justfile`
- Modify: `desktop/shell/tauri.conf.json`

- [ ] **Step 1: justfile**

新增 recipe（`wasm` recipe 附近）：

```makefile
# Build the aleph-voice sidecar (release)
voice-build:
    cargo build -p aleph-voice --release

# Fast aleph-voice tests (no sherpa C++ build)
voice-test:
    cargo test -p aleph-voice --no-default-features --lib
```

`build` recipe 依赖链 `build: wasm swift-bridge` → `build: wasm voice-build swift-bridge`。

`_stage-shell-binaries`（justfile:60-69）在 aleph-server 的 install 行后加：

```bash
    install -m 0755 "target/{{profile}}/aleph-voice$ext" "{{shell_dir}}/binaries/aleph-voice-$triple$ext"
```

`test-all` recipe 追加一行 `just voice-test`（定位：`grep -n "^test-all" justfile`）。

- [ ] **Step 2: tauri.conf.json**

`desktop/shell/tauri.conf.json:20` `externalBin` 数组：

```json
    "externalBin": ["binaries/aleph-server", "binaries/aleph-voice"],
```

- [ ] **Step 3: 验证**

Run: `just voice-build && just voice-test && ls -la target/release/aleph-voice`
Expected: 构建+测试过，二进制存在（记录体积）。

Run（可选完整链，耗时长）: `just build && ls desktop/shell/binaries/`
Expected: `aleph-voice-<triple>` 与 `aleph-server-<triple>` 并列。

- [ ] **Step 4: clippy + fmt 收口**

Run: `cargo fmt && cargo clippy -p aleph-voice -p alephcore -- -D warnings 2>&1 | tail -5`
Expected: 无警告（修到零）。

- [ ] **Step 5: Commit**

```bash
git add justfile desktop/shell/tauri.conf.json
git commit -m "build: package aleph-voice as tauri externalBin + voice test lane"
```

---

### Task 15: 人工验收（HUMAN — 按 CLAUDE.md 部署链替换二进制后执行）

**部署**（worktree 合回 main 后）：

```bash
just wasm && cargo build --release -p alephcore --bin aleph-server && just voice-build
./target/release/aleph-server stop
# dev daemon 路径：
cargo run --release -p alephcore --bin aleph-server -- start --daemon
# （.app 部署则按 CLAUDE.md 的 mv/cp/kill supervisor 链，另需 cp target/release/aleph-voice 到 .app 同目录）
```

配置 `~/.aleph/config.toml` 加：

```toml
[voice.local]
enabled = true
```

**验收清单**（spec §6.3 + 切换承诺）：

- [ ] 对话说"打开语音模式" → `voice_mode_set` 触发 + warmup 自动开始；`local_voice(status)` 能看到下载百分比推进
- [ ] 下载完成前发语音 → 文本回复带"语音模型下载中 N%"提示，**voice mode 未被 3-strike 关闭**
- [ ] Telegram 语音 round-trip：语音 → 本地转写正确 → 回复 → **opus 语音气泡**可播放
- [ ] Panel voice mode 同链路（panel 录音 webm/mp4 能被 sidecar 解码）
- [ ] 中英混合句转写与合成质量复核
- [ ] Activity Monitor 观察：TTS 用后 2min 卸载（RSS 回落）→ STT 10min 卸载 → 30min 进程消失；再次语音自动重拉
- [ ] **本地/云切换**：config 设 `default_speech_provider = "openai_tts"`（或对话改）→ 重载后 TTS 走云、STT 仍本地；再设回 `"local"` 复原；`voice_mode_set(provider="openai_tts")` 单渠道覆盖生效
- [ ] 断网开始下载 → 中断 → 恢复网络 → 续传完成（看 `local_voice(status)` 百分比续接而非归零）
- [ ] `kill -9 <sidecar pid>` × 1 → 下次语音自动重拉；连杀 3 次内快速请求 → 进入 cooldown，降级文本，5min 后自愈

结果记入 `docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md` 附录（acceptance 段）。

---

## 范围外后续任务（不在本计划内，记录避免隐含承诺）

1. **Linux/Windows 三平台 CI**：sherpa-onnx 静态编译进 `aleph-app-release.yml`（CI 平台门控血泪史——`#[cfg]`/链接类问题 macOS 本地不可见，需专轮处理 + `just verify-build` 预检）
2. Tier 2 流式管线（句级切片 TTS / TG 分段语音 / 首包优化）
3. Tier 3 全双工原生客户端（独立 spec）
4. Panel 下载/管理 UI（本轮对话式 status 够用）
5. 模型升级机制（本轮以 marker 文件判完整性；后续轮引入 manifest.json 版本档再做换代重下）

## Plan 自检备忘（执行者注意）

- sherpa-rs 真实 API 以 **Task 1 spike verdict 为唯一事实源**——Task 6/8 的 `sherpa_rs::` 调用按 verdict 修正后再编译。
- `VoiceSection` 包装（Task 9 Step 2）落地后，Task 10/11/12/13 中所有 `cfg.voice_local.X` / `cfg.local_voice()` 访问统一走 `Config::local_voice()` 助手。
- `GenerationError` 构造名（Task 11）以 `src/generation/error.rs` 真实定义为准。
- 每个 Task 收口跑 `cargo fmt`；rustfmt 若卷出无关文件（mod.rs 递归惯性），`git add` 只挑本 Task 文件。




---

## Amendment 1 (2026-06-13): Pivot to BYO endpoint (Ollama-style) — self-built sidecar dropped

**User decision** after Tier-0 spike round 2 + Qwen3-TTS research: local voice follows the Ollama model.
Users install their own OpenAI-compatible voice server (e.g. `mlx-audio` server with Qwen3-TTS +
Whisper/Qwen3-ASR — exposes `/v1/audio/speech` and `/v1/audio/transcriptions`); Aleph only provides
the local interface. Rationale: sherpa-onnx quality ceiling (kokoro zh FAIL, melo "将就") can never
reach Qwen3-TTS level; R3 (内核只调度，不搬砖); zero inference-code maintenance.

**Dropped**: Tasks 6/8/14; `aleph-voice` crate is removed from the tree (Tasks 2-5/7 artifacts —
git history keeps them); `scripts/voice_models_fetch.sh`; `src/gateway/voice/sidecar.rs` supervisor.
Spike docs are kept as the decision record.

**Kept as-is**: provider seams (`local_provider.rs` protocol shape), SttSource late binding with
cloud fallback, TtsOutcome 3-strike degradation, config normalize (fill-empty-only + disable cleanup).

**Task 16 (replaces 6/8/14): BYO rework** — see Explore map in session. Summary:
remove crate + sidecar.rs + init_global; `VoiceLocalConfig` gains `endpoint` + optional `api_key`,
loses `binary_path`/`idle_*`/`download_source`; providers resolve endpoint from config;
`SttUnavailable::Downloading` / `TtsOutcome::Downloading` variants removed (zero producers under BYO);
`local_voice` tool becomes endpoint health probe (warmup action + voice_mode_set hook removed).

**Task 17 (replaces 15): human acceptance** — user runs mlx-audio server, configures
`[voice.local] endpoint`, verifies voice round-trip + cloud fallback on endpoint-down.
