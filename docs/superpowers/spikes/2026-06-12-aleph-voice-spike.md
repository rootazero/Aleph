# aleph-voice Tier-0 Spike Verdict — sherpa-rs 真机验证

> **FINAL** — 两轮人工听感门均已裁决。终局：**MeloTTS-zh_en 为默认 TTS（音色 = melo sid0）**，Kokoro v1.1-zh 降级为可选音色（英文较好，`kokoro-en`，ModelManager 惰性下载）；**无自动语言路由**（产品决策）。四门全部 PASS，终局表见下。

- **日期**: 2026-06-13（计划文档 2026-06-12；同日完成 Kokoro 第一轮、melo 备胎第二轮、终局裁决）
- **机器**: Apple M4 / 16 GB / macOS 27.0 (26A5353q)
- **sherpa-rs**: `0.6.8`（计划写 `0.6`，语义化解析命中 0.6.8）
- **构建**: `--release`，onnxruntime 走 sherpa-rs 默认 `download-binaries` 预编译产物（**本机无 cmake，无需源码编译**——计划假设"首次编译需 cmake"未发生）

---

## 四个验证门 Verdict

### 终局（两轮裁决合并，2026-06-13）

| # | 门 | 终局 Verdict |
|---|----|---------|
| 1 | 听感可接受 | **PASS（melo）** — 用户原话："比前一个中文更好"。中英混合句"效果不是很好"→ 记入 Known Limitations |
| 2 | STT 转写准确度 | **PASS（凭闭环放行）** — 真人录音暂不录，保留后期补录验证的能力 |
| 3 | 内存确定性回落 | **PASS** — 标准 = 无泄漏（双周期不叠加）+ 深闲进程自退兜底（Task 8）；melo 引擎实测 ~898 MB（约 Kokoro 一半）进一步缓解水位 |
| 4 | sherpa-rs 覆盖度 | **PASS** — VitsTts（melo）+ KokoroTts + SenseVoiceRecognizer 加载/推理全可用。**在案例外**：`rule_fsts`/`rule_fars` 经 sherpa-rs 0.6.8 传入是 use-after-free（库 bug，API §10）；Task 6 对策 = **留空 + Rust 侧数字/日期文本预归一化** |

**产品决策（用户选定）**：melo 为默认 TTS（默认音色 = **melo sid0**）；Kokoro 保留为音色表可选项（如 `voice="kokoro-en"`，默认 sid0，ModelManager 惰性下载，不用不下）；**无自动语言路由**。

### Known Limitations（在案）

- **中英混合句效果不佳**：melo 与 kokoro 两个模型家族的共同短板（melo 弱在嵌入英文术语，kokoro 中文整体 FAIL）——双模型按语言路由也救不了混合句，故产品决策不做自动路由
- Kokoro 中文整体不可用（第一轮 FAIL），仅作英文可选音色保留
- `rule_fsts` 数字/日期 FST 归一化经 sherpa-rs 0.6.8 不可用（UAF），含数字回复按字面 token 念，待 Rust 侧预归一化补位（Task 6）

### 第一轮：Kokoro v1.1-zh（用户裁决，2026-06-13）

| # | 门 | Verdict |
|---|----|---------|
| 1 | 中文听感可接受 | **FAIL** — 用户原话：发音不准、吐字不清晰、语调走样。英文听感 **PASS**（很好） |
| 2 | STT 转写准确度可接受 | **PASS（凭闭环结果放行）** — 真人录音暂不录；保留后期补录验证的能力 |
| 3 | 内存确定性回落 | **PASS（用户裁决）** — 按"无泄漏 + 深闲进程自退兜底"标准（双周期不叠加证无泄漏；完全归还 OS 由 Task 8 深闲自退保证），非严格 RSS<50MB 判据 |
| 4 | sherpa-rs 覆盖度 | Kokoro/SenseVoice 加载+推理均通过；TTS 侧最终覆盖度随备胎裁决（melo 有一处 `rule_fsts` UAF 缺口，见下） |

