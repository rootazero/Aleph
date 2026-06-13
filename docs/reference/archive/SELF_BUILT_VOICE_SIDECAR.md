# 自建语音引擎 Sidecar（aleph-voice）设计存档

> **状态：已删除（2026-06-13），本文为重启路线的设计存档。**
> 完整实现保留在 git 历史（删除 commit `414e9ec42`，完整树在 `414e9ec42^`）；
> 完整 15-Task 伪代码在 [plans/2026-06-12-aleph-voice-local-sidecar.md](../../superpowers/plans/2026-06-12-aleph-voice-local-sidecar.md)；
> spike 实测全文在 [spikes/2026-06-12-aleph-voice-spike.md](../../superpowers/spikes/2026-06-12-aleph-voice-spike.md)。
> 本文是三者的**精炼索引 + 设计结晶**，供将来重启自建路线时直接续作。

---

## 1. 为什么存档（转向 BYO 的决策记录）

aleph-voice 是一个独立 Rust sidecar 二进制（sherpa-onnx 推理：SenseVoice STT + Kokoro/Melo TTS），
由 daemon 按需懒拉起、闲置释放、模型运行时下载，经 OpenAI 兼容端点接入 core 的 provider 缝。
实现推进到约 7 成（crate 侧 Task 1-5/7 完成，core 侧 Task 9-13 完成）后，产品转向 **BYO 端点**
（Ollama 模式：用户自跑 OpenAI 兼容语音服务如 mlx-audio + Qwen3-TTS，Aleph 只配置 endpoint），
整个 crate 在 `414e9ec42` 删除。

**转向理由**（plan Amendment 1，2026-06-13 用户决策）：

1. **质量天花板** — sherpa-onnx 生态的最优解仍不够：Kokoro v1.1-zh 中文听感人工门 **FAIL**
   （发音不准、吐字不清晰、语调走样），备胎 MeloTTS-zh_en 仅"将就"（中文 PASS 但英文弱、
   中英混合句两家都不行），永远到不了 Qwen3-TTS 级别。
2. **R3 内核不搬砖** — 自建推理 = Aleph 维护推理代码 + 模型分发 + 三平台 sherpa-onnx 静态编译
   CI；BYO 把这些全部外置，Aleph 零推理代码维护。

**何种条件下值得重启自建**：

- **零配置开箱**优先级提升 — BYO 要求用户自己装语音服务器，若产品要求"装完 App 语音即用"，
  自带 sidecar 是唯一路径（externalBin 打包链 Task 14 已设计）。
- **目标平台无生态服务可装** — 嵌入式 / NAS / 无 Python 生态环境，sherpa-onnx 静态二进制是
  少数可行选项。
- **本地模型质量追平** — sherpa-onnx 生态出现中文听感过关的模型（或 Qwen3-TTS 出 ONNX 版），
  门 1 不再 FAIL。
- 重启时**协议面零返工**：BYO 化保留了 provider 缝（`local_provider.rs` 协议形状、SttSource
  晚绑定 + 云回退、TtsOutcome 三振降级、config normalize fill-empty-only），sidecar 只需重新
  作为这些缝的另一个 endpoint 提供者插回。

---

## 2. 架构设计精华

### 2.1 Sidecar 进程模型（loopback + per-spawn token + READY 握手 + crash-loop 防护）

独立 workspace binary crate，由 core 侧 `VoiceSidecarSupervisor`（进程级 OnceLock 单例，
镜像 SwiftBridge 先例）懒 spawn——首次语音需求才拉起，STT/TTS 两条路径共享一个子进程。

- **动态端口 + 每次 spawn 铸新 token**：sidecar bind `127.0.0.1:0`，token 为双 UUID 拼接，
  经 stdout 单行握手交给 supervisor（日志全走 stderr，stdout 只承载这一行）：

  ```text
  READY {"v":1,"port":54321,"token":"..."}
  ```

  supervisor 在 `HANDSHAKE_TIMEOUT = 10s` 内逐行读 stdout 找 `READY ` 前缀，解析出
  `SidecarEndpoint { base_url: "http://127.0.0.1:{port}/v1", token }`，之后 spawn 一个
  drain task 持续吸掉剩余 stdout 防管道阻塞。token 作为 Bearer 进 axum 中间件校验
  （loopback-only 的纵深防御）。

