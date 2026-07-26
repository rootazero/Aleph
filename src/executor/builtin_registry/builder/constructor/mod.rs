//! Builder / constructor for `BuiltinToolRegistry`
//!
//! Extracted from registry.rs to keep file sizes manageable.
//! Contains the `with_config()` constructor that wires up all tool instances
//! and registers their metadata. Cohesive groups of optional-tool construction
//! are split into sibling modules to keep file sizes manageable.

mod agent_acp_tools;
mod collab_session_tools;
mod coord_team_tools;

use crate::error::AlephError;
use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tracing::{info, warn};

use super::{BuiltinToolConfig, BuiltinToolRegistry};
use crate::builtin_tools::browser_tools::{
    BrowserClickTool, BrowserConsoleTool, BrowserCookiesTool, BrowserDialogTool, BrowserDragTool,
    BrowserEmulateTool, BrowserEvaluateTool, BrowserFillFormTool, BrowserHoverTool,
    BrowserNavigateTool, BrowserNetworkTool, BrowserOpenTool, BrowserPdfTool, BrowserPressKeyTool,
    BrowserProfileTool, BrowserResizeTool, BrowserScreenshotTool, BrowserScrollTool,
    BrowserSelectTool, BrowserSessionTool, BrowserSnapshotTool, BrowserTabsTool, BrowserTypeTool,
    BrowserUploadTool, BrowserWaitForTool,
};
use crate::builtin_tools::{
    ApplyPatchTool, AutomationTool, BashExecTool, CodeCheckTool, CodeExecTool, DesktopTool,
    FileEditTool, FileOpsTool, FileReadTool, FileWriteTool, ImageGenerateTool, MediaTool,
    MemoryBrowseTool, MemoryExploreTool, MemorySearchTool, PdfGenerateTool, PermissionTool,
    PimTool, ReadConfigGuideTool, ScratchpadTool, SearchTool, SelfManageTool, SystemTool,
    VaultStoreTool, WebFetchTool,
};
use crate::tool_metadata::{ToolSource, UnifiedTool};