**门 1 FAIL → 按计划备胎条款换 TTS：MeloTTS-zh_en（同 sherpa-onnx 库内），第二轮 spike 数据如下。**

### 第二轮：MeloTTS-zh_en 备胎（2026-06-13）

| # | 门 | Verdict |
|---|----|---------|
| 1 | 中文听感可接受（melo） | **PASS** — 用户原话："比前一个中文更好"；混合句"效果不是很好"（记入 Known Limitations）。试听样本 `/tmp/aleph_spike_melo_sid0_{zh,en,mixed}.wav` |
| 4 | sherpa-rs 覆盖度（melo / VitsTts） | 机器侧通过：加载/合成/STT 闭环全通。**已知缺口**：`rule_fsts`（数字/日期文本归一化）经 sherpa-rs 0.6.8 传入会 use-after-free（库 bug，详见 API 形状 §10），Task 6 不可使用 |

### 选定默认音色（终局）

- **默认 TTS = MeloTTS-zh_en，默认音色 = melo sid0**。melo 是单 speaker（运行时实证：`This model contains only 1 speakers. sid should be in the range [0, 0]`，传 sid=1 自动回落 0 → sid0/sid1 wav 内容相同）→ `tts_voice` 对 melo 退化为常量 0
- **Kokoro = 可选音色**（如 `voice="kokoro-en"`，默认 sid0=af_maple 推断，英文较好；ModelManager 惰性下载，不用不下）。sid 映射参考: voices.bin 53,790,720 B ÷ 522,240 B/voice = **103 voices** (sid 0..102)；官方 = 100 中文 (zf_*/zm_*) + 3 英文 (af_maple/af_sol/bf_vale)；闭环旁证 **sid 0/1 = 英文音色，sid 50/100 = 中文音色**
- **无自动语言路由**（产品决策）——按句子语言切引擎救不了混合句，且违背简单性

---

## 实测数据

### 模型包（Step 3，github 源直连成功，URL 与计划一致无 404）

| 包 | tarball | 字节 | sha256 |
|---|---|---|---|
| SenseVoice-small | `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2` | 1,047,870,769 (~999 MB) | `f6b2a72ebcb1ac7a764d4cfccd886e6bcb2a95c4657c2199d0ba95ed4b9ea71a` |
| Kokoro v1.1-zh | `kokoro-multi-lang-v1_1.tar.bz2` | 364,816,464 (~348 MB) | `a3f4c73d043860e3fd2e5b06f36795eb81de0fc8e8de6df703245edddd87dbad` |
| MeloTTS-zh_en（备胎，第二轮） | `vits-melo-tts-zh_en.tar.bz2` | 167,006,755 (~159 MB) | `e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514` |

URL（github 直连成功）: `https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2`（hf-mirror 备源: `https://hf-mirror.com/csukuangfj/vits-melo-tts-zh_en/resolve/main/vits-melo-tts-zh_en.tar.bz2`）

> 计划估计 SenseVoice 包 ~230MB，实际 ~1GB——tarball 同时含 fp32 `model.onnx` (938MB) 与 `model.int8.onnx` (239MB)，运行时只用 int8。Task 5 ModelManager 可考虑解包后删 fp32 省盘。

**kokoro-v1.1-zh 实际文件清单**（与计划预期一致 + 额外 fst/lexicon-gb）:

```
LICENSE  README.md  date-zh.fst  dict/  espeak-ng-data/
lexicon-gb-en.txt  lexicon-us-en.txt  lexicon-zh.txt
model.onnx (325MB)  number-zh.fst  phone-zh.fst  tokens.txt  voices.bin (53MB)
```

**sense-voice-small 实际文件清单**: `LICENSE README.md export-onnx.py model.int8.onnx (239MB) model.onnx (938MB) test_wavs/ tokens.txt`