- **crash-loop 防护**（纯函数决策，可单测）：

  ```rust
  const CRASH_WINDOW: Duration = Duration::from_secs(60);
  const CRASH_LIMIT: usize = 3;
  const COOLDOWN: Duration = Duration::from_secs(300);

  /// >= CRASH_LIMIT crashes inside CRASH_WINDOW
  pub fn crash_loop_active(crashes: &VecDeque<Instant>, now: Instant) -> bool
  ```

  握手超时 / 非零退出都记一次 crash（VecDeque 截留最近 8 条）；触发即进入 5min cooldown，
  期间 `ensure_endpoint()` 直接 bail，上层降级文本回复，5min 后自愈。

- **liveness 用 `try_wait()`**：每次取 endpoint 前探活；正常退出（深闲自退 exit 0）不计 crash。
  子进程 `kill_on_drop(true)`，daemon 死则 sidecar 跟着死。

- **二进制定位**：默认在 `current_exe()` 同目录找 `aleph-voice`（Tauri externalBin 布局），
  `voice.local.binary_path` 可覆盖。

### 2.2 EngineSlot 三级内存策略（懒加载 / 闲置卸载 / 深闲进程自退）

16GB 机器上 TTS 引擎 ~0.9-1.7GB RSS，不能常驻。三级释放，每级一个纯函数决策（显式传
`now_ms`，测试无需时钟）：

| 级 | 触发 | 机制 | 默认 |
|---|---|---|---|
| 1 懒加载 | 首个请求 | `EngineSlot::get_or_load`，load 跑 `spawn_blocking` | — |
| 2 闲置卸载 | 引擎空闲超 TTL | tick 循环（10s 间隔）调 `maybe_unload` → drop 引擎 | TTS 120s / STT 600s |
| 3 深闲自退 | 整个进程无任何请求 | `should_exit` → `std::process::exit(0)`，supervisor 视为正常退出，下次需求重拉 | 1800s |

```rust
pub struct EngineSlot<E: ?Sized + Send + Sync> {
    state: tokio::sync::Mutex<Option<Arc<E>>>,
    last_used_ms: AtomicU64,
}
// get_or_load(now, load): 并发 caller 在 mutex 后排队，第二个复用首个的加载结果
// （spec 的 "Loading 期间请求排队 hold" 由锁免费获得）；handler 侧再包 15s 超时
// 返回 503 {"status":"loading"}。
pub fn should_unload(last_used_ms: u64, now_ms: u64, ttl_secs: u64) -> bool;
pub fn should_exit(last_activity_ms: u64, now_ms: u64, idle_exit_secs: u64) -> bool;
```

**为什么必须有第 3 级**：spike 实测引擎 drop 只还 ~40% RSS（onnxruntime/allocator arena 页
缓存残留 ~1GB），完全归还 OS 只能靠进程退出。`last_activity_ms` 由 auth 中间件在每个
authed 请求上刷新。

### 2.3 ModelManager（多源断点续传 + sha256 + 原子解包 + marker 就绪）

模型不随安装包分发（~1GB+），运行时按需下载到 `~/.aleph/models/voice/<id>/`。

- **静态 manifest**：`ModelSpec { id, urls: &'static [&'static str], sha256, marker }`——
  urls 按优先级排列（github → hf-mirror），sha256 钉 tarball，marker 是"解包完整"的证明文件。
- **状态机**：`Missing → Downloading{percent} → Unpacking → Ready | Error{message}`；
  `state()` 懒查磁盘——marker 存在即 Ready，**下载成果跨进程重启存活**（深闲自退不丢模型）。
- **并发安全**：per-model 异步锁（`HashMap<String, Arc<Mutex<()>>>`），并发 `ensure()` 只下载一次。
- **断点续传**：`.part` 文件 + HTTP `Range: bytes={existing}-`；服务器忽略 Range（非 206）则
  truncate 重来；checksum 失败删 `.part` 换下一个源重新开始。
- **原子解包**：tar.bz2 解到 `<dest>.unpack-tmp` → strip 单层根目录 → 验 marker 存在 →
  `fs::rename` 原子换入。失败/中断不会留下半套模型被误判 Ready。
- **端点门控**（handlers 侧）：请求到来时模型 Missing/Error → fire-and-forget `ensure()` +
  返 503 `{"status":"downloading","percent":N}`；Downloading/Unpacking → 503 带百分比；
  Ready → 进 EngineSlot 懒加载路径。core 侧把这个 503 翻译成"不计失败"的用户提示
  （TtsOutcome::Downloading 不进 3-strike 计数）。

### 2.4 OpenAI 兼容端点面（axum，4 端点）

