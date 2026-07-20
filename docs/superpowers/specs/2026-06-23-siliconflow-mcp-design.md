# SiliconFlow MCP — Aleph 自建官方 MCP(Aleph-mcp 仓库 #1)

**Date:** 2026-06-23
**Status:** Approved (design); pending implementation plan
**Scope boundary:** Spans two repos.
- **Aleph-mcp (new)** — `rootazero/Aleph-mcp`, monorepo 容器;本期落地第一个 server `siliconflow/`(Python, uvx-runnable)。
- **Aleph (main)** — 唯一改动是 `src/mcp/presets/catalog.json` 加一条 `siliconflow` 预设 + `src/mcp/presets/mod.rs` 解析测试 id 断言。**无 submodule / 无 embed / 无 extractor 改动**(区别于 skills/plugins,见 §1)。

兄弟仓 `Aleph-skills` / `Aleph-plugins` 已存在,本期不动。

---

## 1. 背景与边界(写进 Aleph-mcp README,作为仓库宪法)

### 1.1 存在理由

> **Aleph-mcp 只补"官方缺位"的空白。** 有官方 MCP 的(如火山 veImageX)→ Aleph `catalog.json` 直接指向官方上游源(`uvx --from git+https://github.com/volcengine/mcp-server#...`),不重复造轮。没有官方 MCP 的(如 SiliconFlow)→ Aleph 自建一个"对 Aleph 而言官方"的版本,放进 Aleph-mcp。

SiliconFlow(硅基流动)是重要的集成模型提供商,但官方未发布 MCP。社区有 `stevefordev/siliconflow-mcp`(MIT),但 (a) 是社区版非官方,(b) 端点/模型已部分过时(见 §3 校对表)。Aleph 自建 = 深度参考其 API 知识 + 严格按官方文档校对 + 应用 Aleph 工程约定(cn-native 默认、优雅降级、面向 LLM 的结构化返回)。

### 1.2 与 skills/plugins 承载方式的区别(为什么 Aleph-mcp 更轻)

| 维度 | Aleph-skills / Aleph-plugins | Aleph-mcp(本仓) |
|------|------------------------------|------------------|
| 交付物 | 内容(被 `include_dir!` 嵌入二进制) | 独立可执行进程(Python) |
| 安装 | 首启 clone + 离线 embed fallback + Hub catalog 同步 | 运行时 `uvx` 从 git 按需拉起,**不嵌入** |
| Aleph 接入点 | `src/bundled/` extractor + 提交 submodule + Hub seed | 仅 `src/mcp/presets/catalog.json` 一条预设 |
| 升级 | CalVer 版本门 re-extract / explicit pull | `uvx` 每次按 git ref 拉取(可固定 commit/tag) |

→ 本期对主仓的改动面 = **一条 JSON 预设 + 一行测试断言**,无 Rust 逻辑改动。

### 1.3 与 Aleph core 的职责切分(R3 / R7 / R8)

SiliconFlow 的 **chat / embedding / rerank 已被 Aleph core 覆盖**(`src/memory/embedding_resolver.rs`、`src/memory/rerank/siliconflow.rs`,后者用 `api.siliconflow.cn`)。本 MCP **只做 core 做不了的:媒体生成(图 / 视频 / 语音)**。做 chat/embed/rerank = 违 R7(越俎代庖)。

---

## 2. Locked Decisions

