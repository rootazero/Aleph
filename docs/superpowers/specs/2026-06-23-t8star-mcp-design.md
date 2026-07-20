# T8star MCP — Aleph 自建官方 MCP(Aleph-mcp 仓库 #2)

**Date:** 2026-06-23
**Status:** Approved (design); pending implementation plan
**Scope boundary:** Spans two repos.
- **Aleph-mcp (existing)** — `rootazero/Aleph-mcp`,monorepo 容器;本期落地第二个 server `t8star/`(**TypeScript, npx-runnable**),与已有 `siliconflow/`(Python)平级。仓库暂时 Python + TS 并存;硅基流动 TS 重写是独立后续任务(用户 2026-06-23 决策:Aleph-mcp 全栈收敛到 TS)。
- **Aleph (main)** — 唯一改动是 `src/mcp/presets/catalog.json` 加一条 `t8star` 预设(第 6 条) + `src/mcp/presets/mod.rs` 解析测试 id 断言由 5 扩到 6。**无 submodule / 无 embed / 无 Rust 逻辑改动**。

本设计**沿用 `siliconflow/` 的工程范式**(纯函数 + 薄 client + 一键发布;见同目录 `2026-06-23-siliconflow-mcp-design.md`),但语言/运行时按用户 2026-06-23 决策改为 **TS + npx**(MCP 顺应 TypeScript 主流;npm/npx 分发,对齐 catalog 里 context7),即从 Python/FastMCP/uvx/PyPI **平移**到 TS/`@modelcontextprotocol/sdk`/npx/npm。API 校对层(§3)与语言无关,不变。

---

## 1. 背景与边界

### 1.1 存在理由(承接 Aleph-mcp 仓库宪法)

> **Aleph-mcp 只补「官方缺位」的空白。** t8star(贞贞的AI工坊,`ai.t8star.org`)是大型综合模型聚合中转(830+ 模型,涵盖 OpenAI/Claude/Gemini/Flux/Sora2/Seedance/Veo/Kling/Suno…),但**无官方 MCP**。→ Aleph 自建一个「对 Aleph 而言官方」的版本,放进 Aleph-mcp,作为「翅膀」打通绝大多数媒体生成模型。

t8star 是 `new-api`/`one-api` 形态的中转:鉴权 `Authorization: Bearer sk-...`,对话面 OpenAI + Anthropic 双兼容,媒体面聚合多家厂商。

### 1.2 与 Aleph core 的职责切分(R1 / R3 / R7 / R8)

- **对话 / 推理 → Aleph Provider 系统**:t8star 是 OpenAI 兼容中转,用户把它当成一个 Provider(填 `base_url=https://ai.t8star.org/v1` + key)即可在主循环里用其 830 个 chat 模型。**MCP 不碰 chat**——否则违 R7(让 LLM 把 LLM 当工具调)。
- **媒体生成(图 / 语音,后续视频)→ 本 MCP**:这是 core 与 Provider 系统都不覆盖的能力,正是 MCP 的职责。
- 这与 `siliconflow/` 边界完全同构(硅基的 chat/embed/rerank 由 core 覆盖,MCP 只做媒体)。

### 1.3 与 skills/plugins 承载方式的区别(为什么 Aleph-mcp 更轻)

沿用 siliconflow spec §1.2:交付物是**独立可执行进程**(Python,运行时 `uvx` 拉起,不嵌入二进制);Aleph 接入点仅 `catalog.json` 一条预设。本期对主仓改动面 = **一条 JSON 预设 + 一行测试断言**。

---

## 2. Locked Decisions

