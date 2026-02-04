# 自定义 AI Provider 配置指南

## 快速开始

Aleph 现在支持多个自定义 OpenAI 兼容的 AI 服务！你可以使用 DeepSeek、Moonshot、Azure OpenAI 等任何兼容 OpenAI API 的服务。

## 配置步骤

### 1. 复制示例配置

```bash
cp Aleph/config.example.toml ~/.aleph/config.toml
```

### 2. 添加自定义 Provider

在 `~/.aleph/config.toml` 中添加：

```toml
[providers.deepseek]
provider_type = "openai"           # 使用 OpenAI 兼容接口
api_key = "sk-your-deepseek-key"   # 你的 API key
model = "deepseek-chat"
base_url = "https://api.deepseek.com"  # 自定义 API endpoint
color = "#0066cc"
timeout_seconds = 30
max_tokens = 4096
temperature = 0.7
```

### 3. 配置路由规则

```toml
[[rules]]
regex = "^/deep"        # 匹配以 /deep 开头的输入
provider = "deepseek"   # 使用 DeepSeek provider

[[rules]]
regex = ".*"            # 默认规则
provider = "openai"     # 使用 OpenAI
```

### 4. 使用

- 选中文本：`/deep 解释量子计算`
- 按 `Cmd+~` (macOS) 或 `Ctrl+~` (Windows)
- Aleph 自动使用 DeepSeek 处理请求

## 支持的服务

任何提供 OpenAI Chat Completion API 的服务都支持：

| 服务 | API Endpoint | 获取 API Key |
|------|--------------|--------------|
| **DeepSeek** | `https://api.deepseek.com` | https://platform.deepseek.com |
| **Moonshot** | `https://api.moonshot.cn/v1` | https://platform.moonshot.cn |
| **Azure OpenAI** | `https://{resource}.openai.azure.com` | Azure Portal |
| **OpenRouter** | `https://openrouter.ai/api/v1` | https://openrouter.ai |
| **Together AI** | `https://api.together.xyz/v1` | https://together.ai |

## 配置说明

### 必填字段

- `model`: 模型名称（如 `"deepseek-chat"`, `"gpt-4o"`）

### 可选字段

- `provider_type`: Provider 类型（`"openai"`, `"claude"`, `"ollama"`）
  - 如果不设置，会从 provider 名称自动推断
  - 名称包含 "claude" → Claude provider
  - 名称包含 "ollama" → Ollama provider
  - 其他 → OpenAI provider

- `api_key`: API 密钥（云服务必需，本地 Ollama 不需要）

- `base_url`: 自定义 API endpoint（覆盖默认地址）
  - OpenAI 默认: `https://api.openai.com/v1`
  - Claude 默认: `https://api.anthropic.com`
  - 自定义服务必须设置此项

- `color`: 品牌颜色（十六进制，如 `"#10a37f"`）
  - 用于 Halo UI 显示

- `timeout_seconds`: 请求超时时间（秒）
  - 云服务推荐: 30 秒
  - 本地模型推荐: 60 秒

- `max_tokens`: 最大响应 token 数
  - 控制响应长度
  - Claude 必须设置（默认 4096）

- `temperature`: 响应随机性（0.0-2.0）
  - 0.0 = 确定性强
  - 1.0 = 创造性强
  - 默认 0.7

## 常见配置示例

### DeepSeek（中文友好）

```toml
[providers.deepseek]
provider_type = "openai"
api_key = "sk-..."
model = "deepseek-chat"
base_url = "https://api.deepseek.com"
color = "#0066cc"
timeout_seconds = 30
```

### Moonshot（长上下文）

```toml
[providers.moonshot]
provider_type = "openai"
api_key = "sk-..."
model = "moonshot-v1-8k"  # 或 moonshot-v1-32k, moonshot-v1-128k
base_url = "https://api.moonshot.cn/v1"
color = "#ff6b6b"
max_tokens = 8192
```

### Azure OpenAI（企业级）

```toml
[providers.azure]
provider_type = "openai"
api_key = "your-azure-key"
model = "gpt-4o"  # 你的部署名称
base_url = "https://your-resource.openai.azure.com"
color = "#0078d4"
```

## 故障排查

### "Invalid API key" 错误

- 检查 API key 是否正确
- 确认没有多余的空格或引号
- 在服务商网站验证 key 是否有效

### "Rate limit exceeded" 错误

- 等待几分钟后重试
- 考虑升级 API 计划
- 或使用本地 Ollama 避免限流

### "Timeout" 错误

- 增加 `timeout_seconds` 设置
- 检查网络连接
- 尝试其他 provider

### "Model not found" 错误

- 检查 `model` 名称是否正确
- 确认该模型在你的账户中可用
- 查看服务商文档获取可用模型列表

## 高级功能

### 多个 Provider 同时使用

```toml
[providers.openai]
# OpenAI 官方

[providers.deepseek]
# DeepSeek 中文

[providers.moonshot]
# Moonshot 长上下文

[providers.ollama]
# 本地 Llama

[[rules]]
regex = "^/code"
provider = "openai"  # 代码任务用 OpenAI

[[rules]]
regex = "^/deep"
provider = "deepseek"  # 中文任务用 DeepSeek

[[rules]]
regex = "^/local"
provider = "ollama"  # 隐私任务用本地模型
```

### Provider 优先级

Routing rules 从上到下匹配，**第一个匹配的规则生效**：

```toml
[[rules]]
regex = "^/code"
provider = "claude"  # 优先级最高

[[rules]]
regex = "^/"
provider = "deepseek"  # 次优先级

[[rules]]
regex = ".*"
provider = "openai"  # 兜底规则（匹配所有）
```

## 更多信息

- 完整配置示例：`Aether/config.example.toml`
- 实施细节：`Aether/core/CUSTOM_PROVIDERS_SUMMARY.md`
- OpenAI API 文档：https://platform.openai.com/docs/api-reference
- DeepSeek API 文档：https://platform.deepseek.com/docs
- Moonshot API 文档：https://platform.moonshot.cn/docs

## 需要帮助？

如果遇到问题：
1. 查看 `config.example.toml` 中的详细注释
2. 检查 Aleph 日志输出
3. 在 GitHub 提交 Issue：https://github.com/your-repo/aether/issues

---

Happy Hacking! 🚀