- **D1. 代码来源 = 清爽重写。** 深度参考社区版的 API 知识,用 Aleph 自己的代码重写;按 §3 修正所有过时端点/参数。README 致谢社区参考项与官方文档(非许可义务,工程礼仪)。
- **D2. 仓库结构 = monorepo 容器。** `Aleph-mcp/` 下每个 MCP 各一子目录;`siliconflow/` 为第一个。未来自建 MCP 平级加目录。
- **D3. 分发 = uvx 从 git 源装。** `catalog.json` 指向 `git+https://github.com/rootazero/Aleph-mcp#subdirectory=siliconflow`,与 veimagex 预设同套路。推送后即可用,无需 PyPI 账号。PyPI 发布列为后期可选打磨(§9)。
- **D4. 语言/运行时 = Python + 官方 `mcp` SDK(FastMCP),`uvx` 运行。** 与现有 `minimax`/`veimagex` 预设同构。
- **D5. 端点默认 `.cn`,env 可改。** 默认 `https://api.siliconflow.cn/v1`(对齐 Aleph rerank + cn-native 定位);海外用户 `SILICONFLOW_API_BASE` 改 `.com`。两者均为官方有效镜像。
- **D6. 工具面 media-only。** 图(生成+编辑)、视频(生成+低层异步)、语音(TTS)、`list_models`、`get_user_info`。显式排除 chat/embed/rerank。
- **D7. 不硬编码权威模型清单。** 模型名会过时(社区版即如此)。给合理默认 + 文档标注"截至撰写时" + 靠 `list_models` 动态发现。
- **D8. 仓库创建 = fresh snapshot,owner `rootazero`,public。** `gh repo create rootazero/Aleph-mcp --public --source=. --push`(沿用 skills/plugins 的 D4 约定),不保留 history。**对外动作,执行前向用户确认。**

---

## 3. 官方文档校对结果(本设计的核心价值)

> 校对源:`https://api-docs.siliconflow.cn/docs/api/*`(2026-06-23 抓取)。鉴权一律 `Authorization: Bearer <SILICONFLOW_API_KEY>`。base = `https://api.siliconflow.cn/v1`(默认)。

### 3.1 图片生成 / 编辑 — `POST /v1/images/generations`(生成与编辑**同端点**)

请求体:

| 参数 | 类型 | 必填 | 说明 / 取值 |
|------|------|------|------------|
| `model` | string | ✅ | 如 `Kwai-Kolors/Kolors`、`Qwen/Qwen-Image-Edit-2509` |
| `prompt` | string | ✅ | 文本提示 |
| `negative_prompt` | string | ❌ | 排除内容 |
| `image_size` | string | 条件 | `"宽x高"`;多数模型必填,Qwen-Image-Edit 系列除外 |
| `batch_size` | int | ❌ | 1–4(默认 1) |
| `seed` | int | ❌ | ≤ 9999999999 |
| `num_inference_steps` | int | ❌ | 1–100(默认 20) |
| `guidance_scale` | number | ❌ | 0–20(默认 7.5,**Kolors 专用**) |
| `cfg` | number | ❌ | 0.1–20(**Qwen 专用**) |
| `image` / `image2` / `image3` | string | ❌ | base64 或 URL;`image` 触发图生图/编辑;`image2/3` 仅 Qwen-Image-Edit-2509 多图 |

响应:`{ "images": [{"url": "..."}], "timings": {"inference": 0.1}, "seed": 0 }`。**图片 URL 1 小时过期** → 强化本地落盘设计。

→ **校对修正**:社区版把"编辑"当独立端点,实为同端点 + `image` 参数。我方保留 `generate_image` / `edit_image` 两个工具(LLM 友好),但底层同一调用路径,`edit_image` 仅置 `image`。新增 `cfg`/`image2`/`image3` 支持。

### 3.2 视频提交 — `POST /v1/video/submit`

| 参数 | 类型 | 必填 | 取值 |
|------|------|------|------|
| `model` | string | ✅ | `Wan-AI/Wan2.2-T2V-A14B`(文生视频)、`Wan-AI/Wan2.2-I2V-A14B`(图生视频) |
| `prompt` | string | ✅ | — |
| `image_size` | string | ✅ | **必填**,仅 `1280x720` / `720x1280` / `960x960` |
| `image` | string | 条件 | I2V 模型必填(base64 或 URL) |
| `negative_prompt` | string | ❌ | — |
| `seed` | int | ❌ | — |

响应:`{ "requestId": "..." }`。

→ **校对修正**:社区版列 Wan2.1 旧型号且 `image_size` 当可选;实为 Wan2.2 + `image_size` 必填白名单。

### 3.3 视频状态 — `POST /v1/video/status`