- **D1. 代码来源 = 清爽重写,镜像 `siliconflow/` 文件结构。** t8star 无社区 MCP 可参考,但端点遵循 OpenAI 兼容惯例 + 本设计 §3 的**实时 API 实测校对**。
- **D2. 仓库结构 = 复用现有 monorepo。** `Aleph-mcp/t8star/` 与 `Aleph-mcp/siliconflow/` 平级;不新建仓库(仓库已存在)。
- **D3. 分发 = npm 形态(npx)。** `catalog.json` 指向 `npx -y aleph-t8star-mcp@0.1.0`(`requires_runtime: node`),与 catalog 里既有的 `context7`(npx)同套路。先发 npm,再让 core 的 catalog 指向它。
- **D4. 语言/运行时 = TypeScript + 官方 `@modelcontextprotocol/sdk@^1.29.0`(McpServer + `registerTool` + zod inputSchema) + 原生 `fetch`,`npx` 运行,Node ≥18,`tsc` 构建到 `dist/`,`vitest` 测试。** Node 18+ 内置 `fetch`/`FormData`/`Blob` → 零额外 HTTP 依赖,运行期仅 `@modelcontextprotocol/sdk` + `zod`。
- **D5. 端点默认 `https://ai.t8star.org/v1`,env 可改。** `T8STAR_API_BASE` 覆盖(t8star 另有 `.cn` 镜像)。
- **D6. 工具面 media-only(v1 共 5 个):** `generate_image` / `edit_image` / `generate_speech` / `list_models` / `get_balance`。显式排除 chat/embed/rerank/视频(视频迭代加入,§12)。这正是 siliconflow 8 工具**减去 3 个视频工具**的同构子集。
- **D7. 不硬编码权威模型清单。** 830 模型频繁漂移。给合理默认(图 `gpt-image-2`、TTS `tts-1`) + 文档标注「截至撰写时」+ 靠 `list_models` 动态发现。
- **D8. npm 发布是对外动作,执行前向用户确认。** 新包名 `aleph-t8star-mcp` 需在 npm 配 Trusted Publisher(OIDC,2025-07 GA:零 token、自动 provenance、公开仓可用);GHA `id-token: write` + runner `npm install -g npm@latest`(需 npm v11+)。

---

## 3. 实测 API 校对结果(本设计的核心价值)

> 校对源:**对 `https://ai.t8star.org` 实时探测**(2026-06-23,用户提供的测试 key,只读端点)+ t8star 官方 ComfyUI 节点仓 `T8mars/Comfyui-zhenzhen` 的模型清单。鉴权 `Authorization: Bearer <T8STAR_API_KEY>`,base = `https://ai.t8star.org/v1`(默认)。

### 3.1 模型列表 — `GET /v1/models`(✅ 实测 HTTP 200)

OpenAI 标准 shape,**额外带两个可过滤字段**:
```json
{"data":[
  {"id":"claude-opus-4-5-20251101","object":"model","created":1626777600,
   "owned_by":"vertex-ai","supported_endpoint_types":["anthropic","openai"]}
]}
```
- `owned_by`:厂商来源(`vertex-ai`/`custom`/`bfl`/…)
- `supported_endpoint_types`:`["anthropic","openai"]` 等,可据此过滤。
- 实测 **830** 个模型。
- → `list_models` 支持按 `owned_by` / `supported_endpoint_types` / 子串过滤(纯客户端过滤,端点本身不接受 query,与硅基 `?type=&sub_type=` 不同)。

**确认的关键模型 id(供默认值 + 文档,截至 2026-06-23):**
| 类别 | 端点归属 | 代表模型 id |
|------|---------|------------|
| 图像生成/编辑 | `/v1/images/*`(OpenAI 兼容) | `gpt-image-2`(默认) `gpt-image-1` `gpt-image-1.5` `gpt-image-2-all` `dall-e-3` `flux-2-pro` `flux-dev` `nano-banana-2` `doubao-seedream-4-0-250828` |
| 语音合成 TTS | `/v1/audio/speech`(OpenAI 兼容) | `tts-1`(默认) `tts-1-hd` `gpt-4o-mini-tts` `minimax/speech-2.6-hd` `kling-tts` |
| 视频(v2 迭代) | vendor 异步 submit/poll | `sora-2` `sora-2-pro` `doubao-seedance-2-0-260128` `veo3.1` `veo3.1-pro` `kling-*` |
| 音乐(v3 迭代) | vendor 异步 | `suno_music` |

### 3.2 账户余额 / 用量 — OpenAI 计费端点(✅ 实测 HTTP 200)

> `GET /v1/user/info` 实测 **404**(t8star 不用硅基那套),改走 OpenAI 经典计费面:

| 端点 | 实测响应 | 用途 |
|------|---------|------|
| `GET /v1/dashboard/billing/subscription` | `{"object":"billing_subscription","has_payment_method":true,"hard_limit_usd":100000000,"soft_limit_usd":...,"system_hard_limit_usd":...,"access_until":0}` | 配额上限(USD) |
| `GET /v1/dashboard/billing/usage` | `{"object":"list","total_usage":12930.3154}` | 累计用量(OpenAI 惯例单位 = **美分**) |