**vits-melo-tts-zh_en 实际文件清单**（解包到 `~/.aleph/models/voice/vits-melo-tts-zh_en/`）:

```
LICENSE  README.md  date.fst  dict/  lexicon.txt (6.8MB)
model.int8.onnx (133 B — git-lfs 指针残留，非真模型，不可用)
model.onnx (170,429,550 B = 真运行时模型)
new_heteronym.fst  number.fst  phone.fst  tokens.txt (655 B)
```

> ⚠️ Task 5 manifest 换型注意: melo 的 marker 必须是 **`model.onnx`**（int8 文件是 133 字节 LFS 指针残留）；无 espeak-ng-data、无 voices.bin（VITS 单 speaker，音色嵌在模型内）。

### TTS（Kokoro，num_threads=1 默认）

加载: **3.40 s**。合成（24 kHz 输出）:

| sid | zh | en | mixed |
|---|---|---|---|
| 0 | 5013 ms (3.75s 音频, RTF≈1.34*) | 2098 ms (RTF≈0.78) | 5452 ms (RTF≈0.93) |
| 1 | 3105 ms (RTF≈0.85) | 2293 ms (RTF≈0.77) | 5525 ms (RTF≈0.89) |
| 50 | 4000 ms (RTF≈0.95) | 2580 ms (RTF≈0.87) | 7169 ms (RTF≈1.00) |
| 100 | 3419 ms (RTF≈0.82) | 2194 ms (RTF≈0.75) | 5502 ms (RTF≈0.84) |

\* 首句含一次性 warmup。整体 **RTF ≈ 0.8–1.0（单线程）**——接近实时但无余量；Task 6 建议 `onnx_config.num_threads` 调到 2-4 压 RTF。
已知噪音: 合成期 stderr 打 `lexicon.cc Unknown token: ❓`（lexicon 未覆盖个别符号，输出音频不受影响）。

### TTS 备胎（MeloTTS-zh_en / VitsTts，num_threads=1 默认，第二轮）

加载: **1.31 s**（vs Kokoro 3.40 s）。合成（**44100 Hz** 输出，单 speaker sid=0）:

| case | 耗时 | 音频时长 | RTF |
|---|---|---|---|
| zh | 2085 ms | 3.10 s | **0.67** |
| en | 2292 ms | 3.52 s | **0.65** |
| mixed | 4320 ms | 7.13 s | **0.61** |

- **RTF ≈ 0.61–0.68（单线程）** — 显著优于 Kokoro 的 0.8–1.0，有实时余量
- **内存**: baseline 9.0 → 加载后 **898.4 MB**（引擎占用 ~890 MB，vs Kokoro ~1706 MB，**约一半**）；合成三句后 976.5 MB
- 单 speaker 实证: 传 sid=1 时 sherpa-onnx 打 `contains only 1 speakers. sid should be in range [0, 0]` 并回落 sid=0（sid1 wav 与 sid0 内容相同）
- 未启用 rule_fsts（date/number 归一化）——sherpa-rs 0.6.8 传入即 UAF（见 API §10）；测试句无数字不受影响，**生产中含数字/日期的回复将按字面 token 念**，Task 6 需对策（上游修复 / fork patch / Rust 侧文本预归一化）

### STT 闭环（melo wav → SenseVoice，第二轮）

| 输入 | 耗时 | lang | 文本 |
|---|---|---|---|
| melo zh | 341 ms | `<\|zh\|>` | "你好，我是捞老。本地语音引擎已经就绪。"（**句体全对**；"Aleph"→"捞老" 专名失真，melo 在中文句内念英文词偏中式） |
| melo en | 371 ms | `<\|en\|>` | "Tass the local attached to speech engine."（**明显劣于 Kokoro 英文**——melo 英文能力弱是已知特性） |
| melo mixed | 739 ms | `<\|zh\|>` | "我们用SHLRP光anex QQRO模型首包延迟firs packet latency也很关键。"（中文部分对，嵌入英文术语失真） |

