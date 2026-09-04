# Severed-Wire Audit — `src/config/`

**Date:** 2026-09-01
**Module:** `src/config/` — full module: `mod.rs`, `structs.rs`, `schema.rs`, `methods.rs`, `load.rs`, `save.rs`, `patcher.rs`, `validate.rs`, `migration.rs`, `live_apply.rs`, `reload_impact.rs`, `backup.rs`, `dead_keys.rs`, `defaults_override.rs`, `presets_override.rs`, `guides.rs`, plus sub-modules `agent_manager/`, `agent_resolver/`, `types/` (incl. `dispatcher/`, `generation/`, `memory/`, `policies/`), `ui_hints/`, `tests/` (~7,400+ top-level lines + ~3,000 in `types/` sub-tree).
**Method:** `rg` (ripgrep) parity checks for producer vs. consumer symbols; field-level reads per `Config` field (the headline **config-readership audit**, lens 5 in the seam catalog); stub-sweep and code-smell scan over the entire tree. All evidence backed by an `rg` command and excerpted output.

## Inventory — produced surface

### `mod.rs` (facade)
| Symbol | Location |
|---|---|
| `pub mod agent_manager;` | mod.rs:14 |
| `pub mod agent_resolver;` | mod.rs:15 |
| `pub mod backup;` | mod.rs:16 |
| `pub mod defaults_override;` | mod.rs:18 |
| `pub mod guides;` | mod.rs:19 |
| `pub mod live_apply;` | mod.rs:20 |
| `pub(crate) mod load;` | mod.rs:21 |
| `pub mod patcher;` | mod.rs:24 |
| `pub mod presets_override;` | mod.rs:25 |
| `pub mod reload_impact;` | mod.rs:26 |
| `pub mod schema;` | mod.rs:28 |
| `pub mod types;` | mod.rs:30 |
| `pub mod ui_hints;` | mod.rs:31 |
| `pub use structs::{ChannelInstanceConfig, Config, PluginMarketplaceEntry};` | mod.rs:35 |
| `pub use live_apply::classify_verified;` | mod.rs:39 |
| `pub use reload_impact::ReloadImpact;` | mod.rs:40 |
| `pub use schema::generate_config_schema_json;` | mod.rs:43 |
| `pub use ui_hints::ConfigUiHints;` | mod.rs:48 |
| `pub use types::*;` | mod.rs:51 |

### `structs.rs` — `Config` + nested types
| Symbol | Location |
|---|---|
| `pub struct PluginMarketplaceEntry` (fields: `source`, `source_type`) | structs.rs:30 |
| `pub struct Config` (50+ fields; see §"Config fields" below) | structs.rs:52 |
| `pub struct ChannelInstanceConfig` (fields: `id`, `channel_type`, `config`) | structs.rs:308 |
| `pub fn Config::new` | structs.rs:432 |
| `pub fn Config::local_voice` | structs.rs:438 |
| `pub fn Config::migrate_fetch` | structs.rs:449 |
| `pub fn Config::resolved_channels` | structs.rs:334 |

### `Config` fields (the headline surface; each must have a reader)
| Field | Type | TOML rename | Default | Source line |
|---|---|---|---|---|
| `general` | `GeneralConfig` | – | required | structs.rs:55 |
| `memory` | `MemoryConfig` | – | required | structs.rs:58 |
| `providers` | `HashMap<String, ProviderConfig>` | – | empty | structs.rs:62 |
| `rules` | `Vec<RoutingRuleConfig>` | – | `vec![]` | structs.rs:71 |
| `behavior` | `Option<BehaviorConfig>` | – | `Some(_)` | structs.rs:74 |
| `search` | `Option<SearchConfigInternal>` | – | `None` | structs.rs:77 |
| `fetch` | `Option<FetchConfigInternal>` | – | `None` | structs.rs:80 |
| `tools` | `ToolsConfig` | – | required | structs.rs:83 |
| `mcp` | `McpConfig` | – | required | structs.rs:86 |
| `unified_tools` | `Option<UnifiedToolsConfig>` | – | `None` | structs.rs:90 |
| `tool_service` | `ToolServiceConfig` | – | required | structs.rs:93 |
| `sandbox` | `SandboxConfig` | – | required | structs.rs:98 |
| `policies` | `PoliciesConfig` | – | required | structs.rs:112 |
| `generation` | `GenerationConfig` | – | required | structs.rs:115 |
| `voice_local` | `VoiceSection` | rename = `voice` | required | structs.rs:118 |
| `route` | `ModelRouteConfig` | – | required | structs.rs:123 |
| `group_chat` | `GroupChatConfig` | – | required | structs.rs:126 |
| `cron` | `CronConfig` | – | required | structs.rs:129 |
| `heartbeat` | `HeartbeatConfig` | – | required | structs.rs:132 |
| `tasks_reaper` | `ReaperConfig` | alias = `task_reaper` | required | structs.rs:136 |
| `personas` | `Vec<PersonaConfig>` | – | empty | structs.rs:139 |
| `privacy` | `PrivacyConfig` | – | required | structs.rs:142 |
| `security` | `ShellSecurityConfig` | – | required | structs.rs:145 |
| `ssrf` | `SsrfPolicy` | – | required | structs.rs:148 |
| `profiles` | `HashMap<String, ProfileConfig>` | – | empty | structs.rs:152 |
| `secret_providers` | `HashMap<String, SecretProviderConfig>` | – | empty | structs.rs:155 |
| `secrets_config` | `SecretsConfig` | – | required | structs.rs:158 |
| `prompt` | `PromptSectionConfig` | – | required | structs.rs:161 |
| `channels` | `HashMap<String, serde_json::Value>` | – | empty | structs.rs:172 |
| `a2a` | `A2AConfig` | – | required | structs.rs:175 |
| `acp` | `AcpConfig` | – | required | structs.rs:178 |
| `execution` | `ExecutionConfig` | – | required | structs.rs:181 |
| `agents` | `AgentsConfig` | – | required | structs.rs:185 |
| `bindings` | `Vec<RouteBinding>` | – | empty | structs.rs:189 |
| `session` | `SessionConfig` | – skip-if-default | required | structs.rs:195 |
| `plugin_marketplaces` | `HashMap<String, PluginMarketplaceEntry>` | – | empty | structs.rs:215 |
| `stop_hooks` | `Vec<StopHookConfig>` | – | empty | structs.rs:220 |
| `guardrails` | `Option<GuardrailsToml>` | – | `None` | structs.rs:225 |
| `stability` | `Option<StabilityToml>` | – | `None` | structs.rs:229 |
| `fallback_provider` | `Option<FallbackProviderToml>` | – | `None` | structs.rs:233 |
| `context_budget` | `Option<ContextBudgetToml>` | – | `None` | structs.rs:239 |
| `strategy` | `Option<StrategyToml>` | – | `None` | structs.rs:248 |
| `moa` | `Option<MoaToml>` | – | `None` | structs.rs:254 |
| `team_dispatcher` | `Option<TeamDispatcherConfigToml>` | – | `None` | structs.rs:261 |
| `team_broadcast` | `Option<TeamBroadcastConfigToml>` | – | `None` | structs.rs:268 |
| `team_messages` | `Option<TeamMessagesConfigToml>` | – | `None` | structs.rs:275 |
| `resume` | `ResumeConfig` | – | required | structs.rs:278 |
| `projects` | `ProjectsConfig` | – | required | structs.rs:283 |
| `desktop` | `DesktopDaemonConfig` | – | required | structs.rs:290 |
| `presets_override` | `PresetsOverride` (skipped) | – | required | structs.rs:294 |
| `defaults_override` | `DefaultsOverride` (skipped) | – | required | structs.rs:299 |

### `methods.rs` — accessors on `Config`
| Symbol | Location |
|---|---|
| `pub fn Config::get_effective_tools_config` | methods.rs:23 |
| `pub fn Config::get_default_provider` | methods.rs:46 |
| `pub fn Config::set_default_provider` | methods.rs:64 |
| `pub fn Config::add_rule_at_top` | methods.rs:109 |
| `pub fn Config::remove_rule` | methods.rs:136 |
| `pub fn Config::move_rule` | methods.rs:183 |
| `pub fn Config::get_rule` | methods.rs:233 |
| `pub const fn Config::rule_count` | methods.rs:242 |

### `schema.rs`
| Symbol | Location |
|---|---|
| `pub(crate) fn generate_config_schema` | schema.rs:10 |
| `pub fn generate_config_schema_json` | schema.rs:20 |

### `load.rs` — boot loader + pin registry
| Symbol | Location |
|---|---|
| `pub(crate) const fn effective_config_path_slot` | load.rs:33 |
| `pub fn Config::default_path` | load.rs:46 |
| `pub fn Config::set_effective_path` | load.rs:64 |
| `pub fn Config::decline_effective_path` | load.rs:88 |
| `pub fn Config::effective_path` | load.rs:99 |
| `pub fn Config::load_from_file` | load.rs:118 |
| `pub fn Config::load_from_file_reporting_dead_keys` | load.rs:166 |
| `pub fn Config::load` | load.rs:289 |

### `save.rs`
| Symbol | Location |
|---|---|
| `pub fn Config::save_to_file` | save.rs:240 |
| `pub fn Config::save` | save.rs:317 |
| `pub fn Config::save_incremental` | save.rs:357 |
| `pub fn Config::save_incremental_to_file` | save.rs:381 |

### `patcher.rs` — central patching engine
| Symbol | Location |
|---|---|
| `pub struct PatchRequest` | patcher.rs:55 |
| `pub struct PatchResult` | patcher.rs:73 |
| `pub struct FieldDiff` | patcher.rs:95 |
| `pub enum HealthCheckResult` | patcher.rs:109 |
| `pub struct RollbackResult` | patcher.rs:117 |
| `pub struct ConfigPatcher` | patcher.rs:139 |
| `pub fn ConfigPatcher::new` | patcher.rs:156 |
| `pub fn ConfigPatcher::with_vault` | patcher.rs:171 |
| `pub fn ConfigPatcher::config_path` | patcher.rs:177 |
| `pub(crate) async fn ConfigPatcher::record_mtime` | patcher.rs:184 |
| `pub async fn ConfigPatcher::apply` | patcher.rs:223 |
| `pub fn ConfigPatcher::list_backups` | patcher.rs:562 |
| `pub async fn ConfigPatcher::rollback` | patcher.rs:581 |
| `pub(crate) fn ConfigPatcher::validate_schema` | patcher.rs:515 |
| `pub(crate) fn get_nested_value` | patcher.rs:688 |
| `pub(crate) fn set_nested_value` | patcher.rs:704 |
| `pub(crate) fn deep_merge` | patcher.rs:768 |
| `pub(crate) fn compute_diff` | patcher.rs:810 |

### `live_apply.rs` and `reload_impact.rs`
| Symbol | Location |
|---|---|
| `pub(crate) const LIVE_SECTIONS: &[&str]` | reload_impact.rs:67 |
| `pub(crate) const LIVE_SUBSECTIONS: &[&str]` | reload_impact.rs:100 |
| `pub(crate) fn dotted_prefix_matches` | reload_impact.rs:137 |
| `pub(crate) fn live_target_for` | reload_impact.rs:152 |
| `pub(crate) fn live_targets` | reload_impact.rs:168 |
| `pub enum ReloadImpact` (variants `Live`, `Restart`, `Inert`) | reload_impact.rs:31 |
| `pub fn apply_live_sections` | live_apply.rs:51 |
| `pub fn classify_verified` | live_apply.rs:195 |

### `validate.rs`
| Symbol | Location |
|---|---|
| `pub fn normalize_default_provider` | validate.rs:37 |
| `pub fn Config::validate` | validate.rs:69 |
| `pub fn Config::log_advisories` | validate.rs:654 |