→ `get_balance` 返回:**已用**(`total_usage/100` USD)+ **配额上限**(`hard_limit_usd`)+ **剩余**(仅当 `hard_limit_usd` 是真实有限值时 = `hard_limit_usd − total_usage/100`)。
> ⚠️ **实现期须确认两点**(TDD,不假设):(a) `total_usage` 是否为美分;(b) 测试账户 `hard_limit_usd=1e8` 是「无限」哨兵值——剩余对此类账户无意义,工具须优雅表达「配额无上限,已用 $X」。真实剩余额度通常在 `/api/user/self`(需 web session token,非 API key),**不纳入本 MCP**。

### 3.3 图片生成 — `POST /v1/images/generations`(OpenAI 兼容,高确定)

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `model` | string | ✅ | 默认 `gpt-image-2`;可 `dall-e-3`/`flux-*`/`nano-banana-*`/`doubao-seedream-*` |
| `prompt` | string | ✅ | 文本提示 |
| `size` | string | ❌ | OpenAI 风格 `1024x1024`/`1536x1024`/`1024x1536`/`auto`(**不用硅基的 aspect_ratio→pixel 映射,故无 `ratios.py`**) |
| `n` | int | ❌ | 张数(默认 1) |
| `quality` | string | ❌ | `auto`/`high`/`medium`/`low`(gpt-image 系) |
| `response_format` | string | ❌ | `url` 或 `b64_json`(dall-e 默认 url;gpt-image 多为 b64) |

响应:OpenAI 风格 `{"created":...,"data":[{"url":"..."}|{"b64_json":"..."}]}`。
→ **落盘须同时处理 `url`(下载)与 `b64_json`(解码)两分支**;URL 可能短时过期 → 即时落盘。

### 3.4 图片编辑 — `POST /v1/images/edits`(OpenAI 兼容,中高确定)

OpenAI 标准 **multipart/form-data**:`image`(文件)+ `prompt` + `model` + `size` + 可选 `mask`。
→ **实现期一次小额真实调用确认**:t8star 对 gpt-image 编辑是走 `/v1/images/edits`(multipart)还是 `/v1/images/generations` 带 `image` 参数(硅基那种 img2img)。二者择一,先按 OpenAI 标准 `/v1/images/edits` 实现,实测不通再回退。`edit_image` 与 `generate_image` 对 LLM 是两个工具,底层共享 client。

### 3.5 语音合成 TTS — `POST /v1/audio/speech`(OpenAI 兼容,高确定)

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `model` | string | ✅ | 默认 `tts-1`;可 `tts-1-hd`/`gpt-4o-mini-tts`/`minimax/speech-2.6-hd` |
| `input` | string | ✅ | 待合成文本 |
| `voice` | string | ❌ | OpenAI 音色(`alloy`/`echo`/`fable`/`onyx`/`nova`/`shimmer`),默认 `alloy` |
| `response_format` | string | ❌ | `mp3`(默认)/`opus`/`aac`/`flac`/`wav`/`pcm` |
| `speed` | number | ❌ | 0.25–4.0(默认 1.0) |

响应:**二进制音频**(非 JSON)→ 直接落盘。
> ⚠️ 不同后端模型(如 `minimax/speech-*`/`kling-tts`)的 voice 取值域可能不同;默认 `tts-1`+`alloy` 走纯 OpenAI 兼容路径最稳,其余靠文档与 `list_models` 提示。

---

## 4. 工具规格(对 LLM 暴露,v1 共 5 个)

每个工具:校验入参 → 调端点 → (媒体)落盘 → 返回**结构化文本**(模型名 + 本地绝对路径 + 远程 URL + 关键参数)。

| 工具 | 端点 | 关键入参 | 返回 |
|------|------|---------|------|
| `generate_image` | `POST /v1/images/generations` | model, prompt, size, n, quality, response_format | 每张:本地路径 + URL(或解码后路径) |
| `edit_image` | `POST /v1/images/edits`(multipart) | model, prompt, image(路径/URL), size, mask? | 同上 |
| `generate_speech` | `POST /v1/audio/speech` | model, input, voice, response_format, speed | 本地音频路径 |
| `list_models` | `GET /v1/models` | owned_by?, endpoint_type?, query?(子串) | 过滤后的模型 id 列表 |
| `get_balance` | `GET /v1/dashboard/billing/{subscription,usage}` | — | 已用 / 配额 / 剩余(有限时) |

纯函数(单测覆盖):`buildImagePayload` / `parseImageResponse`(url+b64 双分支)/ `buildSpeechPayload` / `filterModels` / `computeBalance`(美分换算 + 哨兵处理)。

---

## 5. 配置(env)