**机器侧旁证**: melo 中文句体回转干净（与 Kokoro 中文音色相当），**英文是 melo 的短板**——与 Kokoro 恰好互补（Kokoro: 英文 PASS / 中文 FAIL）。用户二轮裁决采纳 melo 为默认（中文 PASS），Kokoro 留作英文可选音色（终局见文首）。

### STT（SenseVoice int8，num_threads=1 默认）

加载: **1.02–1.25 s**。转写（输入 24kHz wav，sherpa 内部自动重采样到 16k）:

| 输入 | 耗时 | lang | 文本 |
|---|---|---|---|
| sid0 zh | 446 ms (RTF≈0.12) | `<\|zh\|>` | "你好，我是alff范diinin Qing已ing周 X。"（差——**TTS 侧问题**：sid0 是英文音色念中文） |
| sid0 en | 304 ms (RTF≈0.11) | `<\|en\|>` | "Hello, this is the local Tex to Spch engine." |
| sid0 mixed | 659 ms (RTF≈0.11) | `<\|en\|>` | 严重劣化（同因） |
| sid50 zh | 470 ms | `<\|zh\|>` | "你好，我是阿le。本地语音引擎已经教绪。"（**近乎全对**，"Aleph"→"阿le" 为专名预期） |
| sid50 en | 341 ms | `<\|en\|>` | "Hello, this is the local text speech engine." |
| sid50 mixed | 780 ms | `<\|zh\|>` | "我们用ship onsp coral模型，受包延迟first packet latency很关键。"（中英混合可用，专名 sherpa-onnx/Kokoro 失真） |
| sid100 zh | 456 ms | `<\|zh\|>` | "你好，我是alle。本地语音引情已经就绪。" |

**闭环结论（机器侧）**: 中文音色 wav → STT 还原度高；劣化样本全部归因 TTS 英文音色念中文。**真人录音准确度检查无法由 agent 执行（无法录音），随 Step 8 人工门由用户完成。**

### 内存（mem_spike，双周期加强版）

```
baseline:             9.0 MB
cycle 1 tts loaded: 1714.6 MB
cycle 1 tts dropped: 995.8 MB   (drop 后 sleep 2s)
cycle 2 tts loaded: 1708.0 MB   ← 不叠加，复用缓存页
cycle 2 tts dropped: 1009.0 MB
```

- drop 每周期确定性释放 ~710 MB；第二次加载 **不再增长**（1714.6 → 1708.0）→ **无泄漏**，残留 ~1.0 GB 是 allocator/onnxruntime arena 页缓存。
- **严格判据（dropped − baseline < 50 MB）按 RSS 未达标**（差值 ~996 MB）。
- 工程含义: 引擎级 idle-unload 只能还 ~40% RSS；**完全归还 OS 依赖 sidecar 深闲自退（进程退出，Task 8 已设计）**——架构上该门可由"unload + 深闲自退"组合满足，是否判 PASS 留给控制者/用户。

---

## sherpa-rs 0.6.8 真实 API 形状（Task 6 直接消费）