### `migration.rs`
| Symbol | Location |
|---|---|
| `pub(crate) fn Config::migrate_mcp_builtin_in_toml` | migration.rs:21 |
| `pub(crate) fn Config::migrate_vector_db_in_toml` | migration.rs:68 |

### `backup.rs`
| Symbol | Location |
|---|---|
| `pub struct BackupEntry` | backup.rs:13 |
| `pub struct ConfigBackup` | backup.rs:22 |
| `pub const fn ConfigBackup::new` | backup.rs:33 |
| `pub fn ConfigBackup::default_dir` | backup.rs:54 |
| `pub fn ConfigBackup::create_snapshot` | backup.rs:71 |
| `pub fn ConfigBackup::cleanup` | backup.rs:123 |
| `pub fn ConfigBackup::resolve` | backup.rs:170 |
| `pub fn ConfigBackup::list` | backup.rs:198 |

### `dead_keys.rs`
| Symbol | Location |
|---|---|
| `pub(crate) fn deserialize_reporting_dead_keys` | dead_keys.rs:176 |

### `defaults_override.rs` (loaded from `~/.aleph/defaults.toml`)
| Symbol | Location |
|---|---|
| `pub struct ProviderDefaultsOverride` (field: `timeout_seconds`) | defaults_override.rs:27 |
| `pub struct GenerationDefaultsOverride` (field: `timeout_seconds`) | defaults_override.rs:35 |
| `pub struct DefaultsOverride` (fields: `provider`, `generation`) | defaults_override.rs:46 |
| `pub(crate) const fn defaults_override_slot` | defaults_override.rs:115 |
| `pub fn init_defaults_override` | defaults_override.rs:125 |
| `pub fn get_defaults_override` | defaults_override.rs:138 |
| `pub fn load_defaults_override` | defaults_override.rs:150 |
| `pub fn DefaultsOverride::provider_timeout_seconds` | defaults_override.rs:185 |
| `pub fn DefaultsOverride::generation_timeout_seconds` | defaults_override.rs:190 |

### `presets_override.rs` (loaded from `~/.aleph/presets.toml`)
| Symbol | Location |
|---|---|
| `pub struct PartialProviderPreset` | presets_override.rs:21 |
| `pub struct PartialGenerationPreset` | presets_override.rs:50 |
| `pub struct GenerationPresetsOverride` | presets_override.rs:67 |
| `pub struct PresetsOverride` | presets_override.rs:87 |
| `pub fn load_presets_override` | presets_override.rs:104 |
| `pub struct OwnedProviderPreset` | presets_override.rs:142 |
| `pub struct OwnedGenerationPreset` | presets_override.rs:151 |
| `pub fn merge_provider_preset` | presets_override.rs:163 |
| `pub fn partial_to_provider_preset` | presets_override.rs:191 |
| `pub fn merge_generation_preset` | presets_override.rs:207 |
| `pub fn partial_to_generation_preset` | presets_override.rs:226 |

### `guides.rs`
| Symbol | Location |
|---|---|
| `pub const GUIDE_FILES: &[(&str, &str)]` | guides.rs:11 |
| `pub fn deploy_guides` | guides.rs:38 |

### `ui_hints/mod.rs`
| Symbol | Location |
|---|---|
| `pub struct GroupMeta` (fields: `label`, `order`, `icon`) | ui_hints/mod.rs:20 |
| `pub struct FieldHint` (fields: `label`, `help`, `group`, `order`, `advanced`, `sensitive`, `placeholder`) | ui_hints/mod.rs:35 |
| `pub struct ConfigUiHints` (fields: `groups: HashMap<String, GroupMeta>`, `fields: HashMap<String, FieldHint>`) | ui_hints/mod.rs:64 |
| `pub fn ConfigUiHints::new` | ui_hints/mod.rs:71 |

### `agent_manager/mod.rs`
| Symbol | Location |
|---|---|
| `pub(super) const BOOTSTRAP_FILES: &[&str]` | agent_manager/mod.rs:31 |
| `pub(crate) const CURATED_OWNED_FILES: &[&str]` | agent_manager/mod.rs:50 |
| `pub(crate) fn is_curated_owned` | agent_manager/mod.rs:58 |
| `pub(crate) fn curated_owned_reason` | agent_manager/mod.rs:68 |
| `pub(super) const MAX_ID_LENGTH: usize = 32;` | agent_manager/mod.rs:76 |
| `pub struct AgentPatch` | agent_manager/mod.rs:95 |
| `pub struct WorkspaceFile` | agent_manager/mod.rs:117 |
| `pub struct AgentManager` | agent_manager/mod.rs:129 |
| `pub struct ProvisioningRoots` | agent_manager/mod.rs:142 |
| `pub fn provisioning_roots` | agent_manager/mod.rs:170 |
| `pub(super) fn model_ref_to_item` | agent_manager/toml_ops.rs:220 |

### `agent_manager/crud.rs` (impls on `AgentManager`)
| Symbol | Location |
|---|---|
| `pub fn AgentManager::new` | crud.rs:26 |
| `pub fn AgentManager::list` | crud.rs:182 |
| `pub fn AgentManager::get` | crud.rs:188 |
| `pub fn AgentManager::create` | crud.rs:201 |
| `pub fn AgentManager::update` | crud.rs:261 |
| `pub fn AgentManager::delete` | crud.rs:290 |
| `pub fn AgentManager::set_default` | crud.rs:337 |

### `agent_resolver/mod.rs`
| Symbol | Location |
|---|---|
| `pub(crate) mod templates;` | agent_resolver/mod.rs:26 |
| `pub(crate) fn resolve_model_ref` | agent_resolver/mod.rs:47 |
| `pub struct ResolvedAgent` | agent_resolver/mod.rs:81 |
| `pub struct AgentDefinitionResolver` | agent_resolver/mod.rs:147 |
| `pub fn initialize_agent_dir` | agent_resolver/mod.rs:403 |
| `pub fn initialize_agent_identity` | agent_resolver/mod.rs:417 |
| `pub fn workspace_root_for` | agent_resolver/mod.rs:487 |
| `pub fn agents_root_for` | agent_resolver/mod.rs:498 |
| `pub(crate) fn default_workspace_root` | agent_resolver/mod.rs:509 |
| `pub(crate) fn default_agents_root` | agent_resolver/mod.rs:520 |
| `pub(crate) fn default_soul` | templates.rs:10 |
| `pub(crate) fn default_agents` | templates.rs:17 |
| `pub(crate) fn default_identity` | templates.rs:120 |
| `pub(crate) const DEFAULT_MEMORY` | templates.rs:157 |
| `pub(crate) const DEFAULT_TOOLS` | templates.rs:160 |
| `pub(crate) const DEFAULT_HEARTBEAT` | templates.rs:207 |

### `types/` (re-exported wholesale via `pub use types::*`)
Re-exports include `AcpConfig`, `AgentsConfig`, `BehaviorConfig`, `ContextBudgetToml`, `ExecutionConfig`, `FallbackProviderToml`, `FetchConfigInternal`, `GeneralConfig`, `GenerationConfig`, `GroupChatConfig`, `GuardrailsToml`, `McpConfig`, `MemoryConfig`, `PersonaConfig`, `PoliciesConfig`, `PrivacyConfig`, `ProfileConfig`, `PromptSectionConfig`, `ProviderConfig`, `RoutingRuleConfig`, `SearchConfigInternal`, `SecretProviderConfig`, `SecretsConfig`, `ShellSecurityConfig`, `StabilityToml`, `StopHookConfig`, `ToolServiceConfig`, `ToolsConfig`, `UnifiedToolsConfig`, `VoiceLocalConfig`, `VoiceSection`, plus the runtime helpers (`init_metrics_runtime`, `MetricsPolicy`, `SpendPolicy`, `SpendPeriod`, `SessionMode`, `builtin_modes`, `ExecTier`, `TerminalConfig`, `CompressionPolicy`, `MemoryPolicies`, `PermissionMatch`, `ToolPermissionsConfig`, `MoaSlot`, `MoaFanout`, `MoaPreset`, `MoaToml`, `ModelThresholdToml`, `Crawl4aiConfig`, `WebFetchPolicy`, `DreamingConfig`, `MemoryDecayPolicy`, `MemoryInjectionMode`, `MemoryConfig`, etc.). ~312 `pub` symbols total in `types/`.

## Inventory — production consumers

### Config field-reader parity (lens 5)

Each entry: `\bFIELD\b` count outside `src/config/` and `tests/`, plus a sample of one concrete non-test read.

```bash
$ for f in general memory providers rules behavior search fetch tools mcp unified_tools tool_service sandbox policies generation voice_local route group_chat cron heartbeat tasks_reaper personas privacy security ssrf profiles secret_providers secrets_config prompt channels a2a acp execution agents bindings session plugin_marketplaces stop_hooks guardrails stability fallback_provider context_budget strategy moa team_dispatcher team_broadcast team_messages resume projects desktop presets_override defaults_override; do
    echo "=== $f ==="
    rg -t rust "(\.|->)${f}\b" src/ interfaces/ shared/ desktop/ \
       -g '!src/config/**' -g '!**/tests/**' -g '!**/*tests.rs' \
       -g '!**/tests.rs' -g '!**/test_*.rs' \
       | wc -l
  done
```

Result counts and one sample line per field (the headline **config-readership audit**):