| env | 必填 | 默认 | 说明 |
|-----|------|------|------|
| `T8STAR_API_KEY` | ✅ | — | API Key(secret);缺失则启动报错(P7) |
| `T8STAR_API_BASE` | ❌ | `https://ai.t8star.org/v1` | 可改 `.cn` 镜像 |
| `T8STAR_IMAGE_DIR` | ❌ | — | 图片落盘目录;未设则只返回远程 URL(b64 模型仍需落盘,见 §6) |
| `T8STAR_AUDIO_DIR` | ❌ | 回退 IMAGE_DIR | 音频落盘目录 |

获取 Key:`https://ai.t8star.org`(注册后控制台生成)。

---

## 6. 错误处理(P7 防御性设计)

- 启动校验 `T8STAR_API_KEY` 缺失 → 明确错误,不静默,**不回显 key**。
- API 4xx/5xx → 解析 t8star 错误体(`{"error":{"message","type","code"}}`,实测格式)→ 人类可读消息。
- `b64_json` 响应**必须落盘**(无 URL 可返回);若用户未配 `T8STAR_IMAGE_DIR` 而模型只回 b64 → 落盘到临时目录并提示,或明确报「请配置 T8STAR_IMAGE_DIR」。
- `url` 响应:即时下载落盘;下载失败 → **优雅降级返回远程 URL**,不丢结果。
- 输入校验:`size` 白名单、`n` 范围、`speed` 0.25–4.0、本地图片路径存在性、模型名非空。

---

## 7. 仓库结构(Aleph-mcp/t8star/,TS + npm)

```
Aleph-mcp/t8star/
├── package.json            # name=aleph-t8star-mcp; type=module; bin → dist/index.js; deps @modelcontextprotocol/sdk + zod
├── tsconfig.json           # NodeNext; outDir dist; strict
├── README.md               # 安装(npx)/配置/工具文档(中英)
├── .env.example
├── src/
│   ├── index.ts            # shebang 入口:建 McpServer + StdioServerTransport
│   ├── server.ts           # registerTool × 5(zod inputSchema)
│   ├── client.ts           # 原生 fetch + settingsFromEnv + 鉴权 + 错误 + 落盘(url/b64)
│   ├── images.ts           # generateImage / editImage
│   ├── audio.ts            # generateSpeech
│   └── models.ts           # listModels / getBalance
└── test/
    ├── client.test.ts      # settingsFromEnv、错误解析、落盘路径/扩展名
    ├── images.test.ts      # payload 构建 + 响应解析(url/b64 双分支)
    ├── audio.test.ts       # speech payload + format
    └── models.test.ts      # filterModels + computeBalance(美分/哨兵)
```
运行期依赖:仅 `@modelcontextprotocol/sdk@^1.29.0` + `zod@^3.25 || ^4`(Node 18+ 内置 fetch/FormData/Blob,无需 HTTP 库);dev:`typescript`/`vitest`/`@types/node`。Node ≥ 18。
> 较 siliconflow 少视频与比例映射(视频迭代加入;图片用 OpenAI `size`),余额走计费端点。

---

## 8. Aleph 接入(主仓唯一改动)

`src/mcp/presets/catalog.json` 追加(第 6 条):

```json
{
  "id": "t8star",
  "name": "T8star 中转",
  "category": "model-provider",
  "description": "聚合中转:图像 / 语音 生成 + 模型列表 / 账户余额(Aleph 自建官方 MCP,视频迭代加入)。",
  "vendor": "T8star · 贞贞的AI工坊",
  "official": true,
  "reachability": "cn-native",
  "transports": [
    { "kind": "stdio", "command": "npx", "args": ["-y", "aleph-t8star-mcp@0.1.0"], "requires_runtime": "node" }
  ],
  "required_env": [
    { "key": "T8STAR_API_KEY", "label": "T8star API Key", "description": "平台 API Key", "secret": true, "required": true, "how_to_get_url": "https://ai.t8star.org" },
    { "key": "T8STAR_API_BASE", "label": "API Base", "description": "默认 .org,可改 .cn 镜像", "secret": false, "required": false, "default": "https://ai.t8star.org/v1" },
    { "key": "T8STAR_IMAGE_DIR", "label": "图片保存目录", "description": "本地落盘目录(留空则返回远程 URL)", "secret": false, "required": false },
    { "key": "T8STAR_AUDIO_DIR", "label": "音频保存目录", "description": "默认回退到图片目录", "secret": false, "required": false }
  ],
  "tags": ["image", "tts", "model-provider", "relay"]
}
```