源码核对: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-rs-0.6.8/src/`

### 终表（Task 6 引擎层照此实现）

| 引擎 | 类型 | 构造 | 推理 | 必设字段（derive(Default) 陷阱） | Send/Sync |
|---|---|---|---|---|---|
| TTS 默认（melo） | `sherpa_rs::tts::VitsTts` | `new(VitsTtsConfig) -> Self`（无 Result，坏路径推迟到 create） | `create(&mut, text, sid=0, speed) -> eyre::Result<TtsAudio>`（44.1 kHz） | `model/lexicon/tokens/dict_dir` + **`noise_scale=0.667, noise_scale_w=0.8, length_scale=1.0, silence_scale=0.2`**（默认全 0.0 不可用）；data_dir 留空；**rule_fsts 必须留空（UAF，§10），数字/日期由 Rust 侧文本预归一化补位** | Send + Sync |
| TTS 可选（kokoro-en） | `sherpa_rs::tts::KokoroTts` | `new(KokoroTtsConfig) -> Self` | `create(&mut, text, sid, speed) -> eyre::Result<TtsAudio>`（24 kHz） | `model/voices/tokens/data_dir/lexicon/dict_dir` + **`length_scale=1.0`**（默认 0.0）；lang 留空；common_config 留默认（rule_fsts 同禁） | Send + Sync |
| STT | `sherpa_rs::sense_voice::SenseVoiceRecognizer` | `new(SenseVoiceConfig) -> eyre::Result<Self>` | `transcribe(&mut, sample_rate: u32, &[f32]) -> OfflineRecognizerResult { lang, text, timestamps, tokens }` | `model`(int8)/`tokens`；language="auto"/use_itn=true 已是 Default | Send + Sync |

通用：错误均为 `eyre::Result`（不能直接 `?` 进 anyhow，须 map_err 或自定义错误承接）；`OnnxConfig { provider:"cpu", num_threads:1, debug }` 建议 num_threads 2-4 压 RTF。

### 与计划假设的差异（必读）

1. **STT 类型名**: 计划 `sherpa_rs::sense_voice::SenseVoice` → 实际 **`SenseVoiceRecognizer`**。
2. **`transcribe` 签名**: 计划 `transcribe(rate, samples: Vec<f32>)` → 实际 **`transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> SenseVoiceRecognizerResult`**（借用切片；返回结构体非 Result/Option）。返回类型是 `sherpa_rs::OfflineRecognizerResult` 的别名: `{ lang: String, text: String, timestamps: Vec<f32>, tokens: Vec<String> }`（lang 形如 `<|zh|>`）。
3. **错误类型是 `eyre::Result` 非 anyhow**: `SenseVoiceRecognizer::new` 与 `KokoroTts::create` 都返回 eyre；`eyre::Report` 不实现 `std::error::Error`，**在 anyhow 函数里不能直接 `?`**，须 `.map_err(|e| anyhow::anyhow!("{e}"))`（spike 例子即如此）。Task 6 引擎层建议直接以 String/自定义错误承接。
4. **`KokoroTtsConfig` 多三个字段**: 计划列的 `model/voices/tokens/data_dir/lexicon/dict_dir/length_scale` 全部存在且同名；额外还有 **`lang: String`**（多语模型留空即可）、**`onnx_config: OnnxConfig { provider, debug, num_threads }`**（默认 cpu/1 线程）、**`common_config: CommonTtsConfig { rule_fars, rule_fsts, max_num_sentences, silence_scale }`**。
5. **`KokoroTtsConfig` 是 `#[derive(Default)]`** → `length_scale` 默认 **0.0**，必须显式设 1.0（计划代码恰好显式设了，无须改）。
6. **`KokoroTts::new(config) -> Self`**（无 Result）——模型路径错误不会在构造时报错，空指针推迟到 `create()` 才以 `"audio is null"` 暴露。Task 6 加载层应先自查文件存在性。
7. **`SenseVoiceConfig`** 实际形状: `{ model, language, use_itn, provider: Option<String>, num_threads: Option<i32>, debug: bool, tokens }`，有手写 Default（language="auto", use_itn=true, num_threads=Some(1)）。计划的字段子集全部存在。
8. **`TtsAudio`**: `{ samples: Vec<f32>, sample_rate: u32, duration: i32 }` — 与计划假设一致（实测 24000 Hz）。
9. **`create` 签名**: `create(&mut self, text: &str, sid: i32, speed: f32) -> eyre::Result<TtsAudio>` — 与计划一致（除错误类型）。
10. **🐛 sherpa-rs 0.6.8 真 bug — `CommonTtsConfig::to_raw()` 的 rule_fsts/rule_fars 是 use-after-free**: `tts_config.rule_fars.map(|v| v.as_ptr())`（kokoro.rs/vits.rs 同模式）在 `map` 闭包内取完裸指针即 drop CString → 传给 C API 的是**悬垂指针**。实测（melo + rule_fsts="date.fst,number.fst"）: C 侧读到的字符串变成 model.onnx 路径，`FstHeader::Read: Bad FST header` 直接 abort。**任何非空 rule_fsts/rule_fars 都不可经 sherpa-rs 0.6.8 传入**（留空 → None → null 安全，Kokoro 第一轮即如此幸免）。Task 6 对策候选：上游报修/fork patch（`[patch.crates-io]`）、或 Rust 侧文本预归一化（数字→中文读法）替代 FST。
11. **MeloTTS 走 `sherpa_rs::tts::VitsTts`**（melo 是 VITS 架构，无专属类型）。**`VitsTtsConfig`** 完整形状: `{ model, lexicon, dict_dir, tokens, data_dir, length_scale, noise_scale, noise_scale_w, silence_scale, onnx_config: OnnxConfig, tts_config: CommonTtsConfig }`，`#[derive(Default)]` → **noise_scale/noise_scale_w/length_scale 全默认 0.0**，必须显式设 sherpa-onnx 规范值 `0.667 / 0.8 / 1.0`（melo spike 即如此；silence_scale 取 0.2）。melo 实际传参: model=model.onnx, lexicon=lexicon.txt, tokens=tokens.txt, dict_dir=dict（jieba），data_dir 留空（无 espeak）。`new() -> Self`（同 Kokoro 无 Result）、`create` 签名同 §9。`VitsTts` 同样 **Send + Sync**（crate 内 unsafe impl）。