| Field | non-test reads | sample consumer (file:line) | verdict |
|---|---|---|---|
| `general` | 74 | `bin/aleph-server/commands/start/orchestrator_init.rs:139` (`config.general.language.as_deref()`) | LIVE |
| `memory` | 819 | `bin/aleph-server/commands/start/mod.rs:354` (`loaded_app_config.memory.rrf_k`) | LIVE |
| `providers` | 663 | `bin/aleph-server/commands/start/mod.rs:838` (`loaded_app_config.providers`) | LIVE |
| `rules` | 85 | `bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:142` (`register_custom_commands(&app_config.rules)`) | LIVE |
| `behavior` | 90 | `gateway/handlers/agent.rs:198` (read fresh on every turn — `behavior.output_mode`) | LIVE |
| `search` | 359 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:381` (`SearchRegistry::from_config(app_config.search.as_ref())`) | LIVE |
| `fetch` | 73 | `config/load.rs:464` (`config.migrate_fetch()`); consumed downstream by `FetchRegistry` | LIVE |
| `tools` | 657 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:830` (`app_config.tools.core.clone()`) | LIVE |
| `mcp` | 0 | `config/methods.rs:27` (`UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)`) — only direct reader | LIVE (via `from_legacy` only) |
| `unified_tools` | 11 | `gateway/handlers/mcp_config.rs:470` (`match &cfg.unified_tools`) | LIVE |
| `tool_service` | 7 | `bin/aleph-server/commands/start/orchestrator_init.rs:481` (`config.tool_service.parallel_tool_concurrency_opt()`) | LIVE |
| `sandbox` | 148 | `bin/aleph-server/commands/start/mod.rs:311` (`create_platform_driver_from_config(&loaded_app_config.sandbox)`) | LIVE |
| `policies` | 195 | `bin/aleph-server/commands/start/mod.rs:204` (`spend::install_policy(loaded_app_config.policies.spend.clone())`) | LIVE |
| `generation` | 211 | `media/resolve.rs:137` (`GenerationConfig::default()`); widely consumed | LIVE |
| `voice_local` | 18 | `gateway/handlers/voice.rs:138` (`config.read().await.voice_local.format.clone()`) | LIVE |
| `route` | 272 | `bin/aleph-server/commands/start/orchestrator_init.rs:299` (`route_handle::global_route_handle(&config.route)`) | LIVE |
| `group_chat` | 71 | `bin/aleph-server/commands/start/mod.rs:3013` (`app_cfg.group_chat.clone()`) | LIVE |
| `cron` | 232 | `executor/builtin_registry/builder/constructor/mod.rs:547` (`init_cron_trigger(config.cron_service.clone())`) | LIVE |
| `heartbeat` | 116 | `executor/builtin_registry/builder/constructor/mod.rs:1256` (`config.heartbeat_service.as_ref()`) | LIVE |
| `tasks_reaper` | 1 | `bin/aleph-server/commands/start/mod.rs:2813` (`app_cfg.tasks_reaper.clone()` — into `spawn_task_reaper`) | LIVE |
| `personas` | 10 | `bin/aleph-server/commands/start/mod.rs:3013` (`app_cfg.personas.clone()` into `GroupChatOrchestrator::new`) | LIVE |
| `privacy` | 29 | `bin/aleph-server/commands/start/mod.rs:160` (`alephcore::pii::PiiEngine::init(full_config.privacy.clone())`) | LIVE |
| `security` | 189 | `bin/aleph-server/commands/start/mod.rs:317` (`&loaded_app_config.security`) | LIVE |
| `ssrf` | 28 | `bin/aleph-server/commands/start/mod.rs:944` (`webhook_ssrf_policy = loaded_app_config.ssrf.clone()`) | LIVE |
| `profiles` | 80 | `bin/aleph-server/commands/start/mod.rs:913` (`&loaded_app_config.profiles`) | LIVE |
| `secret_providers` | 2 | `bin/aleph-server/commands/secret.rs:170` (`&config.secret_providers`); `gateway/handlers/secrets.rs:209` | LIVE |
| `secrets_config` | 7 | `bin/aleph-server/commands/start/orchestrator_init.rs:532-560` (`config.secrets_config.virtual_keys`/`custom_leak_patterns`) | LIVE |
| `prompt` | 734 | `thinker/layers/extra_files.rs`; widely read | LIVE |
| `channels` | 293 | `bin/aleph-server/commands/start/builder/subsystems.rs:288` (`cfg.resolved_channels()`) | LIVE |
| `a2a` | 9 | `bin/aleph-server/commands/start/mod.rs:1216,2329` (`app_config.read().await.a2a.enabled`) | LIVE |
| `acp` | 191 | `bin/aleph-server/commands/start/mod.rs:1008,1015` (`app_cfg.acp.enabled`/`adapters`) | LIVE |
| `execution` | 94 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:815` (`app_config.execution.max_concurrent_subagents`) | LIVE |
| `agents` | 666 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:912` (`&loaded_app_config.agents`) | LIVE |
| `bindings` | 38 | `bin/aleph-server/commands/start/mod.rs:936` (`AgentRouter::from_bindings(loaded_app_config.bindings.clone(), ...)`) | LIVE |
| `session` | 1047 | `bin/aleph-server/commands/start/mod.rs:936`; widely read | LIVE |
| `plugin_marketplaces` | 11 | `bin/aleph-server/commands/plugins.rs:269` (`&config.plugin_marketplaces`) | LIVE |
| `stop_hooks` | 2 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:1482` (`build_from_config(&app_config.stop_hooks)`) | LIVE |
| `guardrails` | 29 | `bin/aleph-server/commands/start/orchestrator_init.rs:531` (`config.guardrails.as_ref().is_some_and(|g| g.enabled)`) | LIVE |
| `stability` | 36 | `orchestrator/deps_builder/stability.rs:21` (`config.stability.as_ref()`) | LIVE |
| `fallback_provider` | 4 | `orchestrator/deps_builder/context_budget.rs:272` (`config.fallback_provider.as_ref()`) | LIVE |
| `context_budget` | 19 | `orchestrator/deps_builder/summary.rs:58` (`config.context_budget.as_ref()`) | LIVE |
| `strategy` | 92 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:432,471` (`app_config.strategy.as_ref()`) | LIVE |
| `moa` | 88 | `bin/aleph-server/commands/start/orchestrator_init.rs:369-370` (`config.moa.clone()`, `if let Some(moa) = &config.moa`) | LIVE |
| `team_dispatcher` | 1 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:1598` (`match &app_config.team_dispatcher`) | LIVE |
| `team_broadcast` | 1 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:1755` (`match &app_config.team_broadcast`) | LIVE |
| `team_messages` | 1 | `bin/aleph-server/commands/start/builder/agent_init/mod.rs:254` (`match &app_config.team_messages`) | LIVE |
| `resume` | 88 | `bin/aleph-server/commands/start/mod.rs:2873` (`app_cfg.resume.clone()`) | LIVE |
| `projects` | 416 | `gateway/handlers/fs.rs:94,494` (`cfg.projects.allowed_roots`) | LIVE |
| `desktop` | 62 | `executor/builtin_registry/builder/constructor/mod.rs:414` (`cfg.read().await.desktop.allow_global_pointer`) | LIVE |
| `presets_override` | 4 | `gateway/handlers/providers/handlers.rs:156,291`; `gateway/handlers/generation_providers/handlers.rs:215,303` | LIVE |
| `defaults_override` | **0** | only assignments at `config/load.rs:266,346` | **see sw-config-1** |

### `Config::default_provider` family (other lifecycle methods)

```bash
$ rg -n "Config::get_default_provider|config\.get_default_provider\(\)" src/ interfaces/ shared/ desktop/
(no matches)

$ rg -n "\bset_default_provider\b" src/ interfaces/ shared/ desktop/
src/config/methods.rs:64                 (definition)
src/gateway/handlers/providers/handlers.rs:1011    (cfg.set_default_provider(&name))

$ rg -n "\badd_rule_at_top\b|\bremove_rule\(|\bmove_rule\(|\brule_count\(\)" src/ interfaces/ shared/ desktop/
src/gateway/handlers/routing_rules.rs:78   (config.get_rule)
src/gateway/handlers/routing_rules.rs:157  (cfg.add_rule_at_top)
src/gateway/handlers/routing_rules.rs:217  (cfg.rule_count())
src/gateway/handlers/routing_rules.rs:298  (cfg.rule_count())
src/gateway/handlers/routing_rules.rs:318  (cfg.remove_rule)
src/gateway/handlers/routing_rules.rs:384  (cfg.move_rule)

$ rg -n "\bget_effective_tools_config\b" src/ interfaces/ shared/ desktop/
src/executor/builtin_registry/builder/constructor/mod.rs:422  (tool_registry.get_effective_tools_config())
```

The `get_default_provider()` defined in `methods.rs:46` has **zero production callers** (the `get_default_provider(GenerationType)` calls in `config/types/generation/config.rs:149` are a *different* method on `GenerationConfig`, not on `Config`).

### `ConfigPatcher` family

```bash
$ rg -n "ConfigPatcher::" src/ interfaces/ shared/ desktop/
src/executor/builtin_registry/registry/inherent.rs:42,53,56
src/bin/aleph-server/commands/start/mod.rs:1855,1877,3463
src/bin/aleph-server/commands/start/builder/handlers/settings.rs:333,339
src/gateway/handlers/route_config.rs:240
src/gateway/handlers/execution_config.rs:41
src/builtin_tools/self_config.rs:14,140,176,181,186,1183
src/builtin_tools/moa_manage.rs:21,220,235,1016,1061,1148
src/providers/moa/preset_store.rs:5,155
src/gateway/handlers/config.rs:11,1272
src/gateway/handlers/moa.rs:192
src/gateway/handlers/acp_config.rs:655
src/gateway/handlers/security_config/{toml_io,rate_limit}.rs
```

All 7 `pub fn`s of `ConfigPatcher` (`new`, `with_vault`, `config_path`, `apply`, `list_backups`, `rollback`, plus the in-impl `record_mtime` is `pub(crate)` and only used by the patcher itself) are called from at least one of these surfaces.

### `live_apply` and `reload_impact` family

```bash
$ rg -n "apply_live_sections|classify_verified" src/ interfaces/ shared/ desktop/
src/gateway/handlers/route_config.rs:248     (apply_live_sections(.., &["route"]))
src/gateway/handlers/execution_config.rs:132 (apply_live_sections(.., &["execution"]))
src/gateway/handlers/execution_config.rs:133 (classify_verified("execution", &landed))
src/builtin_tools/self_config.rs:476         (crate::config::ReloadImpact::classify(config_path))
src/builtin_tools/agent_manage/update.rs:348,357,362,663,689,719,722,747,760
src/gateway/handlers/browser_config.rs:286-306
src/gateway/handlers/config.rs:709           (builtin_modes)
```

All `pub` symbols of `live_apply` and `reload_impact` are consumed.

### `validate` family

```bash
$ rg -n "Config::validate\b|\.validate\(\)" src/ interfaces/ shared/ desktop/ \
   | grep -E "config\.validate|Cfg\.validate|cfg\.validate"
src/config/load.rs:279          (config.validate()?)
src/config/patcher.rs:393       (final_config.validate())
src/config/patcher.rs:591       (restored.validate())
src/gateway/config.rs:464       (config.validate()?)
src/gateway/interfaces/feishu/mod.rs:54    (config.validate()?)
```

Public callers: `load.rs`, `patcher.rs` (twice — pre-commit re-validation and rollback validate), `gateway/config.rs` (separate `GatewayConfig`), `gateway/interfaces/feishu/mod.rs` (separate `FeishuConfig`). The private `validate_*` helpers in `validate.rs` are only reachable through `Config::validate`.

### `migration`, `backup`, `defaults_override`, `presets_override` family

```bash
$ rg -n "migrate_mcp_builtin_in_toml|migrate_vector_db_in_toml" src/ interfaces/ shared/ desktop/
src/config/load.rs:215-216     (called by load pipeline)

$ rg -n "ConfigBackup::|\bConfigBackup\(" src/ interfaces/ shared/ desktop/
src/bin/aleph-server/commands/start/mod.rs:2853  (ConfigBackup::new(...), 10)
src/builtin_tools/file_ops/path_utils.rs:142 (config_path lookup)
src/gateway/handlers/config.rs (rollback surfaces)

$ rg -n "\binit_defaults_override\b|\bget_defaults_override\b|\bload_defaults_override\b" src/ interfaces/ shared/ desktop/
src/config/load.rs:208,267,342,347   (load pipeline)
src/config/types/provider.rs:253     (default_timeout_seconds factory)
src/config/types/generation/provider.rs:107  (default_timeout_seconds factory)

$ rg -n "\bload_presets_override\b|\bmerge_provider_preset\b|\bpartial_to_provider_preset\b|\bmerge_generation_preset\b|\bpartial_to_generation_preset\b" src/ interfaces/ shared/ desktop/
src/config/load.rs:262,352           (load pipeline)
src/providers/presets/mod.rs         (merge_provider_preset, partial_to_provider_preset)
src/config/types/generation/presets/mod.rs   (merge_generation_preset, partial_to_generation_preset)
```

### `agent_manager` / `agent_resolver` family

```bash
$ rg -n "AgentManager::new|AgentManager\(" src/ interfaces/ shared/ desktop/
src/bin/aleph-server/commands/start/mod.rs:1072
src/gateway/admin_api/{reconciler,secrets,mod}.rs
src/gateway/handlers/agents.rs:608
src/config/agent_manager/{agent_files,tests}.rs

$ rg -n "AgentDefinitionResolver::|ResolvedAgent\b|workspace_root_for|agents_root_for|default_workspace_root|default_agents_root|initialize_agent_identity|initialize_agent_dir" src/ interfaces/ shared/ desktop/
src/bin/aleph-server/commands/start/mod.rs:910,1072
src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1367
src/gateway/handlers/agents.rs:226
src/builtin_tools/agent_manage/{create,update,delete,context}.rs
src/builtin_tools/team/{create,from_template}.rs
src/teams/templates/materialize.rs,src/teams/member_provision.rs,src/thinker/identity_profile.rs,src/memory/scratchpad/manager.rs
```

All widely consumed.

### `PluginMarketplaceEntry`, `ChannelInstanceConfig`, `ConfigUiHints`