`src/mcp/presets/mod.rs` 测试 `bundled_catalog_parses_and_has_first_batch` 的 id 列表加 `"t8star"`,断言数量由 5 → 6。

> 注:`T8STAR_API_BASE` 标 `required:false`(有默认值,用户不填也能装),与 siliconflow 同模式。

---

## 9. 测试

- **TS(vitest,不打活 API):** `buildImagePayload`;`parseImageResponse`(`data[].url` / `data[].b64_json` 双分支);`buildSpeechPayload`;`filterModels`;`computeBalance`(美分换算 + 1e8 哨兵);`settingsFromEnv` 默认值 + 缺 key 构造报错。网络型工具函数不进单测(与 siliconflow 同)。
- **Rust(节制 cargo):** 复用 `bundled_catalog_parses_and_has_first_batch`,加 `"t8star"` 断言 → 一次 `cargo test -p alephcore --lib presets`。
- **手动:** 本地 `node dist/index.js` 冒烟跑 `list_models`/`get_balance`(免额度);Panel 设置出现 T8star 预设 → 填 key → 一键装 → 跑 `generate_image` 验证落盘。

---

## 10. 交付顺序

1. 本地写 `Aleph-mcp/t8star/` 全套 TS 代码 + vitest(纯本地,零 cargo)。
2. 本地 `npm run build` + `npm test` 绿;`node dist/index.js` 冒烟跑通工具列表;用测试 key 实跑一次 `list_models`+`get_balance`,并对 `edit_image` 做一次 §3.4 的端点确认。
3. 加 `ci-t8star.yml`(typecheck + vitest)+ `publish-t8star.yml`(OIDC 发布);npm 配 `aleph-t8star-mcp` Trusted Publisher。**发布是对外动作,执行前确认。**
4. 发布 `aleph-t8star-mcp@0.1.0` 到 npm。
5. 主仓 `catalog.json` 加预设(npx/node)+ `mod.rs` 测试断言 → 一次 `cargo test -p alephcore --lib presets`。

---

## 11. 风险与权衡

- **模型名漂移**:830 模型频繁上下线。缓解:D7 不硬编码 + `list_models` 动态发现 + 文档标注时间。
- **`edit_image` 端点形态**:multipart vs img2img 未实测确认 → §3.4 实现期一次小额调用拍板,先按 OpenAI 标准实现。
- **`get_balance` 语义**:`total_usage` 单位(美分?)与 `hard_limit_usd` 哨兵值(1e8=无限)→ TDD 确认 + 工具诚实表达「无上限/已用 $X」,不伪造「剩余」。
- **媒体 URL 短时过期 / b64 必落盘**:未配 `T8STAR_IMAGE_DIR` 时 b64 模型须有兜底(临时目录或明确报错)。
- **`npx` 首次拉取延迟**:首次 `npx` 需下载包;与 catalog 里 context7 同模式,用户已接受。
- **npm 新包注册**:`aleph-t8star-mcp` 是新 npm 包名,首次发布需在 npmjs 配 Trusted Publisher(一次性);runner 需 `npm install -g npm@latest` 拿 npm v11+ 以支持 OIDC。

---

## 12. Open / Fast-follow(本期外,迭代路线)

- **v2 视频**(t8star 主打):`generate_video`(异步 submit+轮询)/`submit_video_generation`/`get_video_status`,覆盖 `sora-2`/`sora-2-pro`/`doubao-seedance-2-0`/`veo3.1`/`kling-*`。**前置**:从 apifox 文档拿异步端点(submit/status)精确 shape——这是 v1 故意不做的不确定面。
- **v3**:Midjourney 代理全套(imagine/fetch/action/blend/describe)、Suno 音乐(`suno_music`)、`transcribe_audio`(STT,待确认模型)。
- catalog 预设固定 tag / 版本以求可复现(已用 `@0.1.0` 固定)。
- 每个新工具独立小步加入,不影响已发布工具。

---

## 13. 致谢与参考

- API 形态实测自 `https://ai.t8star.org`(OpenAI/Anthropic 兼容,new-api/one-api 形态);模型清单参考官方 ComfyUI 节点仓 `T8mars/Comfyui-zhenzhen`。
- 工程范式逐字镜像 Aleph-mcp 既有 `siliconflow/`(`2026-06-23-siliconflow-mcp-design.md`)。
- 接入范式参考 Aleph 现有预设 `siliconflow` / `minimax` / `volcengine-veimagex`(`src/mcp/presets/catalog.json`)。