impl BuiltinToolRegistry {
    /// Create a new registry with custom configuration
    ///
    /// Aleph is designed as a powerful AI Agent that needs to perform complex
    /// multi-step tasks including file operations and code execution.
    ///
    /// # Safety Notes
    /// - Dangerous commands are still blocked by `CommandChecker` (rm -rf /, sudo, etc.)
    /// - File operations are sandboxed by `PathPermissionChecker`
    /// - Tool policy is enforced layered (Guardrails + Sandbox + `ApprovalGate`).
    ///   See docs/reference/SANDBOX.md.
    pub async fn with_config(mut config: BuiltinToolConfig) -> crate::error::Result<Self> {
        let search_tool = if let Some(ref registry) = config.search_registry {
            SearchTool::with_registry(Arc::clone(registry))
        } else {
            SearchTool::with_api_key(config.tavily_api_key.clone())
        };
        let web_fetch_tool = {
            let mut tool = WebFetchTool::new();
            if let Some(ref cfg) = config.config {
                let cfg_guard = cfg.read().await;
                tool = tool.with_ssrf_policy(cfg_guard.ssrf.clone());
                if let Some(ref fetch_cfg) = cfg_guard.fetch {
                    if fetch_cfg.enabled {
                        let vault = config.shared_token_manager.clone();
                        let resolve = move |k: &str| -> Option<String> {
                            vault
                                .as_ref()
                                .and_then(|m| m.get_secret(k).ok().flatten())
                                .map(|s| s.expose().to_string())
                        };
                        let ctx = crate::fetch::factory::FetchBuildCtx {
                            search: cfg_guard.search.as_ref(),
                            resolve_secret: &resolve,
                        };
                        let registry = crate::fetch::FetchRegistry::from_config(fetch_cfg, &ctx);
                        tool = tool.with_fetch_providers(registry.select());
                    }
                }
            }
            tool
        };
        let file_ops_tool = if let Some(ref tc) = config.tool_context {
            FileOpsTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileOpsTool::new()
        };
        let file_read_tool = if let Some(ref tc) = config.tool_context {
            FileReadTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileReadTool::new()
        };
        let file_write_tool = if let Some(ref tc) = config.tool_context {
            FileWriteTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileWriteTool::new()
        };
        let file_edit_tool = if let Some(ref tc) = config.tool_context {
            FileEditTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileEditTool::new()
        };
        let apply_patch_tool = if let Some(ref tc) = config.tool_context {
            ApplyPatchTool::new().with_tool_context(Arc::clone(tc))
        } else {
            ApplyPatchTool::new()
        };
        let bash_tool = if let Some(ref sb) = config.sandbox {
            BashExecTool::new().with_sandbox(sb.clone())
        } else {
            BashExecTool::new()
        };
        let code_exec_tool = if let Some(ref sb) = config.sandbox {
            CodeExecTool::new().with_sandbox(sb.clone())
        } else {
            CodeExecTool::new()
        };
        let code_check_tool = if let Some(ref sb) = config.sandbox {
            CodeCheckTool::new().with_sandbox(sb.clone())
        } else {
            CodeCheckTool::new()
        };
        let pdf_generate_tool = if let Some(ref tc) = config.tool_context {
            PdfGenerateTool::new().with_tool_context(Arc::clone(tc))
        } else {
            PdfGenerateTool::new()
        };

        // Skill list/read tools are constructed per dispatch in
        // `registry.rs` from the active project root (round 3) — no shared
        // field needed; see the `skill_list` / `skill_read` match arms.

        // Config guide tool (Progressive Disclosure for self-management)
        let config_guide_tool = ReadConfigGuideTool::default();

        // Ctx-search tool (BM25 retrieval over offloaded tool output)
        let ctx_search_tool = crate::builtin_tools::CtxSearchTool::new();

        // Recall-events tool (BM25 retrieval over this session's event log)
        let recall_events_tool = crate::builtin_tools::RecallEventsTool::new();

        // Self-management tool (LLM-triggered entry point)
        let self_manage_tool = SelfManageTool::default();

        // Self-config tool (identity files + config.toml access)
        let self_config_tool = {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let mut tool = crate::builtin_tools::self_config::SelfConfigTool::new(agent_id)?;
            if let Some(ref cfg) = config.config {
                tool = tool.with_config(Arc::clone(cfg));
            }
            if let Some(ref patcher) = config.config_patcher {
                tool = tool.with_patcher(Arc::clone(patcher));
            }
            tool
        };

        // Moa-manage tool (LLM-facing MoA session activation + preset CRUD).
        // Reuses the already-injected config + patcher handles — same
        // construction pattern as self_config.
        let moa_manage_tool = {
            let mut tool = crate::builtin_tools::moa_manage::MoaManageTool::new();
            if let Some(ref cfg) = config.config {
                tool = tool.with_config(Arc::clone(cfg));
            }
            if let Some(ref patcher) = config.config_patcher {
                tool = tool.with_patcher(Arc::clone(patcher));
            }
            tool
        };

        // List-models tool (LLM-facing model discovery: capability + cost).
        // Reuses the already-injected config + vault handles — no new wiring.
        let list_models_tool = {
            let mut tool = crate::builtin_tools::list_models::ListModelsTool::new();
            if let Some(ref cfg) = config.config {
                tool = tool.with_config(Arc::clone(cfg));
            }
            if let Some(ref mgr) = config.shared_token_manager {
                tool = tool.with_vault(Arc::clone(mgr));
            }
            tool
        };

        // Doctor tool (self-diagnosis). Reuses the already-injected config +
        // vault handles — when both are present the diagnostics engine gains
        // the providers/connectivity runtime check, so the LLM repair loop
        // can probe provider reachability and verify its own fixes.
        let doctor_tool = {
            let mut tool = crate::builtin_tools::DoctorTool::default();
            if let (Some(cfg), Some(mgr)) = (&config.config, &config.shared_token_manager) {
                tool = tool.with_runtime(Arc::clone(cfg), Arc::clone(mgr));
            }
            tool
        };

        // Vault store tool (requires SharedTokenManager)
        let vault_store_tool = config.shared_token_manager.as_ref().map(|mgr| {
            info!("Creating VaultStoreTool");
            VaultStoreTool::new(Arc::clone(mgr))
        });

        // Store catalog-sync tool (requires CatalogCache)
        let hub_catalog_sync_tool = if let Some(ref cache) = config.catalog_cache {
            info!("Creating HubCatalogSyncTool");
            Some(crate::builtin_tools::hub::HubCatalogSyncTool {
                cache: Arc::clone(cache),
            })
        } else {
            None
        };

        // Store resolve-spec tool (requires CatalogCache)
        let hub_resolve_spec_tool = if let Some(ref cache) = config.catalog_cache {
            info!("Creating HubResolveSpecTool");
            Some(crate::builtin_tools::hub::HubResolveSpecTool {
                cache: Arc::clone(cache),
            })
        } else {
            None
        };

        // Store install-run tool (T7, trust-gated). Requires CatalogCache +
        // marketplace configs + vault; the live MCP handle is optional (None →
        // MCP-spec installs report "MCP manager unavailable", plugin installs
        // and secret storage still work).
        let hub_install_run_tool = match (&config.catalog_cache, &config.shared_token_manager) {
            (Some(cache), Some(vault)) => {
                let marketplaces = config.hub_marketplace_configs.clone().unwrap_or_default();
                info!("Creating HubInstallRunTool");
                Some(crate::builtin_tools::hub::HubInstallRunTool {
                    cache: Arc::clone(cache),
                    marketplaces,
                    vault: Arc::clone(vault),
                    mcp: config.hub_mcp_handle.clone(),
                })
            }
            _ => None,
        };

        // Store install-verify tool (T8). Dep: optional live MCP handle only.
        // Always constructed (even without CatalogCache) — plugin verification
        // needs no cache; the mcp field is None when the handle isn't available.
        let hub_install_verify_tool = Some(crate::builtin_tools::hub::HubInstallVerifyTool {
            mcp: config.hub_mcp_handle.clone(),
        });

        // Store fetch-docs tool (scaffold — no CatalogCache dep; always constructed)
        let hub_fetch_docs_tool = crate::builtin_tools::hub::HubFetchDocsTool;

        // Build platform-specific DesktopPlatform.
        //
        // NOT an R1 violation, despite naming a platform crate here: this is the
        // dependency-injection composition root (P4 — "constructor injection"). `src`
        // depends only on the `aleph_desktop::DesktopPlatform` capability trait;
        // the actual platform-API calls (AppKit / windows-rs / …) live entirely
        // inside the `aleph_desktop_{macos,linux,windows}` crates behind that
        // trait object. Selecting the concrete impl at the startup composition
        // root is exactly where a DI seam is supposed to bind a trait to its
        // implementation — moving this construction behind an IPC boundary would
        // add a transport layer for in-process capabilities with no R1 benefit.
        // (Evaluated 2026-07-20; keep as-is.)
        let desktop_platform: Arc<dyn aleph_desktop::DesktopPlatform> = {
            #[cfg(target_os = "macos")]
            {
                Arc::new(aleph_desktop_macos::MacOSPlatform::new())
            }

            #[cfg(target_os = "linux")]
            {
                Arc::new(aleph_desktop_linux::LinuxPlatform::new())
            }

            #[cfg(target_os = "windows")]
            {
                Arc::new(aleph_desktop_windows::WindowsPlatform::new())
            }
        };

        // Desktop tool — platform-native desktop/screen capability, plus a
        // vision bridge so `screenshot {describe:true}` yields an OCR text
        // layer (offline, platform-native) for text-only models. The bridge's
        // pipeline starts with the platform OCR provider; registering a
        // multimodal provider later lights up the scene-`description` layer
        // with no change here.
        let vision_pipeline = {
            let mut pipeline = crate::vision::VisionPipeline::new();
            // Resolve OCR through the injected platform's screen capability so
            // `screenshot {describe:true}` works on macOS (its OCR routes through
            // the Swift bridge); a bare `PlatformOcrProvider::new()` uses
            // NativeScreen, whose OCR is NotImplemented on macOS.
            pipeline.add_provider(Box::new(
                crate::vision::providers::PlatformOcrProvider::with_platform(Arc::clone(
                    &desktop_platform,
                )),
            ));
            Arc::new(pipeline)
        };
        let vision_bridge = Arc::new(crate::builtin_tools::desktop::VisionBridge::new(
            Arc::clone(&vision_pipeline),
        ));

        // Media pipeline — powers media_understand / document_extract. These
        // were advertised-but-disabled: their schema is gated on
        // `config.media_pipeline` (constructor below) and their dispatch errors
        // when it is None, yet nothing ever constructed one outside tests. Wire
        // the LLM-free providers so the tools actually run: document text
        // extraction stands alone; image understanding shares the vision
        // pipeline (OCR text today, scene description once a multimodal vision
        // provider is registered — no change here). AudioStubProvider is
        // deliberately NOT registered: it only returns `NoProvider`, so adding
        // it would make the pipeline *claim* audio support while still failing —
        // audio_transcribe stays honestly unsupported until a real transcription
        // provider (e.g. a Whisper MCP) is wired. Only construct when the caller
        // did not supply a pipeline.
        if config.media_pipeline.is_none() {
            let mut mp = crate::media::MediaPipeline::new();
            mp.add_provider(Box::new(crate::media::ImageMediaProvider::new(
                Arc::clone(&vision_pipeline),
                10,
            )));
            mp.add_provider(Box::new(crate::media::TextDocumentProvider));
            config.media_pipeline = Some(Arc::new(mp));
        }
        // Approval policy — gates sensitive desktop/PIM actions. Loaded from
        // `~/.aleph/approval-policy.json`; with no file present it falls back to
        // a permissive default (desktop actions Allow, shell Deny), so wiring
        // here is byte-identical to the previous unwired (allow-all) behavior
        // until the user supplies a policy file. Shared by DesktopTool + PimTool
        // and the sensitive browser tools (navigate/click/type/fill_form/
        // evaluate), whose `Browser*` action types the policy engine already
        // models — previously advertised but never enforced.
        let approval_policy: Arc<dyn crate::approval::ApprovalPolicy> =
            Arc::new(crate::approval::ConfigApprovalPolicy::load());

        // `[desktop] allow_global_pointer` — the one input-rail policy knob. Left
        // at its default (false), a coordinate action that names no target
        // process is refused instead of running on the global HID tap, which
        // would drag the user's physical cursor across the screen. Only consulted
        // on a platform that can deliver input into a single process; see
        // `builtin_tools::desktop::native`.
        let allow_global_pointer = match config.config {
            Some(ref cfg) => cfg.read().await.desktop.allow_global_pointer,
            None => false,
        };

        let clipboard_enabled = match config.config {
            Some(ref cfg) => cfg.read().await.get_effective_tools_config().is_clipboard_enabled(),
            None => true,
        };

        let desktop_tool = DesktopTool::new()
            .with_platform(Arc::clone(&desktop_platform))
            .with_vision_bridge(Arc::clone(&vision_bridge))
            .with_approval_policy(Arc::clone(&approval_policy))
            .with_allow_global_pointer(allow_global_pointer)
            .with_clipboard_enabled(clipboard_enabled);

        // AX query tools (macOS + Windows UIA; degrade gracefully on Linux).
        let desktop_ax_query_focused_tool = crate::builtin_tools::DesktopAxQueryFocused::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_tree_tool = crate::builtin_tools::DesktopAxQueryTree::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_by_role_tool = crate::builtin_tools::DesktopAxQueryByRole::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_snapshot_tool = crate::builtin_tools::DesktopAxSnapshot::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_som_tool =
            crate::builtin_tools::DesktopSom::new().with_platform(Arc::clone(&desktop_platform));
        let desktop_gui_locate_tool = crate::builtin_tools::DesktopGuiLocate::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_check_permissions_tool = crate::builtin_tools::DesktopCheckPermissions::new()
            .with_platform(Arc::clone(&desktop_platform));

        // Gate state-changing system ops (launch/quit/restart/clipboard_write)
        // behind the same approval policy DesktopTool uses, so an agent cannot
        // bypass that gate by routing the same OS op through the `system` tool.
        let system_tool = SystemTool::new(Arc::clone(&desktop_platform))
            .with_approval_policy(Arc::clone(&approval_policy))
            .with_clipboard_enabled(clipboard_enabled);
        // Automation runs arbitrary host code (AppleScript/JXA/shell/PowerShell)
        // + Shortcuts — gate it behind the same approval policy as DesktopTool/
        // PimTool via the `DesktopAutomation` action type (permissive default =
        // Allow, so byte-identical until a policy file tightens it).
        let automation_tool = AutomationTool::new(Arc::clone(&desktop_platform))
            .with_approval_policy(Arc::clone(&approval_policy));
        let permission_tool = PermissionTool::new(Arc::clone(&desktop_platform));
        // Media tool — camera/mic capture rides the same approval policy via the
        // `MediaCapture` action type (permissive default = Allow, so byte-
        // identical until a policy file sets `media_capture: ask/deny`).
        let media_tool = MediaTool::new(Arc::clone(&desktop_platform))
            .with_approval_policy(Arc::clone(&approval_policy));

        // PIM tool — platform-native notes/calendar/reminders/contacts capability.
        let pim_tool = PimTool::new()
            .with_platform(Arc::clone(&desktop_platform))
            .with_approval_policy(Arc::clone(&approval_policy));

        let scratchpad_tool = ScratchpadTool::new();

        // Standing-goal store: a session-keyed SQLite DB under the data dir.
        // Initialize the process-global so the harness bridge + tool share it.
        let goal_store = Arc::new(
            crate::goal::GoalStore::open(
                &crate::utils::paths::get_data_dir()
                    .map_err(|e| AlephError::other(format!("goal store data dir: {e}")))?
                    .join("goals.db"),
            )
            .map_err(|e| AlephError::other(format!("goal store open: {e}")))?,
        );
        crate::goal::init_global(goal_store.clone());
        let goal_tool = crate::builtin_tools::GoalTool::new(goal_store);

        // Loop-graph governance store: explicit topology over the
        // self-improvement loops. Its OWN small DB — deliberately outside
        // every optimizer's writable domain (dreaming must never be able to
        // rewrite who watches it). Globalized for the doctor check + future
        // post-run trigger consumers.
        let loop_graph_store = Arc::new(
            crate::loop_graph::LoopGraphStore::open(
                &crate::utils::paths::get_data_dir()
                    .map_err(|e| AlephError::other(format!("loop_graph store data dir: {e}")))?
                    .join("loop_graph.db"),
            )
            .map_err(|e| AlephError::other(format!("loop_graph store open: {e}")))?,
        );
        crate::loop_graph::init_global(loop_graph_store.clone());
        crate::loop_graph::service::init_cron_trigger(config.cron_service.clone());
        let loop_graph_tool = crate::builtin_tools::LoopGraphTool::new(loop_graph_store)
            .with_cron_service(config.cron_service.clone())
            .with_team_store(config.team_store.clone());

        // Loop subsystem — in-memory only (never tasks.db); cleared on restart.
        let loop_registry = Arc::new(crate::looping::LoopRegistry::default());
        crate::looping::init_global(loop_registry.clone());
        let loop_tool = crate::builtin_tools::LoopTool::new(loop_registry);

        // Strategy store: session-keyed SQLite DB under the data dir (mirrors
        // the goal store). Globalized so the harness bridge and lifecycle clears
        // share one store.
        let strategy_store = Arc::new(
            crate::strategy::StrategyStore::open(
                &crate::utils::paths::get_data_dir()
                    .map_err(|e| AlephError::other(format!("strategy store data dir: {e}")))?
                    .join("strategy.db"),
            )
            .map_err(|e| AlephError::other(format!("strategy store open: {e}")))?,
        );
        crate::strategy::init_global(strategy_store.clone());
        let strategy_tool = crate::builtin_tools::StrategyTool::new(strategy_store);

        // Browser tools — always available, use ProfileManager from config or create default
        let browser_profile_manager = config.browser_profile_manager.clone().unwrap_or_else(|| {
            Arc::new(crate::browser::manager::ProfileManager::new(
                crate::browser::profile::BrowserSystemConfig::default(),
            ))
        });
        let browser_open_tool = BrowserOpenTool::new(Arc::clone(&browser_profile_manager));
        let browser_click_tool = BrowserClickTool::new(Arc::clone(&browser_profile_manager))
            .with_approval_policy(Arc::clone(&approval_policy));
        let browser_type_tool = BrowserTypeTool::new(Arc::clone(&browser_profile_manager))
            .with_approval_policy(Arc::clone(&approval_policy));
        // Share the same TTL-cached vision bridge as the desktop screenshot tool
        // so `browser_screenshot {describe:true}` yields an OCR/scene-description
        // layer for text-only models (connect-first — no second pipeline).
        let browser_screenshot_tool =
            BrowserScreenshotTool::new(Arc::clone(&browser_profile_manager))
                .with_vision_bridge(Arc::clone(&vision_bridge));
        let browser_snapshot_tool = BrowserSnapshotTool::new(Arc::clone(&browser_profile_manager));
        let browser_navigate_tool = BrowserNavigateTool::new(Arc::clone(&browser_profile_manager))
            .with_approval_policy(Arc::clone(&approval_policy));
        let browser_tabs_tool = BrowserTabsTool::new(Arc::clone(&browser_profile_manager));
        let browser_select_tool = BrowserSelectTool::new(Arc::clone(&browser_profile_manager));
        let browser_evaluate_tool = BrowserEvaluateTool::new(Arc::clone(&browser_profile_manager))
            .with_approval_policy(Arc::clone(&approval_policy));
        let browser_fill_form_tool = BrowserFillFormTool::new(Arc::clone(&browser_profile_manager))
            .with_approval_policy(Arc::clone(&approval_policy));
        let browser_press_key_tool = BrowserPressKeyTool::new(Arc::clone(&browser_profile_manager));
        let browser_wait_for_tool = BrowserWaitForTool::new(Arc::clone(&browser_profile_manager));
        let browser_console_tool = BrowserConsoleTool::new(Arc::clone(&browser_profile_manager));
        let browser_hover_tool = BrowserHoverTool::new(Arc::clone(&browser_profile_manager));
        let browser_scroll_tool = BrowserScrollTool::new(Arc::clone(&browser_profile_manager));
        let browser_pdf_tool = BrowserPdfTool::new(Arc::clone(&browser_profile_manager));
        let browser_network_tool = BrowserNetworkTool::new(Arc::clone(&browser_profile_manager));
        let browser_dialog_tool = BrowserDialogTool::new(Arc::clone(&browser_profile_manager));
        let browser_drag_tool = BrowserDragTool::new(Arc::clone(&browser_profile_manager));
        let browser_upload_tool = BrowserUploadTool::new(Arc::clone(&browser_profile_manager));
        let browser_resize_tool = BrowserResizeTool::new(Arc::clone(&browser_profile_manager));
        let browser_emulate_tool = BrowserEmulateTool::new(Arc::clone(&browser_profile_manager));
        let browser_cookies_tool = BrowserCookiesTool::new(Arc::clone(&browser_profile_manager));
        let browser_session_tool = BrowserSessionTool::new(Arc::clone(&browser_profile_manager));
        // Start the idle-profile reaper (sweeps stale browsers every 60s).
        browser_profile_manager.spawn_idle_reaper(60);
        let browser_profile_tool = BrowserProfileTool::new(browser_profile_manager);

        // Create memory tools if backend and embedder are provided
        let (
            memory_search_tool,
            memory_browse_tool,
            memory_explore_tool,
            memory_workspace_handle,
            memory_session_key_handle,
        ) = if let (Some(ref db), Some(ref embedder)) = (&config.memory_db, &config.embedder) {
            // Cross-encoder rerank + retrieval-scoring config (both disabled by
            // default → no behaviour change). Read once to avoid re-locking.
            let (rerank_cfg, scoring_cfg, expansion_cfg): (
                Option<crate::memory::rerank::RerankConfig>,
                Option<crate::config::types::memory::RetrievalScoringConfig>,
                Option<crate::config::types::memory::ExpansionConfig>,
            ) = match &config.config {
                Some(cfg) => {
                    let guard = cfg.read().await;
                    (
                        Some(guard.memory.rerank.clone()),
                        Some(guard.memory.retrieval_scoring.clone()),
                        Some(guard.memory.expansion.clone()),
                    )
                }
                None => (None, None, None),
            };
            let search_tool = MemorySearchTool::new_with_config(
                db.clone(),
                Arc::clone(embedder),
                config.memory_similarity_threshold,
                rerank_cfg.as_ref(),
                scoring_cfg.as_ref(),
                expansion_cfg.as_ref(),
            )
            .with_project_scoping(config.memory_project_scoped);
            let ws_handle = search_tool.default_workspace_handle();
            let sk_handle = search_tool.default_session_key_handle();
            let note_memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir().map_or_else(
                    || {
                        std::env::temp_dir()
                            .join("aleph")
                            .join("memory")
                            .join("note")
                    },
                    |p| p.join(".aleph").join("memory").join("note"),
                )
            });
            let browse_tool = MemoryBrowseTool::new(note_memory_dir, "default".to_string());
            let explore_tool = MemoryExploreTool::new(db.clone(), Arc::clone(embedder));
            info!("Created memory_search, memory_browse, and memory_explore tools");
            (
                Some(search_tool),
                Some(browse_tool),
                Some(explore_tool),
                Some(ws_handle),
                Some(sk_handle),
            )
        } else if config.memory_db.is_some() {
            let note_memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir().map_or_else(
                    || {
                        std::env::temp_dir()
                            .join("aleph")
                            .join("memory")
                            .join("note")
                    },
                    |p| p.join(".aleph").join("memory").join("note"),
                )
            });
            let browse_tool = MemoryBrowseTool::new(note_memory_dir, "default".to_string());
            info!("Created memory_browse tool (no embedder for memory_search)");
            (None, Some(browse_tool), None, None, None)
        } else {
            (None, None, None, None, None)
        };

        // Create memory timeline tool if StateDatabase is provided
        let timeline_tool = config.state_db.as_ref().map(|sdb| {
            let traveler = Arc::new(crate::memory::events::traveler::MemoryTimeTraveler::new(
                Arc::clone(sdb),
            ));
            crate::builtin_tools::MemoryTimelineTool::new(traveler)
        });

        // Create image generation tool if generation registry is provided
        let image_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating ImageGenerateTool with generation registry");
            ImageGenerateTool::new(Arc::clone(registry))
        });

        let video_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating VideoGenerateTool with generation registry");
            crate::builtin_tools::generation::VideoGenerateTool::new(Arc::clone(registry))
        });

        let audio_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating AudioGenerateTool with generation registry");
            crate::builtin_tools::generation::AudioGenerateTool::new(Arc::clone(registry))
        });

        let speech_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating SpeechGenerateTool with generation registry");
            crate::builtin_tools::generation::SpeechGenerateTool::new(Arc::clone(registry))
        });

        // Build wiki tools (Spec 5 Task 12)
        let note_orient_tool = config.orientation.as_ref().map(|wiki| {
            use crate::memory::notes::orientation::types::TokenBudget;
            crate::builtin_tools::note_orient::NoteOrientTool::new(
                Arc::clone(wiki),
                TokenBudget::default(),
            )
        });

        let note_schema_tool = config
            .note_memory_dir
            .as_ref()
            .map(|dir| crate::builtin_tools::note_schema::NoteSchemaTool::new(dir.clone()));

        // Build user profile tool (Spec 7 Task 9)
        let user_profile_tool = config.profile_synthesizer.as_ref().map(|synth| {
            crate::builtin_tools::user_profile::UserProfileTool::new(Arc::clone(synth))
        });

        // Build tool metadata
        let mut tools = HashMap::new();

        // Register always-available tool metadata
        Self::register_core_tools(&mut tools);

        // Register browser tools metadata (with parameter schemas from AlephTool::definition)
        {
            use crate::tools::AlephTool;
            let browser_tool_defs = [
                browser_open_tool.definition(),
                browser_click_tool.definition(),
                browser_type_tool.definition(),
                browser_screenshot_tool.definition(),
                browser_snapshot_tool.definition(),
                browser_navigate_tool.definition(),
                browser_tabs_tool.definition(),
                browser_select_tool.definition(),
                browser_evaluate_tool.definition(),
                browser_fill_form_tool.definition(),
                browser_press_key_tool.definition(),
                browser_wait_for_tool.definition(),
                browser_console_tool.definition(),
                browser_hover_tool.definition(),
                browser_scroll_tool.definition(),
                browser_pdf_tool.definition(),
                browser_network_tool.definition(),
                browser_dialog_tool.definition(),
                browser_drag_tool.definition(),
                browser_upload_tool.definition(),
                browser_resize_tool.definition(),
                browser_emulate_tool.definition(),
                browser_cookies_tool.definition(),
                browser_session_tool.definition(),
                browser_profile_tool.definition(),
            ];
            for td in &browser_tool_defs {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
        }
        info!("Registered browser tools (25 tools) in BuiltinToolRegistry");

        // Register parameter schemas for always-available tools that are listed
        // in BUILTIN_TOOL_DEFINITIONS (so the LLM sees them) and dispatched in
        // registry.rs, but whose metadata was never inserted into the runtime
        // `tools` map. `get_tool_schema()` returned None for them, so the agent
        // loop advertised them with an empty parameter schema — leaving the
        // model to guess argument shapes (notably apply_patch's V4A patch format
        // and the desktop AX/SoM/locate targeting args). Source each schema from
        // the tool's own AlephTool::definition(), exactly like the browser block.
        {
            use crate::tools::AlephTool;
            let gateway_route_meta =
                crate::builtin_tools::gateway_route::GatewayRouteTool::default();
            let google_meet_meta = crate::builtin_tools::google_meet::GoogleMeetTool::new(None);
            let select_model_meta = crate::builtin_tools::SelectModelTool;
            let doctor_meta = crate::builtin_tools::DoctorTool::default();
            let extra_defs = [
                apply_patch_tool.definition(),
                desktop_ax_query_focused_tool.definition(),
                desktop_ax_query_tree_tool.definition(),
                desktop_ax_query_by_role_tool.definition(),
                desktop_ax_snapshot_tool.definition(),
                desktop_som_tool.definition(),
                desktop_gui_locate_tool.definition(),
                desktop_check_permissions_tool.definition(),
                gateway_route_meta.definition(),
                google_meet_meta.definition(),
                select_model_meta.definition(),
                doctor_meta.definition(),
            ];
            for td in &extra_defs {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!(
                "Registered schemas for apply_patch, desktop AX/SoM/locate, gateway_route, \
                 google_meet, select_model, doctor in BuiltinToolRegistry"
            );
        }

        // Media-understanding tools share the same gap: listed + dispatched but
        // their schema was never registered. They require a MediaPipeline, so
        // register only when one is configured (matching the dispatch guard).
        if let Some(ref mp) = config.media_pipeline {
            use crate::tools::AlephTool;
            // All three keep a registered schema so their advertisement (via
            // BUILTIN_TOOL_DEFINITIONS, gated on media_pipeline) is never a
            // schema-less entry. media_understand (image→OCR) and
            // document_extract run for real; audio_transcribe returns a clear
            // `NoProvider` until a transcription provider is wired.
            let media_defs = [
                crate::builtin_tools::media_tools::MediaUnderstandTool::new(Arc::clone(mp))
                    .definition(),
                crate::builtin_tools::media_tools::AudioTranscribeTool::new(Arc::clone(mp))
                    .definition(),
                crate::builtin_tools::media_tools::DocumentExtractTool::new(Arc::clone(mp))
                    .definition(),
            ];
            for td in &media_defs {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered schemas for media_understand, audio_transcribe, document_extract");
        }

        // config_audit shares the same gap: listed in BUILTIN_TOOL_DEFINITIONS
        // and dispatched in registry.rs, but its metadata was never inserted
        // into the runtime map, so get_tool_schema() returned None and the
        // model was advertised the tool with an empty parameter schema. It
        // needs the live Config handle, so register only when one is
        // configured (matching the dispatch guard).
        if let Some(ref cfg) = config.config {
            use crate::tools::AlephTool;
            let td = crate::builtin_tools::ConfigAuditTool::new(Arc::clone(cfg)).definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            info!("Registered schema for config_audit");
        }

        info!(
            "Registered skill.list, skill.read, and read_config_guide tools in BuiltinToolRegistry"
        );

        // Store catalog-sync + resolve-spec tools: register schemas only when
        // cache is configured (matches the dispatch guard in tool_registry_impl.rs).
        if let Some(ref cache) = config.catalog_cache {
            use crate::tools::AlephTool;

            let td = crate::builtin_tools::hub::HubCatalogSyncTool {
                cache: cache.clone(),
            }
            .definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            info!("Registered schema for hub_catalog_sync");

            let td = crate::builtin_tools::hub::HubResolveSpecTool {
                cache: cache.clone(),
            }
            .definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            info!("Registered schema for hub_resolve_spec");

            // hub_install_run: register the schema only when a vault is also
            // present (matches the dispatch-guard construction above).
            if let Some(ref vault) = config.shared_token_manager {
                let td = crate::builtin_tools::hub::HubInstallRunTool {
                    cache: cache.clone(),
                    marketplaces: config.hub_marketplace_configs.clone().unwrap_or_default(),
                    vault: vault.clone(),
                    mcp: config.hub_mcp_handle.clone(),
                }
                .definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
                info!("Registered schema for hub_install_run");
            }
        }

        // hub_fetch_docs: scaffold tool, no CatalogCache dep — register unconditionally.
        {
            use crate::tools::AlephTool;
            let td = crate::builtin_tools::hub::HubFetchDocsTool.definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            info!("Registered schema for hub_fetch_docs");
        }

        // hub_install_verify: no CatalogCache dep — register unconditionally.
        {
            use crate::tools::AlephTool;
            let td = crate::builtin_tools::hub::HubInstallVerifyTool {
                mcp: config.hub_mcp_handle.clone(),
            }
            .definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            info!("Registered schema for hub_install_verify");
        }

        // Register optional tool metadata
        Self::register_optional_tools(
            &mut tools,
            &memory_search_tool,
            &memory_browse_tool,
            &memory_explore_tool,
            &timeline_tool,
            &image_generate_tool,
            &vault_store_tool,
            &config,
            config.injection_mode,
            &note_orient_tool,
            &note_schema_tool,
            &user_profile_tool,
        );

        // Agent-management, ACP, and A2A tools (extracted to agent_acp_tools.rs).
        let (
            agent_info_tool,
            agent_create_tool,
            agent_list_tool,
            agent_delete_tool,
            agent_switch_tool,
            session_context_handle,
            acp_delegate_tool,
            acp_switch_tool,
            acp_session_control_tool,
            a2a_delegate_tool,
            a2a_agents_tool,
        ) = Self::build_agent_acp_a2a_tools(&config, &mut tools);

        // Pre-compute current agent ID — used by team, messaging, and session tools
        let current_agent_id = config
            .current_agent_id
            .clone()
            .unwrap_or_else(|| "main".to_string());

        // Task-coordination and team-management tools (extracted to
        // coord_team_tools.rs).
        let (
            task_create_tool,
            task_update_tool,
            task_list_tool,
            task_wait_tool,
            task_comment_tool,
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_set_protocol_tool,
            team_member_add_tool,
            team_member_remove_tool,
            team_from_template_tool,
            team_snapshot_tool,
            team_usage_tool,
            team_acp_member_tool,
            team_workflow_canvas_tool,
            workflow_step_review_tool,
            workflow_tool,
            team_task_control_tool,
            task_exit_journal_tool,
            team_digest_tool,
        ) = Self::build_coord_team_tools(&config, &mut tools, &current_agent_id);

        // Messaging, plan-approval, lifecycle, artifact, collaborative-session,
        // skill, and note tools (extracted to collab_session_tools.rs).
        let (
            message_send_tool,
            inbox_read_tool,
            plan_submit_tool,
            plan_resolve_tool,
            lifecycle_idle_tool,
            lifecycle_request_shutdown_tool,
            lifecycle_resolve_shutdown_tool,
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
            session_collaborate_tool,
            session_turn_tool,
            session_read_tool,
            google_meet_tool,
            skill_status_tool,
            skill_install_tool,
            skill_manage_tool,
            note_manage_tool,
            session_complete_tool,
            memory_reflect_tool,
        ) = Self::build_collab_session_tools(&config, &mut tools, &current_agent_id);

        Ok(Self {
            search_tool,
            web_fetch_tool,
            file_ops_tool,
            file_read_tool,
            file_write_tool,
            file_edit_tool,
            apply_patch_tool,
            bash_tool,
            code_exec_tool,
            code_check_tool,
            pdf_generate_tool,
            image_generate_tool,
            video_generate_tool,
            audio_generate_tool,
            speech_generate_tool,
            config_guide_tool,
            ctx_search_tool,
            recall_events_tool,
            self_manage_tool,
            self_config_tool,
            moa_manage_tool,
            list_models_tool,
            doctor_tool,
            vault_store_tool,
            hub_catalog_sync_tool,
            hub_resolve_spec_tool,
            hub_install_run_tool,
            hub_install_verify_tool,
            hub_fetch_docs_tool,
            desktop_tool,
            desktop_ax_query_focused_tool,
            desktop_ax_query_tree_tool,
            desktop_ax_query_by_role_tool,
            desktop_ax_snapshot_tool,
            desktop_som_tool,
            desktop_gui_locate_tool,
            desktop_check_permissions_tool,
            pim_tool,
            system_tool,
            automation_tool,
            permission_tool,
            media_tool,
            // Share the live session-key handle so the scratchpad tool can
            // bind its project to the session for the goal-loop hook. When
            // memory is unconfigured the handle is None → hook stays dormant.
            scratchpad_tool: scratchpad_tool
                .with_session_key_handle(memory_session_key_handle.clone()),
            goal_tool: goal_tool
                .with_session_key_handle(memory_session_key_handle.clone())
                .with_planner_provider(config.planner_provider.clone()),
            loop_tool: loop_tool
                .with_session_key_handle(memory_session_key_handle.clone())
                .with_planner_provider(config.planner_provider.clone()),
            loop_graph_tool,
            strategy_tool: strategy_tool.with_session_key_handle(memory_session_key_handle.clone()),
            memory_search_tool,
            memory_context_provider: Arc::new(tokio::sync::OnceCell::new()),
            node_registry: Arc::new(tokio::sync::OnceCell::new()),
            node_security_store: Arc::new(tokio::sync::OnceCell::new()),
            memory_browse_tool,
            memory_explore_tool,
            memory_timeline_tool: timeline_tool,
            memory_workspace_handle,
            memory_session_key_handle,
            gateway_context: {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                if let Some(ref ctx) = config.gateway_context {
                    let _ = cell.set(ctx.clone());
                }
                cell
            },
            session_new_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionNewTool::new),
            session_compact_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionCompactTool::new),
            session_set_topic_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionSetTopicTool::new),
            session_set_mode_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionSetModeTool::new),
            // session_search_tool: removed — now constructed on-the-fly from
            // GatewayContext in the dispatch path to enforce A2A policy filtering.
            cron_manage_tool: config
                .cron_service
                .as_ref()
                .map(|svc| crate::builtin_tools::cron_manage::CronManageTool::new(Arc::clone(svc))),
            heartbeat_list_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatListTool::new(Arc::clone(svc))
            }),
            heartbeat_create_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatCreateTool::new(Arc::clone(svc))
            }),
            heartbeat_update_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatUpdateTool::new(Arc::clone(svc))
            }),
            heartbeat_delete_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatDeleteTool::new(Arc::clone(svc))
            }),
            heartbeat_toggle_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatToggleTool::new(Arc::clone(svc))
            }),
            heartbeat_report_tool: crate::builtin_tools::heartbeat_manage::HeartbeatReportTool,
            // Phase 3 Task 19 — flag_user_correction tool (R9: everything is a tool).
            // Constructed inline because it only needs a memory backend, no separate
            // service. The cast to Arc<dyn RawMemoryStore> is zero-cost (vtable lookup).
            flag_user_correction_tool: config.memory_db.as_ref().map(|db| {
                crate::builtin_tools::FlagUserCorrectionTool::new(
                    db.clone()
                        as crate::sync_primitives::Arc<
                            dyn crate::memory::store::raw_memory::RawMemoryStore,
                        >,
                    // Corrections must be namespaced to the active agent so
                    // FeedbackDistill (which reads per-agent) can find them.
                    // Previously hardcoded "default" — broke multi-agent recall.
                    current_agent_id.clone(),
                )
            }),
            browser_open_tool,
            browser_click_tool,
            browser_type_tool,
            browser_screenshot_tool,
            browser_snapshot_tool,
            browser_navigate_tool,
            browser_tabs_tool,
            browser_select_tool,
            browser_evaluate_tool,
            browser_fill_form_tool,
            browser_press_key_tool,
            browser_wait_for_tool,
            browser_console_tool,
            browser_hover_tool,
            browser_scroll_tool,
            browser_pdf_tool,
            browser_network_tool,
            browser_dialog_tool,
            browser_drag_tool,
            browser_upload_tool,
            browser_resize_tool,
            browser_emulate_tool,
            browser_cookies_tool,
            browser_session_tool,
            browser_profile_tool,
            agent_create_tool,
            agent_list_tool,
            agent_delete_tool,
            agent_switch_tool,
            agent_info_tool,
            session_context_handle,
            tool_context_handle: config.tool_context.clone(),
            event_bus: config.event_bus.clone(),
            extension_manager: config.extension_manager.clone(),
            acp_delegate_tool,
            acp_switch_tool,
            acp_session_control_tool,
            a2a_delegate_tool,
            a2a_agents_tool,
            channel_registry_cell: {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                if let Some(ref cr) = config.channel_registry {
                    let _ = cell.set(cr.clone());
                }
                cell
            },
            // ClarificationManager is always injected via deferred wiring at
            // boot (created alongside channels) — start the cell empty.
            clarification_manager_cell: Arc::new(tokio::sync::OnceCell::new()),
            clawhub_tool: match crate::builtin_tools::clawhub::ClawHubTool::new() {
                Ok(tool) => Some(tool),
                Err(e) => {
                    warn!("ClawHubTool disabled: {}", e);
                    None
                }
            },
            // Wire the LLM-facing routing query tool from the SAME live config
            // the inbound gateway snapshots (`subsystems.rs::with_route_bindings`).
            // Previously `::default()` gave it empty bindings + default session
            // config, so `gateway_route` always answered "main"/"default" no
            // matter what `[routing]` configured — the tool lied to the model
            // (violating R8: configuration must be truthfully queryable). Snapshot
            // here (not live-read) to stay in parity with the router, which also
            // snapshots at boot — a live tool against a snapshotted router would
            // diverge again after a config reload.
            gateway_route_tool: match config.config {
                Some(ref cfg) => {
                    let guard = cfg.read().await;
                    crate::builtin_tools::gateway_route::GatewayRouteTool::new(
                        guard.bindings.clone(),
                        guard.session.clone(),
                        crate::routing::DEFAULT_AGENT_ID.to_string(),
                    )
                }
                None => crate::builtin_tools::gateway_route::GatewayRouteTool::with_defaults(),
            },
            task_create_tool,
            task_update_tool,
            task_list_tool,
            task_wait_tool,
            task_comment_tool,
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_set_protocol_tool,
            team_member_add_tool,
            team_member_remove_tool,
            team_digest_tool,
            team_from_template_tool,
            team_snapshot_tool,
            team_usage_tool,
            team_acp_member_tool,
            team_workflow_canvas_tool,
            workflow_step_review_tool,
            workflow_tool,
            team_task_control_tool,
            task_exit_journal_tool,
            message_send_tool,
            inbox_read_tool,
            plan_submit_tool,
            plan_resolve_tool,
            lifecycle_idle_tool,
            lifecycle_request_shutdown_tool,
            lifecycle_resolve_shutdown_tool,
            session_collaborate_tool,
            session_turn_tool,
            session_read_tool,
            google_meet_tool,
            skill_status_tool,
            skill_install_tool,
            skill_manage_tool,
            note_manage_tool,
            session_complete_tool,
            memory_reflect_tool,
            note_orient_tool,
            note_schema_tool,
            user_profile_tool,
            // Per-call dependency handles for tools wired into dispatch but not
            // held as constructed instances (config_audit / media_* / recall_context).
            config: config.config.clone(),
            media_pipeline: config.media_pipeline.clone(),
            memory_project_scoped: config.memory_project_scoped,
            recall_context_db: config.memory_db.clone(),
            memory_trace_db: config.memory_db.clone(),
            tools,
        })
    }
}
