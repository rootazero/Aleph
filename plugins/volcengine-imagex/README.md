# volcengine-imagex — veImageX MCP 挂载

火山引擎官方 **veImageX MCP** 的 Aleph 挂载封装。把 86081「智能视觉 / VisualService」侧的能力
（文生图、超分修复＝图像增强、漫画/风格化＝风格转换、画质评估、OCR 等）作为 MCP 工具接入 Aleph，
**不进 core**（遵守 R3 内核轻量化 / R8 工具即一切）。

> 官方源：`github.com/volcengine/mcp-imagex` · PyPI `veimagex-mcp` · 市场 `volcengine.com/mcp-marketplace`

## 前置依赖

1. **uv / uvx**（运行 Python MCP 包）：
   ```bash
   curl -LsSf https://astral.sh/uv/install.sh | sh   # macOS/Linux
   ```
2. **veImageX 凭据**（火山引擎 veImageX 控制台获取）：`AccessKey`、`SecretKey`、`ServiceId`、`Domain`。

## 配置凭据（务必替换占位符，勿提交真实密钥）

编辑同目录 `.mcp.json`，把 `env` 里的 `<YOUR_*>` 占位符换成你的真实值：

| 变量 | 含义 |
|------|------|
| `VOLCENGINE_ACCESS_KEY` | 火山 AccessKey |
| `VOLCENGINE_SECRET_KEY` | 火山 SecretKey |
| `SERVICE_ID` | veImageX 服务 ID |
| `DOMAIN` | veImageX 加速域名 |

> 安全建议：真实密钥不要写进版本库。若你的部署允许 MCP 子进程继承宿主环境变量，可改为
> `export VOLCENGINE_ACCESS_KEY=...` 等在启动 `aleph-server` 前注入，并把 `.mcp.json` 的 env 留空。

## Aleph 如何加载

Aleph 自动发现插件目录下的 `.mcp.json`（`src/extension/mcp_config.rs`），按 stdio 启动该 MCP
server，工具经 tool bridge 自动注册，对话即可调用（R8）。重启 `aleph-server` 后生效。

## 备选：直接写进主配置（不用插件目录）

不想用插件目录的话，也可把同样的 server 写进 Aleph 主配置 TOML 的 `[mcp]` 段：

```toml
[mcp]
enabled = true

[[mcp.external_servers]]
name = "veimagex"
command = "uvx"
args = ["veimagex-mcp"]
requires_runtime = "python"
timeout_seconds = 60

[mcp.external_servers.env]
VOLCENGINE_ACCESS_KEY = "<YOUR_VOLCENGINE_ACCESS_KEY>"
VOLCENGINE_SECRET_KEY = "<YOUR_VOLCENGINE_SECRET_KEY>"
SERVICE_ID = "<YOUR_IMAGEX_SERVICE_ID>"
DOMAIN = "<YOUR_IMAGEX_DOMAIN>"
```

## 与 core 火山引擎(Ark)预设的关系

- **core 内置（Ark / Bearer / OpenAI 兼容）**：Seedream 4.5 文生图·图生图·基础 inpainting、Seedance 2.0 视频。
- **本 MCP（veImageX / VisualService）**：图像增强(超分修复)、风格转换(漫画风格)、画质评估、OCR 等
  Ark 没有对应端点的能力。
- **图片换装(try-on)**：veImageX MCP 暂未覆盖；如需，用官方 `@volcengine/openapi` SDK 自封一个小 MCP 再以同样方式挂载。