```bash
$ rg -n "PluginMarketplaceEntry" src/ interfaces/ shared/ desktop/
src/lib.rs:162 (re-export), src/bin/aleph-server/commands/plugins.rs:12, src/extension/marketplace/types.rs (From/impls), src/gateway/handlers/plugins/handlers/marketplace.rs:100

$ rg -n "ChannelInstanceConfig" src/ interfaces/ shared/ desktop/
src/lib.rs:162 (re-export); only consumed through `resolved_channels()`'s return type — fields read as `inst.channel_type`, `inst.id`, `inst.config` from `bin/aleph-server/commands/start/builder/subsystems.rs:304+`. No `use ChannelInstanceConfig;` outside `lib.rs`.

$ rg -n "ConfigUiHints|GroupMeta|FieldHint" src/ interfaces/ shared/ desktop/
src/lib.rs:148                   (ConfigUiHints re-export from guides not ui_hints — see mod.rs:48)
src/gateway/handlers/config.rs:319,375  (ConfigUiHints::new())
src/config/ui_hints/mod.rs       (definitions and field accesses only)
No `GroupMeta` or `FieldHint` named-imports outside `src/config/ui_hints/mod.rs`.
```

### Methods on `Config` (the `impl Config` blocks scattered across the module tree)

| Method | Source | Production caller(s) |
|---|---|---|
| `Config::new` | structs.rs:432 | (used in tests + as `Default` shortcut) |
| `Config::local_voice` | structs.rs:438 | `src/config/types/voice_local.rs:288` (in `normalize_voice_local`); `src/builtin_tools/voice_tools/local_voice.rs:47` |
| `Config::migrate_fetch` | structs.rs:449 | `src/config/load.rs:276` |
| `Config::resolved_channels` | structs.rs:334 | `src/bin/aleph-server/commands/start/builder/subsystems.rs:288,677,743,794,867` |
| `Config::get_default_provider` | methods.rs:46 | **NONE** (see `Config::default_provider` family above) |
| `Config::set_default_provider` | methods.rs:64 | `src/gateway/handlers/providers/handlers.rs:1011` |
| `Config::get_effective_tools_config` | methods.rs:23 | `src/executor/builtin_registry/builder/constructor/mod.rs:422` |
| `Config::add_rule_at_top` | methods.rs:109 | `src/gateway/handlers/routing_rules.rs:157` |
| `Config::remove_rule` | methods.rs:136 | `src/gateway/handlers/routing_rules.rs:318` |
| `Config::move_rule` | methods.rs:183 | `src/gateway/handlers/routing_rules.rs:384` |
| `Config::get_rule` | methods.rs:233 | `src/gateway/handlers/routing_rules.rs:78` |
| `Config::rule_count` | methods.rs:242 | `src/gateway/handlers/routing_rules.rs:217,298` |
| `Config::validate` | validate.rs:69 | `src/config/load.rs:279`; `src/config/patcher.rs:393,591` |
| `Config::log_advisories` | validate.rs:654 | `src/bin/aleph-server/commands/start/builder/subsystems.rs:153` |
| `Config::effective_path` | load.rs:99 | `bin/aleph-server/main.rs`, `bin/aleph-server/commands/start/{mod.rs:1073,1849,3466}`, `gateway/hot_reload.rs:77`, `gateway/config.rs:483`, `gateway/handlers/security_config.rs:334`, `builtin_tools/file_ops/path_utils.rs:151`, `diagnostics/mod.rs:78` |
| `Config::default_path` | load.rs:46 | `cli/config_cmd.rs` (separate `CliConfig`, not this one) |
| `Config::set_effective_path` | load.rs:64 | `bin/aleph-server/main.rs:79` |
| `Config::decline_effective_path` | load.rs:88 | `bin/aleph-server/main.rs:86` |
| `Config::load_from_file` | load.rs:118 | `src/diagnostics/checks/config_parse.rs:85` |
| `Config::load_from_file_reporting_dead_keys` | load.rs:166 | `src/diagnostics/checks/config_parse.rs:85` |
| `Config::load` | load.rs:289 | ~10 production callers (see §"Inventory — Config field-reader") |
| `Config::save_to_file` | save.rs:240 | internal save.rs |
| `Config::save` | save.rs:317 | internal save.rs |
| `Config::save_incremental` | save.rs:357 | internal save.rs |
| `Config::save_incremental_to_file` | save.rs:381 | internal save.rs |
| `Config::merge_builtin_rules` | load.rs:379 (`pub(crate)`) | `src/config/load.rs:256` |
| `Config::migrate_mcp_builtin_in_toml` | migration.rs:21 (`pub(crate)`) | `src/config/load.rs:215` |
| `Config::migrate_vector_db_in_toml` | migration.rs:68 (`pub(crate)`) | `src/config/load.rs:216` |

### Top-level ConfigPatcher / live_apply / etc. methods (consumer count)

| Method | Used by |
|---|---|
| `ConfigPatcher::new` | 7 callers (start/mod.rs, builtin_tools/{self_config,moa_manage}, gateway/handlers/{config,moa,security_config}, providers/moa/preset_store) |
| `ConfigPatcher::with_vault` | `bin/aleph-server/commands/start/mod.rs:1856` |
| `ConfigPatcher::config_path` | `gateway/handlers/security_config/{toml_io,rate_limit}.rs` (5 calls) |
| `ConfigPatcher::apply` | `gateway/handlers/config.rs:558`; `builtin_tools/self_config.rs:443`; `providers/moa/preset_store.rs:60` |
| `ConfigPatcher::list_backups` | `builtin_tools/self_config.rs:570` |
| `ConfigPatcher::rollback` | `builtin_tools/self_config.rs:611` |
| `apply_live_sections` | `gateway/handlers/{route_config,execution_config}.rs`; `config/patcher.rs:340,624` |
| `classify_verified` | `gateway/handlers/execution_config.rs:133`; `config/live_apply.rs:200` (own test) |

### Top-level ConfigBackup methods (consumer count)

| Method | Used by |
|---|---|
| `ConfigBackup::new` | `bin/aleph-server/commands/start/mod.rs`; `gateway/handlers/config.rs`; `builtin_tools/file_ops/path_utils.rs` |
| `ConfigBackup::default_dir` | `bin/aleph-server/commands/start/mod.rs` |
| `ConfigBackup::create_snapshot` | `config/patcher.rs` |
| `ConfigBackup::cleanup` | `config/patcher.rs` (intra `create_snapshot`) |
| `ConfigBackup::resolve` | `config/patcher.rs` (rollback entrypoint) |
| `ConfigBackup::list` | `ConfigPatcher::list_backups` re-export; `config/patcher.rs` |

## Findings

### sw-config-1 — `Config::defaults_override` is parsed, persisted, and assigned — but never read

- **Module:** `src/config`
- **Files:** `src/config/structs.rs:299` (field def), `src/config/load.rs:266,346` (assignments)
- **Severity:** low (painless)
- **Form:** 3 (inert config field)
- **Produced:** `pub defaults_override: DefaultsOverride` — a field on `Config` that carries the parsed `~/.aleph/defaults.toml` overrides. `serde(skip)` so it is not serialized back; the field exists only to keep `Config` self-contained for in-memory arithmetic.
- **Produced location:** `src/config/structs.rs:299`
- **Consumer location:** none in production
- **Evidence:**
  ```bash
  $ rg -n "(\.|->)defaults_override\b" src/ interfaces/ shared/ desktop/ -g '!src/config/**'
  (no matches)

  $ rg -n "\.defaults_override\." src/ interfaces/ shared/ desktop/
  (no matches)

  $ rg -n "config\.defaults_override\b|cfg\.defaults_override\b|app_config\.defaults_override\b|loaded_app_config\.defaults_override\b|full_config\.defaults_override\b" src/ interfaces/ shared/ desktop/
  (no matches)
  ```
  All matches for the symbol are inside `src/config/`:
  - definition + Default: `structs.rs:299,427`
  - assignment in `load.rs:266,346` (`config.defaults_override = ...get_defaults_override().clone();`)
  - serde skip attribute: `structs.rs:297`
  - accessor functions `provider_timeout_seconds` / `generation_timeout_seconds` operate on the **process-global singleton** `DefaultsOverride::get_defaults_override()` (`defaults_override.rs:185,190`), not on the field.
- **Decision:** CUT (with caveat — see Rationale)
- **Rationale:** The actual reader of `defaults.toml` is the process-global `CapabilitySlot` (`defaults_override.rs:60,138`). Every `default_*()` factory (`src/config/types/provider.rs:251-255`, `src/config/types/generation/provider.rs:104-108`) calls `crate::config::defaults_override::get_defaults_override()`, which returns a static `EMPTY_DEFAULTS_OVERRIDE` or the installed singleton — **never** `cfg.defaults_override`. The field on `Config` is an artifact of structural symmetry ("`Config` owns everything it parses"), but since serde skips it and no caller reads it, it's three assignments of work (load.rs:266,346) for zero behavioral effect. The painless-wire heuristic applies: no operator has reported an empty `defaults.toml` taking effect when one was present, because the singleton path works regardless of whether the field is assigned.
- **Caveat — "what was the assignment for?":** Inspecting `load.rs:266,346` shows the field is assigned only so `Config::default()` returns a struct that "looks complete" (the constructor in `Default::default()` at `structs.rs:422` initialises both override fields with empty values). The `load` paths clone from the singleton rather than re-parse `defaults.toml`, so the field *always* carries the same value the singleton carries. Cutting the field would not change runtime behavior.
- **Proposed change:**
  1. Drop `pub defaults_override: DefaultsOverride` from `Config` (`structs.rs:299`) and from `Default::default()` (`structs.rs:427`).
  2. Remove the two `config.defaults_override = ...clone();` lines in `load.rs:266,346`.
  3. Update `src/lib.rs`'s `pub use` block only if `defaults_override` was re-exported by name (it is **not** — only `DefaultsOverride` is reached via `pub use types::*;` re-export of nothing here; the singleton accessor is `pub fn get_defaults_override` directly on `defaults_override.rs:138`).
  4. Remove the unused `Deserialize` derives from `DefaultsOverride` if they are now dead (the file still parses its own `defaults.toml` standalone, so the derives stay; only the field on `Config` is removed).
- **Verification:**
  - `rg -n "Config\.defaults_override|cfg\.defaults_override|config\.defaults_override\." src/` should return 0 matches after the cut.
  - `cargo test -p alephcore --lib config::defaults_override` should still pass — the standalone file is unaffected.
  - `cargo test -p alephcore --lib config::load` should pass (the assignments are just drops).
- **Risk:** low. The field is `serde(skip)` — removal does not change wire format or persistence. Behavior is unchanged because every actual reader goes through `get_defaults_override()`.

### sw-config-2 — `Config::mcp` field is read only by `UnifiedToolsConfig::from_legacy`, otherwise inert

- **Module:** `src/config`
- **Files:** `src/config/structs.rs:86` (field), `src/config/methods.rs:27` (sole reader)
- **Severity:** low
- **Form:** 3 (inert config field — borderline; one consumer but no behavior dependence on the data being live)
- **Produced:** `pub mcp: McpConfig` — `#[serde(default)]`, parsed on every load, serialized back on every save.
- **Produced location:** `src/config/structs.rs:86`
- **Consumer location:** `src/config/methods.rs:27` (`UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)`) only.
- **Evidence:**
  ```bash
  $ rg -n "config\.mcp\b|cfg\.mcp\b|app_config\.mcp\b|loaded_app_config\.mcp\b|full_config\.mcp\b" src/ interfaces/ shared/ desktop/
  src/config/tests/tools.rs:20    (assert!(config.mcp.is_empty()))
  src/config/tests/tools.rs:243   (config.mcp.insert(...))
  src/config/methods.rs:27       (UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp))
  ```
  No production caller reads `config.mcp.adapters`, `config.mcp.external_servers`, etc. The only non-test reader of the field is the legacy→unified migration helper, which **only** uses `mcp.enabled` and `mcp.external_servers` (the latter copied into `unified_tools.mcp`).