```text
POST /v1/audio/transcriptions   multipart (file, language, model 忽略) → {"text", "language"}
POST /v1/audio/speech           JSON {input, voice, speed, response_format: wav|opus} → 音频字节
GET  /v1/voice/status           {stt:{model,model_state,engine_loaded}, tts:{...}, uptime_secs}
POST /v1/voice/warmup           202, fire-and-forget: ensure models + 预载引擎
```

- 与 OpenAI 协议同形 → core 复用既有 whisper-dialect HTTP 客户端，`"local"` 只是 provider
  registry 里普通一员；本地/云切换 = 纯配置（normalize 只填空，显式云配置永远赢）。
- `DefaultBodyLimit::max(25MB)`（axum 默认 2MB 不够语音文件）。
- TTS 默认 opus（OGG-Opus，Telegram 语音气泡原生格式），`response_format=wav` 备选。
- 引擎抽象：`SttEngine`/`TtsEngine` 两个 `Send + Sync` 同步 trait（caller 包 `spawn_blocking`），
  AppState 持 factory 闭包（`Arc<dyn Fn() -> anyhow::Result<Arc<dyn SttEngine>>>`）——测试注入
  Mock，main 注入 sherpa loads，server/lifecycle 层永远看不到后端：

  ```rust
  pub trait SttEngine: Send + Sync {
      fn transcribe(&self, samples: &[f32], language: Option<&str>) -> anyhow::Result<SttResult>;
  }
  pub trait TtsEngine: Send + Sync {
      fn synthesize(&self, text: &str, voice: &str, speed: f32) -> anyhow::Result<TtsAudio>;
  }
  ```

### 2.5 音频管线（anything-in → 16k mono f32 PCM）

- 入口 `decode_to_pcm_mono_16k(bytes, name_hint)`：OggS 魔数 + OpusHead 探测 → 自写 ogg_opus
  解码（opus 解码器原生重采样到 16k）；其余走 symphonia（wav/mp3/m4a-aac/flac/webm 容器），
  webm/mkv 内 opus 流由 symphonia 拆包 + opus crate 解码（symphonia 无 opus codec）；
  非 16k 经 rubato SincFixedIn 高质量重采样。
- 出口：`encode_wav`（hound, i16）与 `ogg_opus::encode`（RFC 7845 手写 OpusHead/OpusTags 页 +
  20ms 帧 + granule 按 48k 计），覆盖 TG 语音气泡与通用播放两种消费端。

---

## 3. Spike 实测资产（sherpa-rs 0.6.8，Apple M4 / 16GB，2026-06-13）

> 完整数据见 spike 文档；此处为重启时直接可用的结论摘录。

### 3.1 三引擎 API 形状终表

| 引擎 | 类型 | 构造 | 推理 | 必设字段（derive(Default) 陷阱） | Send/Sync |
|---|---|---|---|---|---|
| TTS 默认（melo） | `sherpa_rs::tts::VitsTts` | `new(VitsTtsConfig) -> Self`（无 Result，坏路径推迟到 create） | `create(&mut, text, sid=0, speed) -> eyre::Result<TtsAudio>`（44.1 kHz） | `model/lexicon/tokens/dict_dir` + **`noise_scale=0.667, noise_scale_w=0.8, length_scale=1.0, silence_scale=0.2`**（默认全 0.0 不可用）；data_dir 留空；rule_fsts 必须留空（UAF，见 3.4） | Send + Sync |
| TTS 可选（kokoro-en） | `sherpa_rs::tts::KokoroTts` | `new(KokoroTtsConfig) -> Self` | `create(&mut, text, sid, speed) -> eyre::Result<TtsAudio>`（24 kHz） | `model/voices/tokens/data_dir/lexicon/dict_dir` + **`length_scale=1.0`**（默认 0.0）；lang 留空 | Send + Sync |
| STT | `sherpa_rs::sense_voice::SenseVoiceRecognizer`（计划误写 `SenseVoice`） | `new(SenseVoiceConfig) -> eyre::Result<Self>` | `transcribe(&mut, sample_rate: u32, &[f32]) -> OfflineRecognizerResult { lang, text, timestamps, tokens }`（lang 形如 `<\|zh\|>`，非 Result） | `model`(int8)/`tokens`；language="auto"/use_itn=true 已是 Default | Send + Sync |

通用：`OnnxConfig { provider: "cpu", num_threads: 1, debug }`，建议 num_threads 2-4 压 RTF；
`TtsAudio { samples: Vec<f32>, sample_rate: u32, duration: i32 }`。