请求体:`{ "requestId": "..." }`。
响应:`{ "status": "Succeed"|"InQueue"|"InProgress"|"Failed", "reason": "...", "results": { "videos": [{"url": "..."}], "timings": {...}, "seed": 0 } }`。

→ **校对修正**:状态值是 **`Succeed`**(非 Succeeded);URL 在 `results.videos[].url`;`reason` 仅 Failed 时出现。轮询需精确匹配这些字符串。

### 3.4 语音合成(TTS)— `POST /v1/audio/speech`

| 参数 | 类型 | 必填 | 取值 |
|------|------|------|------|
| `model` | string | ✅ | 如 `FunAudioLLM/CosyVoice2-0.5B`、`fnlp/MOSS-TTSD-v0.5` |
| `input` | string | ✅ | 1–128000 字;MOSS-TTSD 对话用 `[S1]`/`[S2]` 标记 |
| `voice` | string | ❌ | 格式 `"model:voice_id"`(**模型作用域**,如 `FunAudioLLM/CosyVoice2-0.5B:alex`) |
| `response_format` | string | ❌ | `mp3`(默认)/`opus`/`wav`/`pcm` |
| `sample_rate` | number | ❌ | 随 format 而异 |
| `speed` | number | ❌ | 0.25–4.0(默认 1.0) |
| `gain` | number | ❌ | -10.0–10.0(默认 0.0) |
| `stream` | bool | ❌ | 我方固定 `false`(MCP 一次性返回文件) |

响应:**二进制音频**(非 JSON),格式 = `response_format`。

→ **校对修正**:社区版列 fish-speech/IndexTTS 且未体现 voice 的模型作用域;voice 必须是 `model:voice_id`。

### 3.5 模型列表 — `GET /v1/models?type=&sub_type=`

- `type`:`text`/`image`/`audio`/`video`
- `sub_type`:`chat`/`embedding`/`reranker`/`text-to-image`/`image-to-image`/`speech-to-text`/`text-to-video`

响应:OpenAI 风格 `{ "object": "list", "data": [{"id","object","created","owned_by"}] }`。

### 3.6 用户信息 — `GET /v1/user/info`

响应 `data`:`{ id, name, image, email, isAdmin, balance, status, introduction, role, chargeBalance, totalBalance }`(余额三项:`balance` 赠送余额 / `chargeBalance` 充值余额 / `totalBalance` 总额)。实际外层信封(`code`/`status`/`message`)在实现时按真实响应解析(TDD)。

---

## 4. 工具规格(对 LLM 暴露)

每个工具:校验入参 → 调端点 → (媒体)落盘 → 返回**结构化文本**(模型名 + 本地绝对路径 + 远程 URL + 关键参数)。

| 工具 | 端点 | 关键入参 | 返回 |
|------|------|---------|------|
| `generate_image` | `POST /v1/images/generations` | model, prompt, aspect_ratio→image_size, negative_prompt, batch_size, seed, num_inference_steps, guidance_scale/cfg | 每张图:本地路径 + URL + seed |
| `edit_image` | 同上(置 `image`) | model, prompt, image(路径/URL→base64 或直传 URL), image2/3(可选) | 同上 |
| `generate_video` | submit→轮询 status | model, prompt, image_size(白名单), image(I2V), negative_prompt, seed | 完成后:本地路径 + URL;含轮询超时上限 |
| `submit_video_generation` | `POST /v1/video/submit` | 同上 | `requestId` |
| `get_video_status` | `POST /v1/video/status` | request_id | status + (成功时)本地路径/URL |
| `generate_speech` | `POST /v1/audio/speech` | model, input, voice, response_format, speed, gain | 本地音频路径 |
| `list_models` | `GET /v1/models` | type, sub_type | 模型 id 列表 |
| `get_user_info` | `GET /v1/user/info` | — | 余额(总/充值/赠送)+ 账户信息 |

**aspect_ratio 便利映射**(沿用社区版思路,内部转 `image_size`):图片 `1:1→1024x1024, 3:4→768x1024, 4:3→1024x768, 9:16→576x1024, 16:9→1024x576`;视频走官方 3 值白名单 `16:9→1280x720, 9:16→720x1280, 1:1→960x960`。两表为纯函数,单测覆盖。

