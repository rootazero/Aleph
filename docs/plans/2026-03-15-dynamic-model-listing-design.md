# Dynamic Model Listing Design

**Date**: 2026-03-15
**Status**: Approved
**Approach**: B — 统一 probe + 智能 fallback

## Problem

所有 provider 的模型选取都存在问题：

1. **protocol 默认值陷阱**：`ProviderConfig.protocol()` 在 `protocol` 为 `None` 时默认返回 `"openai"`，导致 Anthropic 等 provider 错误地查到 GPT 模型列表
2. **前端同样的默认值**：`providers.rs` 加载已有 provider 时 `provider_type` 为 `None` 也 fallback 到 `"openai"`
3. **Anthropic 缺少 list_models()**：trait 默认返回 `Ok(None)`，回退到 preset 查找，但 protocol 错误导致查到 GPT 模型
4. **Embedding/Reranking 完全硬编码**：前端 `embedding_models_for_preset()` 和 `rerank_models_for_preset()` 维护独立的硬编码模型列表

## Design

### Part 1: Backend — protocol fix + generic list_models()

#### 1.1 Remove "openai" default

`ProviderConfig.protocol()` 改为 panic（protocol 必须设置）。所有构造 `ProviderConfig` 的路径必须确保 `protocol` 已设置。

#### 1.2 Generic list_models() in trait default

`ProtocolAdapter::list_models()` 默认实现改为尝试 OpenAI 兼容的 `/v1/models` 端点：
- 从 config 获取 base_url + api_key
- GET `{base_url}/models` with `Authorization: Bearer {api_key}`
- 解析 `{"data": [...]}` 格式
- 失败则返回 `Ok(None)`，由 ModelRegistry 回退到 preset

覆盖 80% 的 OpenAI 兼容 provider（DeepSeek、Moonshot、SiliconFlow、Groq 等）。

#### 1.3 Anthropic custom override

Anthropic 没有公开 models API，覆盖 trait 方法直接返回 `Ok(None)`，强制走 `model-presets.toml`。

#### 1.4 ModelRegistry fallback unchanged

三层策略保持：Cache → API Probe → Preset (按 protocol key 查找)。

### Part 2: Frontend — protocol fix + Embedding/Reranking unification

#### 2.1 Fix frontend "openai" default

`providers.rs` 第 561/572 行：`provider_type` 为 `None` 时使用 provider name 推断而非 `"openai"`。

#### 2.2 Embedding provider page

删除 `embedding_models_for_preset()` 硬编码。复用 probe 机制：
- 填写 API Key → 调用 probe → API 返回模型列表（过滤 `"embedding"` capability）
- 失败 → 回退 `model-presets.toml` 中 `[protocol.embedding]` section

#### 2.3 Reranking provider page

删除 `rerank_models_for_preset()` 硬编码。同理复用 probe + preset fallback。

#### 2.4 ModelSelector component unchanged

已支持 `Vec<ModelOption>` 显示、来源标记、`__custom__` 手动输入、refresh 按钮。

### Part 3: model-presets.toml extension + error handling

#### 3.1 TOML structure

扩展为支持子类别：
```toml
[anthropic]
models = [...]

[openai]
models = [...]

[openai.embedding]
models = [
    { id = "text-embedding-3-small", ... },
]

[siliconflow.embedding]
models = [...]

[jina.reranking]
models = [...]
```

ModelRegistry 查找扩展：
- `list_models("anthropic", category=None)` → `[anthropic].models`
- `list_models("openai", category="embedding")` → `[openai.embedding].models`

#### 3.2 Error handling

| Scenario | Behavior |
|----------|----------|
| No API key, first preset selection | Show preset models (tagged `[Preset]`) |
| API key entered, click refresh | Probe API → replace with API models on success |
| Invalid API key | Show error, keep preset list selectable |
| Provider doesn't support `/v1/models` | Probe returns None, fallback to preset |
| No preset data for protocol | Empty list + `__custom__` manual input |
| Network timeout | 5s timeout, fallback to preset, show hint |

#### 3.3 Out of scope

- No automatic capability detection (unreliable from API)
- No persistent disk cache (in-memory 24h TTL sufficient)
- No major embedding/reranking handler refactor (only add probe endpoint)

## Key Files

**Backend:**
- `src/config/types/provider.rs` — remove "openai" default
- `src/providers/adapter.rs` — generic list_models() default
- `src/providers/protocols/anthropic.rs` — override list_models()
- `src/providers/model_registry.rs` — extend for category support
- `shared/config/model-presets.toml` — add embedding/reranking sections
- `src/gateway/handlers/embedding_providers.rs` — add probe endpoint

**Frontend:**
- `apps/panel/src/views/settings/providers.rs` — fix protocol default
- `apps/panel/src/views/settings/embedding_providers.rs` — remove hardcoded, use probe
- `apps/panel/src/views/settings/reranking_providers.rs` — remove hardcoded, use probe