### Send/Sync 探针（Task 6 要用）

`fn is_send<T: Send>() {}` / `is_sync` 编译探针（`aleph-voice/examples/mem_spike.rs`）通过：

- `sherpa_rs::tts::KokoroTts`: **Send + Sync**（crate 内 `unsafe impl Send/Sync`）
- `sherpa_rs::sense_voice::SenseVoiceRecognizer`: **Send + Sync**（同上）

### 依赖/构建事实

- `sherpa-rs 0.6.8` default features = `["download-binaries", "tts"]` → 预编译 sherpa-onnx 静态库，**无 cmake 依赖**（本机确认：无 cmake 仍 `cargo check`/`build --release` 全过）。
- `cargo check -p aleph-voice`（全 features）与 `--no-default-features` 均通过；manifest 无 `<spike-measured>` 残留。

---

## 其他记录

- 下载脚本落在 **`scripts/voice_models_fetch.sh`**（小写，跟随仓库既有 `scripts/` 目录；计划写 `Scripts/`，macOS 大小写不敏感会合并，刻意取小写）。github 直连 ~3.8 MB/s，两包共 ~6 分钟。
- `stt_spike` 增加了可选 CLI 参数（wav 前缀，默认 `tts_sid0`；如 `tts_sid50`/`melo_sid0`）用于对不同 TTS 产出 wav 闭环——计划之外的最小增强。
- `mem_spike` 从单周期扩为双周期 + drop 后 2s 静置——为区分"泄漏 vs allocator 缓存"，结论见上。
- 第二轮新增 `aleph-voice/examples/melo_spike.rs`（MeloTTS/VitsTts 备胎验证，含 RSS 测量与单 speaker 探测）。
- 二轮试听 wav: `/tmp/aleph_spike_melo_sid0_{zh,en,mixed}.wav`（sid1 三个为 sid0 重复，可忽略）；第一轮 Kokoro wav 仍在 `/tmp/aleph_spike_tts_sid{0,1,50,100}_{zh,en,mixed}.wav` 可对照。
