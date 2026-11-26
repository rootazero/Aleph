# Direct model catalog evidence / 直连模型目录证据

**Captured / 查阅日期:** 2026-11-25. Public official pages only; no credentials or inference calls. / 只查阅官方公开页面，未使用凭据或调用推理。

| Exact wire ID / 精确 ID | Official evidence and catalog scope / 官方证据与适用范围 |
| --- | --- |
| `claude-opus-5-5` | [Model overview](https://platform.claude.com/docs/en/models/opus-5-5/overview) lists this Claude API ID, 1M context, 128K interactive max output and adaptive thinking. The row applies to Messages on `https://api.anthropic.com`. Standard USD/MTok input 4, output 20, cache read 0.20, 5m cache write 5; 1h write 8 is not represented. / Claude API 直连 Messages；不采用 Batch 的 300K 上限，不将 1 小时缓存价写成通用价格。 |
| `claude-sonnet-5-5` | [Model overview](https://platform.claude.com/docs/en/models/sonnet-5-5/overview) lists the Claude API ID, 1M context and 128K interactive max output. Standard USD/MTok input 2, output 10, cache read 0.10, 5m cache write 2.50; 1h write 4 is not represented. Same Messages scope. / 适用范围同上，标准价不含长缓存档。 |
| `claude-haiku-5-5` | [Model overview](https://platform.claude.com/docs/en/models/haiku-5-5/overview) lists the Claude API ID, 1M context and 128K interactive max output. Same Messages scope. Billing changes above 100,000 input tokens for the whole request; no flat price is published in this row. / 超过 100,000 个输入 token 时整笔按不同档计价，本目录仅记录能力。 |
| `deepseek-flash` | The [model listing](https://api-docs.deepseek.com/api/list-models/) identifies DeepSeek-V4.1-Flash-1017, `context_length: 1048576`, `max_output_tokens: 393216`. [Pricing](https://api-docs.deepseek.com/quick_start/pricing/) identifies current `deepseek-flash` access and distinct peak/off-peak pricing. The row applies to OpenAI-compatible Chat Completions on `https://api.deepseek.com`, not its separate Anthropic-compatible path. / 只适用现有 DeepSeek OpenAI 兼容直连；不把峰谷价格压成单价，也不扩大非 Flash 范围。 |

The named Claude models are current on the first-party API; see [model deprecations](https://platform.claude.com/docs/en/about-claude/model-deprecations) and [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing). The table covers the standard first-party offerings, not regional-processing variants, cloud aliases, OAuth or relays. It does not assert that any account is entitled. Curated rows are admin/tool reference data only: they do not change the automatic ladder, readiness, authentication or member selection.

Claude 条目是第一方 API 当前型号；生命周期与价格分别参见上面的官方清单。本记录不把第一方标准服务的数据外推到区域处理变体、云平台别名、OAuth 或中转，也不承诺账号权限。策展条目只供管理目录和工具查阅，不改变自动回退、就绪状态、鉴权或成员可选范围。

## Not included / 未纳入

- OpenAI [`gpt-6.2-astra`](https://developers.openai.com/api/docs/models/gpt-6.2-astra), [`gpt-6.2-sol`](https://developers.openai.com/api/docs/models/gpt-6.2-sol), and [`gpt-6.1-terra`](https://developers.openai.com/api/docs/models/gpt-6.1-terra): direct IDs are verified, but the existing Chat Completions token-cap routing does not recognize GPT-6. The Astra/Sol first-party model pages distinguish context (1,050,000) from max input (922,000), and pricing changes above 272K. [`gpt-6-luna`](https://developers.openai.com/api/docs/models/gpt-6-luna) requires `reasoning_effort: none` for Chat Completions function calling. This update does not change transport support or add rows for those models. / 型号已核实，但当前传输实现尚未证明兼容；不修改传输、不加入目录、不类推 Codex/OAuth。
- Anthropic [`claude-fable-5-2`](https://platform.claude.com/docs/en/models/fable-5-2/overview): the API rejects forced tool choices `any` and `tool` and requires 30-day retention. Those conditions are outside this catalog update. / 工具选择及数据保留限制超出本次范围。
- DeepSeek legacy IDs remain independent; this update neither removes them nor assigns a replacement. / 保留既有 ID，不指定迁移后继。