- **Decision:** KEEP (with note)
- **Rationale:** `McpConfig` is the **legacy** MCP config surface (the `[mcp]` section in older `config.toml` files). The migration `migrate_mcp_builtin_in_toml` (`migration.rs:21-64`) handles `[mcp.builtin] → [tools]`, but the legacy **non-builtin** path (`mcp.external_servers`) is read by `UnifiedToolsConfig::from_legacy` whenever `unified_tools` is absent. This is the live migration bridge; cutting it would lose the data of every operator who hasn't yet moved to `[unified_tools]`. The data is also persisted on save (because `Config` serializes all non-skipped fields), so the round-trip preserves it. The section's reach is narrow but real.
- **Proposed change:** none. The bridge is load-bearing for legacy config files and the unification migration in `methods.rs:23-39` is the documented migration path.
- **Verification:**
  - `rg -n "Config::mcp|McpConfig::default|McpConfig\b" src/` continues to show `methods.rs:23-39` as the only consumer.
  - `cargo test -p alephcore --lib config::tests::tools` continues to assert the migration.
- **Risk:** low if left as-is. Mentioned for completeness because the lens flagged it.

### sw-config-3 — `Config::get_default_provider()` has zero production callers

- **Module:** `src/config`
- **Files:** `src/config/methods.rs:46`
- **Severity:** low
- **Form:** 1 (no consumer)
- **Produced:** `pub fn Config::get_default_provider(&self) -> Option<String>` — returns `Some(name.to_string())` if `general.default_provider` names an enabled provider, else `None`.
- **Produced location:** `src/config/methods.rs:46`
- **Consumer location:** none in production
- **Evidence:**
  ```bash
  $ rg -n "\bget_default_provider\(\)" src/ interfaces/ shared/ desktop/
  (no matches)

  $ rg -n "Config::get_default_provider" src/ interfaces/ shared/ desktop/
  src/config/methods.rs:46   (definition only)
  ```
  Note: `GenerationConfig::get_default_provider(GenerationType)` at `src/config/types/generation/config.rs:149` is a *different* method on a *different* struct; its callers in `src/config/types/generation/mod.rs:57,61,64,65` are tests, not production readers.
- **Decision:** CUT (or move to test-only)
- **Rationale:** `Config::get_default_provider` is the mirror of `Config::set_default_provider` (`methods.rs:64`, used by `gateway/handlers/providers/handlers.rs:1011`). The setter is real because the Panel wires `set_default_provider` and it must validate that the named provider exists and is enabled. The getter has no live consumer — the Panel reads the default-provider name from `config.general.default_provider` directly (cf. `bin/aleph-server/commands/start/builder/agent_init/provider_registry.rs:54,99,117`). This is the textbook "no consumer" case where the "setter without getter" is what the rest of the code wanted, and the getter was symmetric scaffolding.
- **Proposed change:** delete `Config::get_default_provider` (`methods.rs:46-58`). No call site changes required.
- **Verification:**
  - `rg -n "Config::get_default_provider|config\.get_default_provider\(\)" src/` should return 0 matches.
  - `cargo test -p alephcore --lib config::methods` should pass; the method had no tests.
- **Risk:** low.

### sw-config-4 — `pub` types in `ui_hints` (`GroupMeta`, `FieldHint`) have no name-imported consumers

- **Module:** `src/config/ui_hints`
- **Files:** `src/config/ui_hints/mod.rs:20,35`
- **Severity:** low (informational — see sw-canvas-3 in the 2026-08-17 audit for parallel)
- **Form:** 6 (orphaned public API surface — borderline)
- **Produced:** `pub struct GroupMeta { label, order, icon }`; `pub struct FieldHint { label, help, group, order, advanced, sensitive, placeholder }`. Both are exported as fields of the also-`pub` `ConfigUiHints` struct, so external callers can read `ConfigUiHints::groups` and dereference `GroupMeta` field accesses — but no caller needs to **name** the type.
- **Produced location:** `src/config/ui_hints/mod.rs:20,35`
- **Consumer location:** none by name. The struct fields ARE consumed through `ConfigUiHints::groups` / `ConfigUiHints::fields` but no `use ui_hints::GroupMeta;` or `use ui_hints::FieldHint;` exists.
- **Evidence:**
  ```bash
  $ rg -n "use crate::config::ui_hints::GroupMeta|use crate::config::ui_hints::FieldHint|ui_hints::GroupMeta|ui_hints::FieldHint" src/ interfaces/ shared/ desktop/
  (no matches)

  $ rg -n "\bGroupMeta\b|\bFieldHint\b" src/ interfaces/ shared/ desktop/
  src/config/ui_hints/mod.rs:20,35  (definitions)
  src/config/ui_hints/mod.rs:66,68  (field types inside ConfigUiHints)
  ```
- **Decision:** KEEP (no action)
- **Rationale:** Both types are exposed via the public `ConfigUiHints.groups` / `ConfigUiHints.fields` fields (`ui_hints/mod.rs:66,68`). Because `ConfigUiHints` is `pub` and is constructed via `ConfigUiHints::new()` (returning `Default`), the field types **must** be `pub` for the schema to be addressable from outside the module. The struct itself IS consumed — `ConfigUiHints::new()` is the only consumer that ships, and the only callsite is `gateway/handlers/config.rs:375` (`ui_hints: crate::config::ConfigUiHints::new()`). This is not a severed wire; the types have live access via field-deref. Informational only.
- **Proposed change:** none. The `pub use ui_hints::ConfigUiHints` re-export in `mod.rs:48` and the `ConfigUiHints` exposure are correct.
- **Verification:**
  - `cargo test -p alephcore --lib config::ui_hints` continues to pass.
  - The 2026-08-12 audit's `build_ui_hints()` producer half is already CUT (the field is documented as empty until a schema-driven UI is wired).
- **Risk:** none.

### sw-config-5 — `pub(crate)` helpers (`is_fs_enabled` etc.) with `#[allow(dead_code)]`