---

## 5. 配置(env)

| env | 必填 | 默认 | 说明 |
|-----|------|------|------|
| `SILICONFLOW_API_KEY` | ✅ | — | API Key(secret);缺失则启动报错(P7) |
| `SILICONFLOW_API_BASE` | ❌ | `https://api.siliconflow.cn/v1` | 海外可改 `.com` |
| `SILICONFLOW_IMAGE_DIR` | ❌ | — | 图/视频落盘目录;未设则只返回远程 URL |
| `SILICONFLOW_AUDIO_DIR` | ❌ | 回退 IMAGE_DIR | 音频落盘目录 |

获取 Key:`https://cloud.siliconflow.cn/account/ak`。

---

## 6. 错误处理(P7 防御性设计)

- 启动校验 `SILICONFLOW_API_KEY` 缺失 → 明确错误,不静默。
- API 4xx/5xx → 解析 SiliconFlow 错误体,返回人类可读消息,**不回显 key**。
- 媒体 URL 短时过期(图 1h / 视频更短)→ 立即下载落盘;下载失败则**优雅降级返回远程 URL**,不丢结果。
- 视频轮询带超时上限 + 退避,超时返回 `requestId` 让用户/LLM 用 `get_video_status` 续查。
- 输入校验:aspect_ratio / image_size 白名单、`batch_size`/`seed`/`speed` 范围、本地图片路径存在性、模型名非空。

---

## 7. 仓库结构(Aleph-mcp = monorepo 容器)

```
Aleph-mcp/
├── README.md            # 仓库宪法(§1.1 边界)+ 各 MCP 索引 + 致谢
├── LICENSE              # MIT
├── .gitignore           # Python(__pycache__, .venv, *.egg-info, .env)
└── siliconflow/         # 第一个 MCP(uv 包)
    ├── pyproject.toml   # name=aleph-siliconflow-mcp; [project.scripts] aleph-siliconflow-mcp=...:main
    ├── README.md        # 安装/配置/工具文档(中英)
    ├── .env.example
    └── src/aleph_siliconflow_mcp/
        ├── __init__.py
        ├── server.py    # FastMCP 注册各工具
        ├── main.py      # 入口 main()
        ├── client.py    # httpx client + base/鉴权 + 落盘(原 common.py 职责)
        ├── images.py    # generate_image / edit_image
        ├── videos.py    # submit / status / generate(轮询)
        ├── audio.py     # generate_speech
        ├── user.py      # get_user_info / list_models
        └── ratios.py    # aspect_ratio→image_size 纯映射(单测)
    └── tests/
        ├── test_ratios.py
        ├── test_payloads.py   # 各端点 payload 构建
        └── test_parsing.py    # 响应解析(images/video status/user info)
```

依赖:`httpx`、`mcp`、`python-dotenv`(与参考项一致,版本取当前)。Python ≥ 3.10。

---

## 8. Aleph 接入(主仓唯一改动)

`src/mcp/presets/catalog.json` 追加:

```json
{
  "id": "siliconflow",
  "name": "硅基流动 SiliconFlow",
  "category": "model-provider",
  "description": "文生图 / 图生视频 / 语音合成(Aleph 自建官方 MCP)。",
  "vendor": "硅基流动 SiliconFlow",
  "official": true,
  "reachability": "cn-native",
  "transports": [
    { "kind": "stdio", "command": "uvx",
      "args": ["--from", "git+https://github.com/rootazero/Aleph-mcp#subdirectory=siliconflow", "aleph-siliconflow-mcp"],
      "requires_runtime": "python" }
  ],
  "required_env": [
    { "key": "SILICONFLOW_API_KEY", "label": "SiliconFlow API Key", "description": "平台 API Key", "secret": true, "required": true, "how_to_get_url": "https://cloud.siliconflow.cn/account/ak" },
    { "key": "SILICONFLOW_API_BASE", "label": "API Base", "description": "区域接入点", "secret": false, "required": false, "default": "https://api.siliconflow.cn/v1" },
    { "key": "SILICONFLOW_IMAGE_DIR", "label": "图片/视频保存目录", "description": "本地落盘目录(留空则返回远程 URL)", "secret": false, "required": false },
    { "key": "SILICONFLOW_AUDIO_DIR", "label": "音频保存目录", "description": "默认回退到图片目录", "secret": false, "required": false }
  ],
  "tags": ["image", "video", "tts", "model-provider"]
}
```