### 3.2 性能实测（--release，num_threads=1）

| 指标 | Kokoro v1.1-zh | MeloTTS-zh_en (VitsTts) |
|---|---|---|
| 引擎加载 | 3.40 s | **1.31 s** |
| 合成 RTF | 0.8–1.0（无实时余量） | **0.61–0.68**（有余量） |
| 引擎 RSS | ~1706 MB | **~898 MB**（约一半） |
| 输出采样率 | 24 kHz | 44.1 kHz |
| 听感（人工门） | 中文 **FAIL** / 英文 PASS | 中文 **PASS**（"比前一个中文更好"）/ 英文弱 / 混合句不佳 |

STT（SenseVoice int8）：加载 1.02–1.25s，转写 ~300-800ms（RTF≈0.11-0.12），输入 24k wav
sherpa 内部自动重采样。中文音色闭环还原度高（"近乎全对"，专名失真为预期）。

内存（mem_spike 双周期）：baseline 9.0 MB → loaded 1714.6 → dropped 995.8 → 二次 loaded
1708.0（**不叠加 = 无泄漏**）→ dropped 1009.0。drop 每周期确定性还 ~710 MB，残留 ~1GB 是
onnxruntime/allocator arena——**严格判据（dropped−baseline < 50MB）不达标**，门 3 按
"无泄漏 + 深闲进程自退兜底"组合标准 PASS。

终局产品决策：**melo 为默认 TTS（单 speaker，sid 恒 0）**，Kokoro 留作英文可选音色
（`kokoro-en`，惰性下载）；**无自动语言路由**（混合句两家都救不了，路由也没用）。
Kokoro voices.bin = 103 voices（53,790,720 B ÷ 522,240 B/voice），官方 100 中文
(zf_*/zm_*) + 3 英文 (af_maple/af_sol/bf_vale)；闭环旁证 sid 0/1 = 英文、sid 50/100 = 中文。

### 3.3 模型包 sha256 实值

| 包 | tarball | 字节 | sha256 |
|---|---|---|---|
| SenseVoice-small | `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2` | 1,047,870,769 | `f6b2a72ebcb1ac7a764d4cfccd886e6bcb2a95c4657c2199d0ba95ed4b9ea71a` |
| Kokoro v1.1-zh | `kokoro-multi-lang-v1_1.tar.bz2` | 364,816,464 | `a3f4c73d043860e3fd2e5b06f36795eb81de0fc8e8de6df703245edddd87dbad` |
| MeloTTS-zh_en | `vits-melo-tts-zh_en.tar.bz2` | 167,006,755 | `e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514` |

下载源：`https://github.com/k2-fsa/sherpa-onnx/releases/download/{asr,tts}-models/<file>`
（github 直连成功，~3.8 MB/s）；备源 `https://hf-mirror.com/csukuangfj/<repo>/resolve/main/<file>`。
注意：**melo 的 sha256 只在 spike 文档里**——manifest.rs 删除时只含 sense-voice-small 与
kokoro-v1.1-zh 两条（melo 换型属未完成的 Task 6 范围）。

### 3.4 全部已知陷阱

1. **🐛 sherpa-rs 0.6.8 `rule_fsts`/`rule_fars` use-after-free（库 bug）**：
   `CommonTtsConfig::to_raw()` 里 `rule_fars.map(|v| v.as_ptr())` 在 map 闭包内取完裸指针即
   drop CString → C API 拿到悬垂指针。实测传 `rule_fsts="date.fst,number.fst"` 时 C 侧读到的
   字符串变成 model.onnx 路径，`FstHeader::Read: Bad FST header` 直接 abort。
   **任何非空 rule_fsts/rule_fars 都不可传**（留空 → null 安全）。后果：数字/日期 FST 归一化
   不可用，含数字回复按字面 token 念。对策候选：上游报修 / fork patch（`[patch.crates-io]`）/
   Rust 侧文本预归一化（数字→中文读法）。
2. **`#[derive(Default)]` 配置全 0.0 陷阱**：`VitsTtsConfig` 的 noise_scale/noise_scale_w/
   length_scale 默认 0.0 不可用，必须显式设规范值 `0.667 / 0.8 / 1.0`（silence_scale 取 0.2）；
   `KokoroTtsConfig.length_scale` 同理必须设 1.0。
3. **TTS 构造无 Result**：`VitsTts::new`/`KokoroTts::new` 返回 `Self`，模型路径错误推迟到
   `create()` 才以 `"audio is null"` 暴露——加载层应先自查文件存在性。