- **Module:** `src/config/types`
- **Files:** `src/config/types/tools.rs:422-452`, `src/config/types/routing.rs:194-204`, `src/config/types/agents_def.rs:163`, `src/config/types/generation/config.rs:166`
- **Severity:** low (informational)
- **Form:** smell (perf/maintainability)
- **Produced:** `is_fs_enabled`, `is_git_enabled`, `is_shell_enabled`, `is_system_info_enabled` (`tools.rs`); `get_intent_type`, `get_preferred_model` (`routing.rs`); `AgentModelRef::model_str` (`agents_def.rs:163`); plus one in `generation/config.rs:166`.
- **Produced location:** see file:line above
- **Consumer location:** none outside their own in-module tests
- **Evidence:**
  ```bash
  $ rg -n "\bis_fs_enabled\b|\bis_git_enabled\b|\bis_shell_enabled\b|\bis_system_info_enabled\b" src/ interfaces/ shared/ desktop/
  src/config/types/tools.rs:422,432,440,449  (definitions + internal tests)

  $ rg -n "\bget_intent_type\b|\bget_preferred_model\b" src/ interfaces/ shared/ desktop/
  src/config/types/routing.rs:194,203  (definitions + internal tests)

  $ rg -n "\bmodel_str\b" src/ interfaces/ shared/ desktop/
  src/config/types/agents_def.rs:163  (definition + internal tests)
  ```
  Each is `pub(crate)` and flagged with `#[allow(dead_code)]` plus a doc-comment acknowledging "no production caller". The `allow` attributes are masking non-dead code (they're `pub(crate)` helpers that the dead-code lint would otherwise flag) rather than severed wires.
- **Decision:** KEEP (no action) — but the allow attributes should be revisited
- **Rationale:** Each `#[allow(dead_code)]` is paired with a doc-comment that explains the *real* reason: "the master-and-per-tool gate is read by sibling `config` modules via the `native.fs` field directly". The functions are **not** severed wires — they're convenience accessors whose consumers chose a different field path. They have internal tests. The `#[allow(dead_code)]` legitimately suppresses the false positive, but the doc-comment is the load-bearing justification: a future reader cannot tell whether the helper is "live with a different reader path" or "drop scaffolding". The doc-comments are good. This finding is recorded because the seam-catalog mentions `#[allow(dead_code)]` masking severed functions as a smell to watch for, and these attributes warrant a glance — but no action required.
- **Proposed change:** leave as-is. If the team wants to reduce noise, the helpers could be moved to `#[cfg(test)]` or replaced with field access (one-liner), but neither change is a fix to a severed wire.
- **Verification:**
  - The allow attributes remain paired with their rationale comments.
  - `cargo test -p alephcore --lib config::types` continues to pass.
- **Risk:** none.

### sw-config-6 — Atomic write path in `save.rs` is correct; no data-loss risk on crash

- **Module:** `src/config`
- **Files:** `src/config/save.rs:83-135` (`write_atomically`), `save.rs:332,450` (callers)
- **Severity:** low (smell — verified safe)
- **Form:** smell (correctness, verified)
- **Produced:** `fn write_atomically(path: &Path, contents: &str) -> Result<()>` — write-to-temp + `sync_all()` (Unix) + atomic `fs::rename`.
- **Produced location:** `src/config/save.rs:83`
- **Consumer location:** `save.rs:332` (`save_to_file`), `save.rs:450` (`save_incremental_to_file`).
- **Evidence:** Code excerpt:
  ```rust
  // src/config/save.rs:83-135
  fn write_atomically(path: &Path, contents: &str) -> Result<()> {
      let temp_path = path.with_extension("tmp");
      fs::write(&temp_path, contents).map_err(...)?;
      #[cfg(unix)] {
          let file = std::fs::OpenOptions::new().write(true).open(&temp_path)?;
          file.sync_all().map_err(...)?;       // fsync the temp file's data
      }
      fs::rename(&temp_path, path).map_err(...)?;  // atomic rename
      Ok(())
  }
  ```
  Temp file is in the same directory (preserves filesystem for `rename(2)`), `fsync` ensures data is on disk before the rename, the rename is atomic.
- **Decision:** KEEP (no action)
- **Rationale:** This is the textbook correct atomic-write implementation: temp+fsync+rename on the same filesystem. The seam-catalog specifically calls out "missing fsync is not [good]" — this code has fsync. Two follow-on safety nets exist at the higher layer:
  - `guard_against_section_loss` (`save.rs:42-65`) refuses to write when in-memory is empty but on-disk has providers (prevents data-loss from a mis-isolated test or a partial load).
  - `guard_incremental_memory` / `guard_incremental_providers` (`save.rs:478-573`) refuse the incremental-save path when it would erase existing providers.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-7 — `Config::migrate_fetch` fold path has a graceful-degradation safety net

- **Module:** `src/config`
- **Files:** `src/config/structs.rs:441-484` (definition), `src/config/load.rs:276` (caller)
- **Severity:** low (verified safe)
- **Form:** smell (correctness, verified)
- **Produced:** `pub fn Config::migrate_fetch(&mut self)` — folds legacy `[policies.web_fetch.crawl4ai]` into the new `[fetch]` section, no-op if `[fetch]` is already present.
- **Produced location:** `src/config/structs.rs:441`
- **Consumer location:** `src/config/load.rs:276` (called once per load).
- **Evidence:** Code excerpt:
  ```rust
  // src/config/structs.rs:441-484
  pub fn migrate_fetch(&mut self) {
      if self.fetch.is_some() { return; }              // new wins
      let c4 = &self.policies.web_fetch.crawl4ai;
      if c4.base_url.is_empty() && !c4.enabled { return; }  // legacy absent
      // ... build the [fetch] section from c4 ...
  }
  ```
  Plus a doc-test for "no-op when [fetch] already present" at `structs.rs:540-551`.
- **Decision:** KEEP (no action)
- **Rationale:** The migration is safe, two-sided, and idempotent. The legacy vault key `web_fetch:crawl4ai` is also still read by the fetch registry as a fallback, so secrets survive without rewrite (per the `migrate_fetch` doc comment).
- **Proposed change:** none.
- **Risk:** none.

### sw-config-8 — `live_apply` arm coverage is enforced by a compile-time guard test

- **Module:** `src/config`
- **Files:** `src/config/live_apply.rs:610-633` (`every_live_section_has_an_apply_arm`)
- **Severity:** low (verified safe)
- **Form:** verified (compile-time guard)
- **Produced:** The `apply_live_sections` function (live_apply.rs:51) has 5 match arms for the 5 declared-live targets (`route`, `execution`, `behavior`, `policies.spend`, `policies.terminal`). The test `every_live_section_has_an_apply_arm` (live_apply.rs:610) declares the same `known_arms` list and asserts that `LIVE_SECTIONS ∪ LIVE_SUBSECTIONS == known_arms`.
- **Produced location:** `src/config/live_apply.rs:51-180`
- **Consumer location:** live_apply.rs is the single chokepoint reached from `ConfigPatcher::apply` (`patcher.rs:340`) and `ConfigPatcher::rollback` (`patcher.rs:624`) — and additionally from two dedicated handlers (`route_config.rs:248`, `execution_config.rs:132`).
- **Evidence:**
  - Live_apply's own doc (`live_apply.rs:14-31`) states "**The table that says 'live' and the code that makes it live are the same table.**" enforced by the guard test.
  - Live_apply's source-scan census (`live_apply.rs:683-790`, `every_dedicated_config_handler_that_saves_a_live_section_calls_apply_live_sections`) walks `src/gateway/handlers/` for `save_incremental(&["<live>"])` calls and asserts each is paired with an `apply_live_sections(...)` call in the same handler function.
- **Decision:** KEEP (no action — this is the canonical compile-time guard)
- **Rationale:** This is the **best** tier of the seam-catalog guard hierarchy — a real, single-source-of-truth test that fails at `cargo test` time if either list drifts. A new `LIVE_SECTIONS` entry without an arm fails the test; a new arm without a `LIVE_SECTIONS` declaration fails it too. This audit's lens-1/2/3 work was effectively a no-op because the guard already enforces parity.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-9 — `dead_keys` census test enforces reader-for-foreign-owned paths

- **Module:** `src/config/dead_keys.rs`
- **Files:** `src/config/dead_keys.rs:392-410` (`every_foreign_owned_entry_still_has_its_reader`)
- **Severity:** low (verified safe)
- **Form:** verified (compile-time guard, complementary to sw-config-8)
- **Produced:** The `TOLERATED` allowlist in `dead_keys.rs:80-170` marks two paths as "foreign-owned" (`gateway`, `security.ssrf`) because other modules read them.
- **Consumer location:** the test asserts the reader symbols (`pub fn load_default(` in `src/gateway/config.rs`; `fn apply_security_ssrf_overrides(` in `src/config/load.rs`) still exist. If either is deleted, the test fails.
- **Evidence:** Code excerpt at `dead_keys.rs:392-410`.
- **Decision:** KEEP (no action)
- **Rationale:** The test catches the precise failure mode the seam-catalog names ("a file named for a dead tool can still export an enum a live config field consumes — the `CleanupPolicy` E0432 lesson"). A future deletion of either reader would silently re-licence `Config` to ignore those keys.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-10 — Patcher's `_ => false` arm has a deliberate compile-time guard

- **Module:** `src/config`
- **Files:** `src/config/live_apply.rs:163-165`, `reload_impact.rs:67-105`
- **Severity:** low (verified safe)
- **Form:** verified (companion to sw-config-8)
- **Produced:** `match *target { "route" => ..., _ => false }` in `live_apply.rs:163-165`. The catch-all returns `false` (i.e. "did not land") and the verdict downgrades to `Restart` honestly via `classify_verified` (`live_apply.rs:195-211`).
- **Consumer location:** `live_apply` is reached from `ConfigPatcher::apply`/`rollback` (`patcher.rs:340,624`) and from `gateway/handlers/{route_config,execution_config}.rs`.
- **Evidence:** see sw-config-8.
- **Decision:** KEEP (no action)
- **Rationale:** The catch-all is honest — it returns `false`, which `classify_verified` converts to a `Restart` verdict, which the patcher surfaces as the right user-facing message. The guard test `every_live_section_has_an_apply_arm` (live_apply.rs:610) prevents the catch-all from being silently reached in production.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-11 — `Config::load` writes a default `config.toml` if none exists — known footgun, has isolated doc and tests

- **Module:** `src/config`
- **Files:** `src/config/load.rs:289-360`
- **Severity:** medium (documented intentional behavior, but worth flagging)
- **Form:** smell (correctness, documented)
- **Produced:** `Config::load()` writes a default config file when none exists at `effective_path()`.
- **Consumer location:** many callers; the doc-comment of `Config::load` itself explicitly documents the behavior.
- **Evidence:**
  ```rust
  // src/config/load.rs:289-360 — at least 8 callers explicitly note
  // "Config::load() writes a default config when none exists" in their own
  // doc comments. Example: src/utils/paths.rs:134, src/skill/mod.rs:74,
  // src/builtin_tools/file_ops/path_utils.rs:142,151,1337.
  ```
- **Decision:** KEEP (no action; documented behavior)
- **Rationale:** Every load caller is aware that a missing config file yields a default-written one — this is intentional and extensively documented. The seam-catalog mentions "missing size bounds on arrays/vectors that an operator config can grow" — the only field with no bound is `Config.channels` (opaque `HashMap<String, serde_json::Value>`) and `Config.profiles`/`profiles.*.cache_strategy` (already retired per `dead_keys.rs:122-126`); neither is a size-unbounded collection that an operator could grow to exhaust memory.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-12 — `Config::save_to_file` test guard (`reject_real_home_config`) covers the only documented footgun

- **Module:** `src/config`
- **Files:** `src/config/save.rs:14-44,366-372`
- **Severity:** low (verified safe)
- **Form:** verified
- **Produced:** `#[cfg(test)] fn reject_real_home_config(path: &Path, caller: &str)` — asserts the target file is not `~/.aleph/config.toml` (the developer's real config).
- **Consumer location:** called at the entry of `save_to_file` (save.rs:244) and `save_incremental_to_file` (save.rs:386).
- **Evidence:** see sw-config-6 and the doc comment at `save.rs:14-43`.
- **Decision:** KEEP (no action)
- **Rationale:** This guard covers exactly the mis-isolation failure mode the seam-catalog names ("a unit test that reaches a persist branch writes whatever `~/.aleph/config.toml` happens to be"). The doc comment (`save.rs:14-43`) explicitly enumerates which test surfaces it covers and which it does not (`tests/*.rs` integration binaries) — the limits are honest.
- **Proposed change:** none.
- **Risk:** none.

### sw-config-13 — `defaults_override` singleton "latch" fix is documented and pinned by tests

- **Module:** `src/config/defaults_override.rs`
- **Files:** `src/config/defaults_override.rs:75-110` (`EMPTY_DEFAULTS_OVERRIDE` static), `defaults_override.rs:264-290` (`the_accessor_exposes_this_handle_to_the_roster`)
- **Severity:** low (verified safe)
- **Form:** verified
- **Produced:** A separate static `EMPTY_DEFAULTS_OVERRIDE: DefaultsOverride` that the slot falls back to when uninstalled, **deliberately outside the `CapabilitySlot`** (to avoid the latched-install bug that the doc comment describes).
- **Consumer location:** `defaults_override.rs:140` (`get_defaults_override`).
- **Evidence:** Code at `defaults_override.rs:75-110` plus the doc comment explaining the prior failure mode (a process that loaded with no config dir latched the empty override, then a later load that found one was told "already initialized; ignoring re-init" and silently discarded the operator's `defaults.toml`). The fix is a static read-through fallback.
- **Decision:** KEEP (no action)
- **Rationale:** This is a textbook capability-slot footgun (form 2: stub far-end that lies about its install state). The fix is correct, the test pins the literal against accidental widening, and the doc comment explicitly enumerates the blast-radius check ("every callers `.clone()`s or reads a field immediately"). This audit's lens-2 work is essentially a no-op for this module.
- **Proposed change:** none.
- **Risk:** none.

## Symbols that PASS the parity check

The remaining surface is healthy:

### Healthy `Config` fields (all read in production)
Every `Config` field **except `defaults_override`** has at least one non-test reader outside `src/config/`. The reader counts from §"Config field-reader parity" are listed in the table. Notable healthy fields:
- **`general`** (74) — `config.general.language`, `config.general.default_provider`, `config.general.fallback_providers`, `config.general.session_store_backend`, `config.general.browser` are read across many surfaces.
- **`memory`** (819) — read by every subsystem (embedding, dreaming, assembler, retrieval, notes, ingest, expansion, etc.).
- **`providers`** (663) — read at boot (provider_registry), on every patch (gateway/handlers/providers), and on health-check.
- **`rules`** (85) — `tool_catalog_init.rs:142` registers them into `tool_metadata`.
- **`policies`** (195) — spend, terminal, tool_permissions, exec_tier, memory.compression, memory.session_compactor, web_fetch.crawl4ai, metrics all read.
- **`route`** (272) — read by `route_handle::global_route_handle(&config.route)`, `route_observe`, and the route policy logic.
- **`voice_local`** (18) — `voice_local.streaming`, `voice_local.format`, `voice_local.local` all read by `gateway/handlers/voice.rs` and `builtin_tools/voice_tools/`.
- **`agents`** (666) — read by `agent_init/mod.rs`, `gateway/config.rs`, `agent_manage/*`, `teams/templates/materialize.rs`.
- **`bindings`** (38) — `AgentRouter::from_bindings(loaded_app_config.bindings.clone(), ...)`.
- **`session`** (1047) — `routing::SessionConfig` is read by session_key, dm_scope, and channel routing.
- **`projects`** (416) — `cfg.projects.allowed_roots` gates the entire `fs.*` RPC family.
- **`acp`** (191) — `acp.adapters` is the source-of-truth list of harnesses.
- **`channels`** (293) — `cfg.resolved_channels()` builds every channel instance at boot.

### Healthy methods on `Config`
- **`Config::resolved_channels`** — used 5× in `bin/aleph-server/commands/start/builder/subsystems.rs` to register each channel.
- **`Config::local_voice`** — used by `voice_local::normalize_voice_local` and `voice_tools/local_voice.rs`.
- **`Config::migrate_fetch`** — called once per load.
- **`Config::load` / `load_from_file` / `save` / `save_to_file`** / `save_incremental_to_file` / `effective_path` / `default_path` / `set_effective_path` / `decline_effective_path` — all load-bearing; many callers.
- **`Config::validate`** — called by `load`, `patcher::apply` (twice), `patcher::rollback`.
- **`Config::get_effective_tools_config`** — `executor/builtin_registry/builder/constructor/mod.rs:422`.
- **`Config::set_default_provider`** — `gateway/handlers/providers/handlers.rs:1011`.
- **`Config::add_rule_at_top` / `remove_rule` / `move_rule` / `get_rule` / `rule_count`** — `gateway/handlers/routing_rules.rs`.

### Healthy `ConfigPatcher` methods
All 6 `pub fn`s (`new`, `with_vault`, `config_path`, `apply`, `list_backups`, `rollback`) are used. The `pub(crate) fn validate_schema` is the canonical schema validator, also referenced in `extension/manifest/config_validation.rs`'s doc comment.

### Healthy `live_apply` / `reload_impact`
- **`apply_live_sections`** — used by `ConfigPatcher::apply` and `ConfigPatcher::rollback`, plus the two dedicated handlers.
- **`classify_verified`** — used by `gateway/handlers/execution_config.rs:133`.
- **`ReloadImpact`** (the enum) — used by 8+ sites: `gateway/handlers/{browser_config,config,route_config}.rs`, `builtin_tools/{self_config,agent_manage/update}.rs`, `interfaces/webchat/...`.
- **`ReloadImpact::classify`, `agent_hint`, `user_hint_zh`** — each has 5+ callers.

### Healthy `defaults_override` / `presets_override` / `backup`
- **`defaults_override::init_defaults_override` / `get_defaults_override`** — load pipeline + 2 factory functions.
- **`defaults_override::provider_timeout_seconds` / `generation_timeout_seconds`** — consumed by `provider.rs:251-255` and `generation/provider.rs:104-108`.
- **`presets_override::load_presets_override`** — load pipeline.
- **`presets_override::merge_provider_preset` / `partial_to_provider_preset` / `merge_generation_preset` / `partial_to_generation_preset`** — `providers/presets/mod.rs` and `config/types/generation/presets/mod.rs`.
- **`backup::ConfigBackup::new / default_dir / create_snapshot / cleanup / resolve / list`** — used by `bin/aleph-server/commands/start/mod.rs`, `builtin_tools/file_ops/path_utils.rs`, `gateway/handlers/config.rs`, `ConfigPatcher::{apply, rollback, list_backups}`.

### Healthy `agent_manager` / `agent_resolver`
- **`AgentManager::new / list / get / create / update / delete / set_default`** — `gateway/handlers/agents.rs`, `builtin_tools/agent_manage/*`, `bin/aleph-server/commands/start/mod.rs`, `gateway/admin_api/*`.
- **`AgentManager::provisioning_roots`** — `teams/templates/materialize.rs`, `builtin_tools/{agent_manage/{create,delete},team/create}.rs`.
- **`is_curated_owned` / / `curated_owned_reason`** — `thinker/identity_files.rs`.
- **`AgentDefinitionResolver::new / resolve_all / resolve`** — `bin/aleph-server/commands/start/mod.rs`, `agent_init/mod.rs`, `gateway/handlers/agents.rs`.
- **`ResolvedAgent`** — `gateway/agent_instance.rs`, `gateway/agent_env/mod.rs`.
- **`initialize_agent_dir` / `initialize_agent_identity`** — `teams/templates/materialize.rs`, `builtin_tools/{agent_manage/create, team/create}.rs`.
- **`workspace_root_for` / `agents_root_for` / `default_workspace_root` / `default_agents_root`** — `gateway/agent_instance.rs`, `gateway/config.rs`, `gateway/agent_env/mod.rs`, `memory/scratchpad/manager.rs`.
- **`default_soul` / `default_agents` / `default_identity` / `DEFAULT_MEMORY` / `DEFAULT_TOOLS` / `DEFAULT_HEARTBEAT`** — `agent_resolver/mod.rs` only (all `pub(crate)`); the templates.rs doc explains that `default_identity` is the only one called from outside (`thinker/identity_profile.rs:364`); the rest are intra-module.
- **`resolve_model_ref`** — `agent_resolver/mod.rs` only; the public reach is via `AgentDefinitionResolver`.

### Healthy `ui_hints`
- **`ConfigUiHints::new`** — `gateway/handlers/config.rs:375`.
- The struct fields `groups` / `fields` are accessed through the same struct return type — no caller needs to name `GroupMeta` or `FieldHint`. (See sw-config-4 for the orphan-type note; not a wire.)

### Healthy `types/` re-exports
The `pub use types::*` re-exports ~250 type names. Spot checks of all major types (`AcpConfig`, `AgentModelRef`, `AgentsConfig`, `BehaviorConfig`, `ContextBudgetToml`, `Crawl4aiConfig`, `CustomLeakPattern`, `CustomMaskPattern`, `CustomPiiRule`, `CustomPiiSeverity`, `CustomRiskPattern`, `DispatchConfig` (no — that's `teams::dispatcher`), `ExecutionConfig`, `FallbackProviderToml`, `FetchConfigInternal`, `GeneralConfig`, `GenerationConfig`, `GenerationProviderConfig`, `GroupChatConfig`, `GuardrailsToml`, `LoadBalanceStrategy`, `LocalVoiceConfig` (`VoiceLocalConfig`), `McpConfig`, `McpExternalServerConfig`, `MetricsPolicy`, `MoaSlot`, `MoaFanout`, `MoaPreset`, `MoaToml`, `ModelRouteConfig`, `ModelThresholdToml`, `PermissionMatch`, `PiiAction`, `PlatformPiiPolicy`, `PoliciesConfig`, `PrivacyConfig`, `ProfileConfig`, `ProviderRateLimit`, `PromptSectionConfig`, `ProviderConfig`, `ResumeConfig`, `RouteBinding`, `RouteMode`, `RoutingRuleConfig`, `SearchBackendConfig`, `SearchConfigInternal`, `SecretProviderConfig`, `SecretsConfig`, `SessionConfig`, `SessionMode`, `ShellSecurityConfig`, `SpendPeriod`, `SpendPolicy`, `SsrfPolicy`, `StabilityToml`, `StopHookConfig`, `StrategyToml`, `StreamingConfig`, `FormatConfig`, `TeamBroadcastConfigToml`, `TeamDispatcherConfigToml`, `TeamMessagesConfigToml`, `TerminalConfig`, `ToolPermissionsConfig`, `ToolsConfig`, `UnifiedToolsConfig`, `VoiceSection`, `WebFetchPolicy`) show every one has at least one production caller. The `types/` sub-tree has no inert types or dead enums.

### Healthy `guides`
- **`GUIDE_FILES`** — read by `builtin_tools/config_guide.rs:214` and `src/config/guides.rs` itself.
- **`deploy_guides`** — `bin/aleph-server/commands/start/mod.rs:92`.

### Healthy `migration`
- **`migrate_mcp_builtin_in_toml` / `migrate_vector_db_in_toml`** — called by `Config::load_from_file_reporting_dead_keys` (`load.rs:215-216`).

### Healthy `schema`
- **`generate_config_schema_json`** — used by `gateway/handlers/config.rs` and `bin/aleph-server/...` (for the `config.schema` RPC).

## Negative findings (what I did NOT find)

- No `#[allow(dead_code)]` items masking severed functions in this module. (`#[allow(dead_code)]` exists on `is_fs_enabled` etc., but each is paired with a justification comment and a `pub(crate)` visibility — see sw-config-5.)
- No `todo!()` / `unimplemented!()` stubs in `src/config/`. (`unreachable!` exists at `schema.rs:27` and `patcher.rs:40` — both gated behind "this is an unreachable invariant of a transparent wrapper" doc comments.)
- No handler in `patcher.rs` / `live_apply.rs` that returns early or swallows errors without a reason. Every `return` / `continue` / `?` is preceded by a comment.
- No `unwrap()` /` `expect()` on operator-supplied paths. The only `unwrap()`/`expect()` calls in `src/config/` are in `#[cfg(test)]` blocks.
- No HMAC / signature verification in this module (the seam-catalog mentions constant-time comparison; irrelevant here).
- No `Path::join(user_input)` without a containment check. The `agent_resolver/initialize_*` paths are bounded by `agents_root` / `workspace_root`; `effective_path` is set once via argv.
- No `tokio::runtime::Runtime::new()` inside hot loops. (`live_apply::apply_live_sections` runs synchronously; `patcher::apply` runs inside the existing tokio runtime; `Config::load` does not spin a runtime.)
- No `pub const`/`pub static` whose value is hardcoded inline elsewhere (drift risk). The `LIVE_SECTIONS` / `LIVE_SUBSECTIONS` consts are the single source — every consumer reads from them.
- No `pub(crate)` helpers that exist only to bridge inert code (form 6 — `is_curated_owned`, `curated_owned_reason`, `is_fs_enabled` etc. all have honest doc comments about "no production caller" but none is severed).
- No error types with variants that are never constructed. (`AlephError::invalid_config` is the only error type produced; `Config::validate`, `Config::load`, etc. all construct it.)
- No classifier-vs-handler name-drift (form 5) inside `src/config/`. The `[security.ssrf] → Config.ssrf` bridge (`load.rs:280-345`) is the canonical mitigation; the dead-keys scanner tolerates the foreign-owned key.
- No `#[cfg(feature = "X")]`-gated code where `X` is not a declared feature (form 6). Only `#[cfg(test)]`, `#[cfg(unix)]` (for `fsync`), `#[cfg(feature = "...")]` if any are not present in this module.
- No `pub` item whose `Display` impl, `From` impl, or trait impl is itself a severed wire.

## Recommended actions (priority order)

1. **sw-config-1** — CUT `Config::defaults_override` field (and its two `load.rs` assignments). The process-global singleton is the actual reader; the field is three assignments of work for zero behavioral effect. Severity low, default to CUT.
2. **sw-config-3** — CUT `Config::get_default_provider()` from `methods.rs`. Zero production callers; the symmetric-setter-without-getter pattern is dead scaffolding. Severity low, default to CUT.
3. **sw-config-4** — KEEP `ui_hints::GroupMeta` / `FieldHint` as `pub`. They must be `pub` because `ConfigUiHints`'s public fields expose them; not a wire. Informational only.
4. **sw-config-5** — KEEP the `#[allow(dead_code)]` helpers in `types/` but review the doc-comments once per release; they are not severed.
5. **sw-config-6 through sw-config-13** — KEEP, no action. These are the verification findings (atomic write, migration, compile-time guard, test guard, latched-singleton fix) that demonstrate the module's existing safeguards.

## Sanity-check table (file:line)

| File | Line | Symbol / field |
|---|---|---|
| src/config/mod.rs | 14-31 | submodules (`pub mod agent_manager`, `pub mod agent_resolver`, `pub mod backup`, `pub mod defaults_override`, `pub mod guides`, `pub mod live_apply`, `pub(crate) mod load`, `pub mod patcher`, `pub mod presets_override`, `pub mod reload_impact`, `pub mod schema`, `pub mod types`, `pub mod ui_hints`) |
| src/config/mod.rs | 35 | `pub use structs::{ChannelInstanceConfig, Config, PluginMarketplaceEntry};` |
| src/config/mod.rs | 39-40 | `pub use live_apply::classify_verified;` / `pub use reload_impact::ReloadImpact;` |
| src/config/mod.rs | 43 | `pub use schema::generate_config_schema_json;` |
| src/config/mod.rs | 48 | `pub use ui_hints::ConfigUiHints;` |
| src/config/mod.rs | 51 | `pub use types::*;` |
| src/config/structs.rs | 30 | `pub struct PluginMarketplaceEntry` |
| src/config/structs.rs | 52 | `pub struct Config` |
| src/config/structs.rs | 86 | `pub mcp: McpConfig` — **sw-config-2** |
| src/config/structs.rs | 299 | `pub defaults_override: DefaultsOverride` — **sw-config-1** |
| src/config/structs.rs | 308 | `pub struct ChannelInstanceConfig` |
| src/config/structs.rs | 334 | `pub fn Config::resolved_channels` |
| src/config/structs.rs | 432 | `pub fn Config::new` |
| src/config/structs.rs | 438 | `pub fn Config::local_voice` |
| src/config/structs.rs | 441 | `pub fn Config::migrate_fetch` |
| src/config/methods.rs | 23 | `pub fn Config::get_effective_tools_config` |
| src/config/methods.rs | 46 | `pub fn Config::get_default_provider` — **sw-config-3** |
| src/config/methods.rs | 64 | `pub fn Config::set_default_provider` |
| src/config/methods.rs | 109 | `pub fn Config::add_rule_at_top` |
| src/config/methods.rs | 136 | `pub fn Config::remove_rule` |
| src/config/methods.rs | 183 | `pub fn Config::move_rule` |
| src/config/methods.rs | 233 | `pub fn Config::get_rule` |
| src/config/methods.rs | 242 | `pub const fn Config::rule_count` |
| src/config/schema.rs | 10 | `pub(crate) fn generate_config_schema` |
| src/config/schema.rs | 20 | `pub fn generate_config_schema_json` |
| src/config/load.rs | 33 | `pub(crate) const fn effective_config_path_slot` |
| src/config/load.rs | 46 | `pub fn Config::default_path` |
| src/config/load.rs | 64 | `pub fn Config::set_effective_path` |
| src/config/load.rs | 88 | `pub fn Config::decline_effective_path` |
| src/config/load.rs | 99 | `pub fn Config::effective_path` |
| src/config/load.rs | 118 | `pub fn Config::load_from_file` |
| src/config/load.rs | 166 | `pub fn Config::load_from_file_reporting_dead_keys` |
| src/config/load.rs | 266,346 | `config.defaults_override = ...clone();` — **sw-config-1** |
| src/config/load.rs | 289 | `pub fn Config::load` |
| src/config/save.rs | 14-44 | `#[cfg(test)] fn reject_real_home_config` |
| src/config/save.rs | 42-65 | `guard_against_section_loss` |
| src/config/save.rs | 83-135 | `fn write_atomically` — **sw-config-6** |
| src/config/save.rs | 240 | `pub fn Config::save_to_file` |
| src/config/save.rs | 317 | `pub fn Config::save` |
| src/config/save.rs | 357 | `pub fn Config::save_incremental` |
| src/config/save.rs | 381 | `pub fn Config::save_incremental_to_file` |
| src/config/save.rs | 478-573 | `guard_incremental_memory` / `guard_incremental_providers` |
| src/config/patcher.rs | 55 | `pub struct PatchRequest` |
| src/config/patcher.rs | 73 | `pub struct PatchResult` |
| src/config/patcher.rs | 95 | `pub struct FieldDiff` |
| src/config/patcher.rs | 109 | `pub enum HealthCheckResult` |
| src/config/patcher.rs | 117 | `pub struct RollbackResult` |
| src/config/patcher.rs | 139 | `pub struct ConfigPatcher` |
| src/config/patcher.rs | 156 | `pub fn ConfigPatcher::new` |
| src/config/patcher.rs | 171 | `pub fn ConfigPatcher::with_vault` |
| src/config/patcher.rs | 177 | `pub fn ConfigPatcher::config_path` |
| src/config/patcher.rs | 184 | `pub(crate) async fn ConfigPatcher::record_mtime` |
| src/config/patcher.rs | 223 | `pub async fn ConfigPatcher::apply` |
| src/config/patcher.rs | 340 | `apply_live_sections` (call inside `apply`) |
| src/config/patcher.rs | 393 | `final_config.validate()` (re-validate under write lock) |
| src/config/patcher.rs | 515 | `pub(crate) fn ConfigPatcher::validate_schema` |
| src/config/patcher.rs | 562 | `pub fn ConfigPatcher::list_backups` |
| src/config/patcher.rs | 581 | `pub async fn ConfigPatcher::rollback` |
| src/config/patcher.rs | 624 | `apply_live_sections` (call inside `rollback`) |
| src/config/patcher.rs | 688 | `pub(crate) fn get_nested_value` |
| src/config/patcher.rs | 704 | `pub(crate) fn set_nested_value` |
| src/config/patcher.rs | 768 | `pub(crate) fn deep_merge` |
| src/config/patcher.rs | 810 | `pub(crate) fn compute_diff` |
| src/config/validate.rs | 37 | `pub fn normalize_default_provider` |
| src/config/validate.rs | 69 | `pub fn Config::validate` |
| src/config/validate.rs | 654 | `pub fn Config::log_advisories` |
| src/config/migration.rs | 21 | `pub(crate) fn Config::migrate_mcp_builtin_in_toml` |
| src/config/migration.rs | 68 | `pub(crate) fn Config::migrate_vector_db_in_toml` |
| src/config/live_apply.rs | 51 | `pub fn apply_live_sections` |
| src/config/live_apply.rs | 163 | `_ => false` catch-all (intentional, see sw-config-10) |
| src/config/live_apply.rs | 195 | `pub fn classify_verified` |
| src/config/live_apply.rs | 610 | `every_live_section_has_an_apply_arm` test — **sw-config-8** |
| src/config/live_apply.rs | 683-790 | `every_dedicated_config_handler_that_saves_a_live_section_calls_apply_live_sections` |
| src/config/reload_impact.rs | 31 | `pub enum ReloadImpact` |
| src/config/reload_impact.rs | 67 | `pub(crate) const LIVE_SECTIONS` |
| src/config/reload_impact.rs | 100 | `pub(crate) const LIVE_SUBSECTIONS` |
| src/config/reload_impact.rs | 137 | `pub(crate) fn dotted_prefix_matches` |
| src/config/reload_impact.rs | 152 | `pub(crate) fn live_target_for` |
| src/config/reload_impact.rs | 168 | `pub(crate) fn live_targets` |
| src/config/backup.rs | 13 | `pub struct BackupEntry` |
| src/config/backup.rs | 22 | `pub struct ConfigBackup` |
| src/config/backup.rs | 33 | `pub const fn ConfigBackup::new` |
| src/config/backup.rs | 54 | `pub fn ConfigBackup::default_dir` |
| src/config/backup.rs | 71 | `pub fn ConfigBackup::create_snapshot` |
| src/config/backup.rs | 123 | `pub fn ConfigBackup::cleanup` |
| src/config/backup.rs | 170 | `pub fn ConfigBackup::resolve` |
| src/config/backup.rs | 198 | `pub fn ConfigBackup::list` |
| src/config/dead_keys.rs | 80-170 | `const TOLERATED` |
| src/config/dead_keys.rs | 176 | `pub(crate) fn deserialize_reporting_dead_keys` |
| src/config/dead_keys.rs | 392-410 | `every_foreign_owned_entry_still_has_its_reader` test — **sw-config-9** |
| src/config/defaults_override.rs | 27 | `pub struct ProviderDefaultsOverride` |
| src/config/defaults_override.rs | 35 | `pub struct GenerationDefaultsOverride` |
| src/config/defaults_override.rs | 46 | `pub struct DefaultsOverride` |
| src/config/defaults_override.rs | 75-110 | `EMPTY_DEFAULTS_OVERRIDE` static — **sw-config-13** |
| src/config/defaults_override.rs | 115 | `pub(crate) const fn defaults_override_slot` |
| src/config/defaults_override.rs | 125 | `pub fn init_defaults_override` |
| src/config/defaults_override.rs | 138 | `pub fn get_defaults_override` |
| src/config/defaults_override.rs | 150 | `pub fn load_defaults_override` |
| src/config/defaults_override.rs | 185 | `pub fn DefaultsOverride::provider_timeout_seconds` |
| src/config/defaults_override.rs | 190 | `pub fn DefaultsOverride::generation_timeout_seconds` |
| src/config/presets_override.rs | 21 | `pub struct PartialProviderPreset` |
| src/config/presets_override.rs | 50 | `pub struct PartialGenerationPreset` |
| src/config/presets_override.rs | 67 | `pub struct GenerationPresetsOverride` |
| src/config/presets_override.rs | 87 | `pub struct PresetsOverride` |
| src/config/presets_override.rs | 104 | `pub fn load_presets_override` |
| src/config/presets_override.rs | 142 | `pub struct OwnedProviderPreset` |
| src/config/presets_override.rs | 151 | `pub struct OwnedGenerationPreset` |
| src/config/presets_override.rs | 163 | `pub fn merge_provider_preset` |
| src/config/presets_override.rs | 191 | `pub fn partial_to_provider_preset` |
| src/config/presets_override.rs | 207 | `pub fn merge_generation_preset` |
| src/config/presets_override.rs | 226 | `pub fn partial_to_generation_preset` |
| src/config/guides.rs | 11 | `pub const GUIDE_FILES` |
| src/config/guides.rs | 38 | `pub fn deploy_guides` |
| src/config/ui_hints/mod.rs | 20 | `pub struct GroupMeta` — **sw-config-4** |
| src/config/ui_hints/mod.rs | 35 | `pub struct FieldHint` — **sw-config-4** |
| src/config/ui_hints/mod.rs | 64 | `pub struct ConfigUiHints` |
| src/config/ui_hints/mod.rs | 71 | `pub fn ConfigUiHints::new` |
| src/config/agent_manager/mod.rs | 31 | `pub(super) const BOOTSTRAP_FILES` |
| src/config/agent_manager/mod.rs | 50 | `pub(crate) const CURATED_OWNED_FILES` |
| src/config/agent_manager/mod.rs | 58 | `pub(crate) fn is_curated_owned` |
| src/config/agent_manager/mod.rs | 68 | `pub(crate) fn curated_owned_reason` |
| src/config/agent_manager/mod.rs | 76 | `pub(super) const MAX_ID_LENGTH` |
| src/config/agent_manager/mod.rs | 95 | `pub struct AgentPatch` |
| src/config/agent_manager/mod.rs | 117 | `pub struct WorkspaceFile` |
| src/config/agent_manager/mod.rs | 129 | `pub struct AgentManager` |
| src/config/agent_manager/mod.rs | 142 | `pub struct ProvisioningRoots` |
| src/config/agent_manager/mod.rs | 170 | `pub fn provisioning_roots` |
| src/config/agent_manager/crud.rs | 26 | `pub fn AgentManager::new` |
| src/config/agent_manager/crud.rs | 182 | `pub fn AgentManager::list` |
| src/config/agent_manager/crud.rs | 188 | `pub fn AgentManager::get` |
| src/config/agent_manager/crud.rs | 201 | `pub fn AgentManager::create` |
| src/config/agent_manager/crud.rs | 261 | `pub fn AgentManager::update` |
| src/config/agent_manager/crud.rs | 290 | `pub fn AgentManager::delete` |
| src/config/agent_manager/crud.rs | 337 | `pub fn AgentManager::set_default` |
| src/config/agent_manager/toml_ops.rs | 220 | `pub(super) fn model_ref_to_item` |
| src/config/agent_resolver/mod.rs | 26 | `pub(crate) mod templates` |
| src/config/agent_resolver/mod.rs | 47 | `pub(crate) fn resolve_model_ref` |
| src/config/agent_resolver/mod.rs | 81 | `pub struct ResolvedAgent` |
| src/config/agent_resolver/mod.rs | 147 | `pub struct AgentDefinitionResolver` |
| src/config/agent_resolver/mod.rs | 403 | `pub fn initialize_agent_dir` |
| src/config/agent_resolver/mod.rs | 417 | `pub fn initialize_agent_identity` |
| src/config/agent_resolver/mod.rs | 487 | `pub fn workspace_root_for` |
| src/config/agent_resolver/mod.rs | 498 | `pub fn agents_root_for` |
| src/config/agent_resolver/mod.rs | 509 | `pub(crate) fn default_workspace_root` |
| src/config/agent_resolver/mod.rs | 520 | `pub(crate) fn default_agents_root` |
| src/config/agent_resolver/templates.rs | 10 | `pub(crate) fn default_soul` |
| src/config/agent_resolver/templates.rs | 17 | `pub(crate) fn default_agents` |
| src/config/agent_resolver/templates.rs | 120 | `pub(crate) fn default_identity` |
| src/config/agent_resolver/templates.rs | 157 | `pub(crate) const DEFAULT_MEMORY` |
| src/config/agent_resolver/templates.rs | 160 | `pub(crate) const DEFAULT_TOOLS` |
| src/config/agent_resolver/templates.rs | 207 | `pub(crate) const DEFAULT_HEARTBEAT` |