`src/mcp/presets/mod.rs` 测试 `bundled_catalog_parses_and_has_first_batch` 的 id 列表加 `"siliconflow"`。

> 注:`required_env` 含 `default` 的字段(`SILICONFLOW_API_BASE`)沿用 minimax `MINIMAX_API_HOST` 的既有模式;但 minimax 把它标 `required:true`。我方标 `required:false`(确有默认值,用户不填也能装),与 `PresetEnvVar::required` 语义(§mod.rs:90 "Must be present unless default is set")一致 —— 实现时确认 `missing_required_env` 逻辑对 `required:false` 的处理符合预期。

---

## 9. 测试

- **Python(pytest,不打活 API):** aspect_ratio 映射;各端点 payload 构建;响应解析(`images[].url` / video `status`+`results.videos[].url` / user info 余额);落盘路径生成 + 扩展名推断;缺 key 启动报错。一个 `@pytest.mark.skipif(no key)` 的 live smoke test(可选)。
- **Rust(节制 cargo):** 复用 `bundled_catalog_parses_and_has_first_batch`,加 `"siliconflow"` 断言 → 一次 `cargo test -p alephcore --lib presets`。
- **手动:** Panel 设置里出现 SiliconFlow 预设 → 填 key → 一键装 → 跑 `generate_image` 验证落盘 + 返回路径。

---

## 10. 交付顺序

1. 本地写 `Aleph-mcp/siliconflow` 全套代码 + pytest(纯本地,零 cargo)。
2. 本地 `pytest` 绿;`uvx --from <local path>` 冒烟跑通工具列表。
3. `gh repo create rootazero/Aleph-mcp --public --source=. --push`(**对外动作,执行前确认**)。
4. 主仓 `catalog.json` + `mod.rs` 测试断言 → 一次 `cargo test -p alephcore --lib presets`。
5. (可选/后期)PyPI 发布 + GitHub Actions CI(lint + pytest)。

---

## 11. 风险与权衡

- **模型名漂移**:SiliconFlow 频繁上新/下线模型。缓解:D7 不硬编码权威清单 + `list_models` 动态发现 + 文档标注时间。
- **媒体 URL 短时过期**:必须即时下载;未配落盘目录时返回的 URL 可能很快失效 —— 文档明确提示用户配 `SILICONFLOW_IMAGE_DIR`。
- **`uvx` 首次拉取延迟**:从 git 装首次需 clone + 建环境;veimagex 已是同模式,用户已接受。
- **git ref 漂移**:`#subdirectory=siliconflow` 默认跟随默认分支 HEAD。如需可复现,后续可在 catalog 固定 `@<tag>`(fast-follow)。
- **官方未来发布 MCP**:若 SiliconFlow 出官方 MCP,按 §1.1 准则 catalog 改指官方源、Aleph-mcp 的 siliconflow 降级为历史/废弃。

---

## 12. Open / Fast-follow(本期外)

- PyPI 发布(`uvx aleph-siliconflow-mcp` 免 git)。
- catalog 预设固定 tag 以求可复现。
- GitHub Actions:lint(ruff)+ pytest。
- 后续自建 MCP 进 Aleph-mcp(平级目录)。

---

## 13. 致谢与参考

- API 知识深度参考社区项目 `stevefordev/siliconflow-mcp`(MIT);本仓为清爽重写,端点按官方文档校对。
- 官方文档:`https://api-docs.siliconflow.cn/docs/api/*`、`https://docs.siliconflow.com/`。
- 接入范式参考 Aleph 现有预设 `minimax` / `volcengine-veimagex`(`src/mcp/presets/catalog.json`)。