4. **错误类型是 `eyre::Result` 非 anyhow**：`eyre::Report` 不实现 `std::error::Error`，
   在 anyhow 函数里不能直接 `?`，须 `.map_err(|e| anyhow::anyhow!("{e}"))`。
5. **melo 包的 `model.int8.onnx` 是 133 字节 git-lfs 指针残留**（非真模型）——melo 的
   manifest marker 必须是 **`model.onnx`**（170,429,550 B 才是真模型）；melo 无
   espeak-ng-data、无 voices.bin（VITS 单 speaker，音色嵌在模型内）。
6. **SenseVoice 包 ~1GB 而非计划估的 ~230MB**：tarball 同时含 fp32 `model.onnx`(938MB) 与
   `model.int8.onnx`(239MB)，运行时只用 int8——解包后可删 fp32 省盘。
7. **melo 是单 speaker**：传 sid=1 时 sherpa 打 `contains only 1 speakers. sid should be in
   range [0, 0]` 并自动回落 sid=0 → `tts_voice` 对 melo 退化为常量。
8. **无需 cmake**：sherpa-rs 0.6.8 default features = `["download-binaries", "tts"]`，
   走预编译 sherpa-onnx 静态库（计划假设"首次编译需 cmake"未发生）。
9. 合成期 stderr 噪音 `lexicon.cc Unknown token: ❓`（lexicon 未覆盖个别符号），输出音频不受影响。

---

## 4. 代码考古指引

### 4.1 完整树（`git show 414e9ec42^:aleph-voice/<path>` 可读任意文件）

```text
aleph-voice/
├── Cargo.toml              # workspace member；feature "sherpa"（default）门控 bin/examples
├── build.rs                # VERSION 文件 → ALEPH_VERSION（CalVer 单一来源，同 alephcore）
├── src/
│   ├── lib.rs              # pub mod audio/engine/lifecycle/models/server
│   ├── main.rs             # ⚠️ 3 行占位（Task 8 未做：READY 握手/tick 循环/深闲自退未装配）
│   ├── engine/
│   │   ├── mod.rs          # SttEngine/TtsEngine trait + SttResult/TtsAudio（同步 trait，spawn_blocking 消费）
│   │   ├── mock.rs         # MockStt（回显 sample 数）/ MockTts（100ms 440Hz 正弦）——server/lifecycle 测试用
│   │   └── sherpa.rs       # ⚠️ 1 行占位（Task 6 未做：真引擎实现从未落地）
│   ├── lifecycle.rs        # EngineSlot<E> 懒加载槽 + should_unload/should_exit 纯函数（122 行）
│   ├── audio/
│   │   ├── mod.rs          # decode_to_pcm_mono_16k（symphonia+opus）/ resample_to_16k（rubato）/ encode_wav（232 行）
│   │   └── ogg_opus.rs     # RFC 7845 手写 OGG-Opus mux/demux，TG 语音气泡格式（114 行）
│   ├── models/
│   │   ├── manifest.rs     # ModelSpec 静态表（sense-voice-small + kokoro-v1.1-zh 实测 sha256；无 melo 条目）
│   │   └── mod.rs          # ModelManager：状态机/per-model 锁/断点续传/sha256/原子解包（393 行，含 Range-aware 测试服务器）
│   ├── server/
│   │   ├── mod.rs          # Router 组装 + AppState（factory 闭包注入引擎）+ 6 个 tower oneshot 测试（187 行）
│   │   ├── auth.rs         # Bearer token 中间件 + last_activity_ms 刷新（23 行）
│   │   └── handlers.rs     # transcriptions/speech/status/warmup 四端点 + 模型门控 503（215 行）
│   ├── examples/           # Task 1 spike（tts_spike/stt_spike/mem_spike/melo_spike，含 RSS 测量与 Send 探针）
│   └── tests/fixtures/tone.mp3
scripts/voice_models_fetch.sh   # 双源（github|hf-mirror）下载+sha256+解包脚本
```

core 侧已删除的 supervisor：`git show 17f133c7b^:src/gateway/voice/sidecar.rs`
（VoiceSidecarSupervisor + crash_loop_active + init_global/global OnceLock + fake-sidecar
shell 脚本测试）。

### 4.2 Task ↔ commit 对照表（全部已核实）

| Task | 内容 | Commit |
|---|---|---|
| 1 | Tier-0 spike（examples + manifest 实值 + verdict 文档） | `4a9c42db9` |
| 2 | engine traits + mocks | `ce3147d49` |
| 3 | lifecycle（EngineSlot） | `a587e6d06` |
| 4 | audio（decode/resample/wav/ogg-opus） | `b4202495a` |
| 5 | ModelManager | `9cf0687ad` |
| 7 | HTTP server（auth + 四端点 + 门控） | `75bced580` |
| 9 | core config `[voice.local]` + normalize | `9418722e5` |
| 10 | VoiceSidecarSupervisor | `ca8baffbf` |
| 11 | local providers + SttSource 晚绑定 | `4b37b6865` |
| 12 | TtsOutcome 下载预检 | `3fe3f227d` |
| 13 | local_voice 工具 + warmup 钩子 | `ae405da42` |
| — | **BYO 转向**（core 侧 rework，删 sidecar.rs/Downloading 变体/warmup） | `17f133c7b` |
| — | **crate 删除**（aleph-voice/ + fetch 脚本，−2221 行） | `414e9ec42` |
| — | plan Amendment 1 文档 | `c7e3ec0e4` |

### 4.3 仍存活的部分（BYO 化后保留，重启时的接驳点）

- `src/config/types/voice_local.rs` — `VoiceLocalConfig`（BYO 形：endpoint + api_key，
  丢了 binary_path/idle_*/download_source）+ normalize fill-empty-only
- `src/gateway/voice/local_provider.rs` — LocalTranscription/LocalVoiceProvider（改为从
  provider config 读 endpoint，supervisor 引用已删）
- `src/gateway/voice/inbound.rs` — SttSource 晚绑定 + 云回退（回退点从 materialize 时移到
  请求失败时；`SttUnavailable::Downloading` 变体已删）
- `src/gateway/voice/outbound.rs` — TtsOutcome 三振降级（`Downloading` 变体 + 预检已删）
- `src/builtin_tools/voice_tools/local_voice.rs` — 改为 endpoint 可达性探针（warmup action 已删）

重启自建 = 恢复 crate（Task 6/8 补完）+ 恢复 supervisor（`17f133c7b^` 整文件可用）+
把上述 BYO 删掉的 Downloading 变体/warmup/晚绑定 supervisor 路径接回（`17f133c7b` 的
diff 反向即是 wiring 清单）。

---

## 5. 当时已知的未完成项（重启续作起点）

| Task | 缺什么 | 续作要点 |
|---|---|---|
| **6 sherpa 引擎实装** | `engine/sherpa.rs` 是 1 行占位，真引擎从未写 | 按 §3.1 终表实现：默认 `VitsTts`（melo，规范值显式设）+ 可选 `KokoroTts` + `SenseVoiceRecognizer`；`Mutex<引擎>` 内部可变性（推理 `&mut self`，类型已证 Send+Sync）；eyre→anyhow map_err；构造前自查模型文件存在；rule_fsts 留空 + Rust 侧数字/日期文本预归一化；num_threads 调 2-4；manifest 补 melo 条目（marker=`model.onnx`，sha256 见 §3.3）+ VOICE_TABLE（melo sid0 默认 / kokoro-en）；plan Task 6 有完整伪代码但写于第一轮（按 Kokoro 默认），须按终局裁决换 melo 为主 |
| **8 serve 装配** | `main.rs` 是 3 行占位 | plan Task 8 完整伪代码可直接用：clap Args → READY 单行握手（stdout flush，日志 stderr）→ 10s tick 循环（maybe_unload×2 + should_exit→exit(0)）→ axum graceful shutdown；冒烟流程也在 plan 内 |
| **14 打包链** | justfile / Tauri externalBin 未动 | `voice-build`/`voice-test` recipe + `build` 链 + `_stage-shell-binaries` install 行 + `tauri.conf.json` externalBin 数组 + `test-all` 接 voice-test；plan Task 14 有逐行 diff |
| 15/17 人工验收 | 未执行 | plan Task 15 验收清单完整（含断点续传/kill -9 重拉/crash-loop cooldown/本地云切换） |
| 范围外 | Linux/Windows CI（sherpa-onnx 三平台静态编译，平台门控陷阱预警）、Tier 2 流式管线、模型升级机制（marker → manifest.json 版本档） | plan "范围外后续任务"一节 |

另一个口子：STT 真人录音准确度（门 2）是"凭闭环放行"——TTS wav 回转验证过，真人麦克风录音
从未测过，重启后应补录验证。
