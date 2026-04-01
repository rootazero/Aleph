# Generation Providers Wiring Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up the existing generation provider system so `generate_image`/`generate_video`/`generate_audio` tools work at runtime, with hot-reload and flexible openai_compat URLs.

**Architecture:** Three independent changes: (1) startup wiring reads config and injects generation registry into BuiltinToolConfig, (2) a background task rebuilds the registry on config change events, (3) openai_compat treats `base_url` as a full endpoint URL instead of appending paths.

**Tech Stack:** Rust, Leptos (WASM Panel UI), tokio broadcast channels, serde_json

**Spec:** `docs/superpowers/specs/2026-03-18-generation-providers-wiring-design.md`

---

## Chunk 1: openai_compat URL Fix

### Task 1: Add `edit_url` field to GenerationProviderConfig

**Files:**
- Modify: `src/config/types/generation/provider.rs:38-87` (struct definition)
- Modify: `src/config/types/generation/provider.rs:101-116` (Default impl)

- [ ] **Step 1: Add `edit_url` field to struct**

In `src/config/types/generation/provider.rs`, add after line 86 (`pub verified: bool`):

```rust
    /// Optional explicit edit endpoint URL (for openai_compat providers with non-standard edit paths)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,
```

- [ ] **Step 2: Add `edit_url` to Default impl**

In the `Default` impl, add `edit_url: None,` after `verified: false,`.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (field is Option with serde default, so existing code using `..Default::default()` still works)

- [ ] **Step 4: Commit**

```bash
git add src/config/types/generation/provider.rs
git commit -m "generation: add edit_url field to GenerationProviderConfig"
```

---

### Task 2: Add `edit_endpoint` to OpenAiCompatProvider struct + builder

**Files:**
- Modify: `src/generation/providers/openai_compat/provider.rs:39-54` (struct)
- Modify: `src/generation/providers/openai_compat/builder.rs:32-47` (builder struct)
- Modify: `src/generation/providers/openai_compat/builder.rs:127-181` (build method)

- [ ] **Step 1: Add field to provider struct**

In `src/generation/providers/openai_compat/provider.rs`, add after line 53 (`pub(crate) supported_types: Vec<GenerationType>`):

```rust
    /// Optional explicit edit endpoint URL
    pub(crate) edit_endpoint: Option<String>,
```

- [ ] **Step 2: Add field + method to builder**

In `src/generation/providers/openai_compat/builder.rs`, add to the builder struct (after line 46, `pub(crate) timeout_secs: u64`):

```rust
    /// Optional explicit edit endpoint URL
    pub(crate) edit_endpoint: Option<String>,
```

In `new()` (line 63-71), add `edit_endpoint: None,` to the struct init.

Add builder method after `timeout_secs()` (after line 112):

```rust
    /// Set an explicit edit endpoint URL
    ///
    /// If not set, edit URL is derived from the generations URL by replacing
    /// "/generations" with "/edits".
    pub fn edit_endpoint<S: Into<String>>(mut self, url: S) -> Self {
        self.edit_endpoint = Some(url.into());
        self
    }
```

- [ ] **Step 3: Remove `/v1` normalization and wire `edit_endpoint` in `build()`**

In `build()` (lines 163-170), replace the normalization block:

```rust
        // Normalize base URL (remove trailing slash and /v1 suffix)
        // This prevents duplicate /v1 in the final URL when user provides "https://api.example.com/v1"
        let endpoint = self
            .base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();
```

With:

```rust
        // base_url is the full endpoint URL — only strip trailing slash
        let endpoint = self.base_url.trim_end_matches('/').to_string();
```

And in the `Ok(OpenAiCompatProvider { ... })` block (lines 172-180), add `edit_endpoint: self.edit_endpoint,` after `supported_types`.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/generation/providers/openai_compat/provider.rs src/generation/providers/openai_compat/builder.rs
git commit -m "generation: add edit_endpoint to openai_compat provider + remove /v1 normalization"
```

---

### Task 3: Change URL logic in helpers.rs

**Files:**
- Modify: `src/generation/providers/openai_compat/helpers.rs:10-19`

- [ ] **Step 1: Replace URL methods**

In `src/generation/providers/openai_compat/helpers.rs`, replace lines 11-19:

```rust
    /// Get the full URL for the generations endpoint
    /// base_url is already the full endpoint URL (e.g. "https://ai.t8star.cn/v1/images/generations")
    pub(crate) fn generations_url(&self) -> String {
        self.endpoint.clone()
    }

    /// Get the full URL for the edits endpoint
    /// Uses explicit edit_endpoint if set, otherwise derives from generations URL
    pub(crate) fn edits_url(&self) -> String {
        if let Some(ref edit_url) = self.edit_endpoint {
            return edit_url.clone();
        }
        // Heuristic for standard OpenAI-style URLs: /generations → /edits
        // For non-standard URLs, returns unchanged (those providers don't support editing)
        self.endpoint.replace("/generations", "/edits")
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/generation/providers/openai_compat/helpers.rs
git commit -m "generation: openai_compat uses base_url as full endpoint URL"
```

---

### Task 4: Update existing tests for new URL semantics

**Files:**
- Modify: `src/generation/providers/openai_compat/mod.rs:315-660` (URL tests)

The existing tests expect the old behavior where `base_url = "https://api.example.com"` produces `https://api.example.com/v1/images/generations`. Under the new semantics, `base_url` IS the full URL.

- [ ] **Step 1: Update URL tests**

In `src/generation/providers/openai_compat/mod.rs`, update these tests:

`test_generations_url` (line 318): Change base_url to full URL, update assertion:
```rust
    #[test]
    fn test_generations_url() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://api.example.com/v1/images/generations", None,
        ).unwrap();
        assert_eq!(provider.generations_url(), "https://api.example.com/v1/images/generations");
    }
```

`test_generations_url_with_trailing_slash` (line 329): Update similarly:
```rust
    #[test]
    fn test_generations_url_with_trailing_slash() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://api.example.com/v1/images/generations/", None,
        ).unwrap();
        assert_eq!(provider.generations_url(), "https://api.example.com/v1/images/generations");
    }
```

`test_generations_url_with_v1_suffix` (line 340): Now tests custom endpoint:
```rust
    #[test]
    fn test_generations_url_custom_endpoint() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://ai.t8star.cn/suno/generate", None,
        ).unwrap();
        assert_eq!(provider.generations_url(), "https://ai.t8star.cn/suno/generate");
    }
```

`test_generations_url_with_v1_and_trailing_slash` (line 353): Test video endpoint:
```rust
    #[test]
    fn test_generations_url_video_endpoint() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://ai.t8star.cn/v2/videos/generations", None,
        ).unwrap();
        assert_eq!(provider.generations_url(), "https://ai.t8star.cn/v2/videos/generations");
    }
```

`test_edits_url` (line 640): Update for new semantics:
```rust
    #[test]
    fn test_edits_url_derived() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://api.example.com/v1/images/generations", None,
        ).unwrap();
        assert_eq!(provider.edits_url(), "https://api.example.com/v1/images/edits");
    }
```

`test_edits_url_with_v1_suffix` (line 651): Test explicit edit_endpoint:
```rust
    #[test]
    fn test_edits_url_explicit() {
        let provider = OpenAiCompatProvider::builder("proxy", "key", "https://api.example.com/v1/images/generations")
            .edit_endpoint("https://api.example.com/v1/images/edits")
            .build()
            .unwrap();
        assert_eq!(provider.edits_url(), "https://api.example.com/v1/images/edits");
    }
```

Add new test for non-standard URL (no `/generations` to replace):
```rust
    #[test]
    fn test_edits_url_non_standard_unchanged() {
        let provider = OpenAiCompatProvider::new(
            "proxy", "key", "https://ai.t8star.cn/suno/generate", None,
        ).unwrap();
        // No "/generations" to replace, returns unchanged
        assert_eq!(provider.edits_url(), "https://ai.t8star.cn/suno/generate");
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib openai_compat`
Expected: All URL tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/generation/providers/openai_compat/mod.rs
git commit -m "generation: update openai_compat tests for full-URL semantics"
```

---

### Task 5: Wire `edit_url` through factory

**Files:**
- Modify: `src/generation/providers/mod.rs:147-169` (openai_compat branch in create_provider)

- [ ] **Step 1: Pass edit_url in factory**

In `src/generation/providers/mod.rs`, in the `"openai_compat"` match arm (around line 147), after `builder = builder.color(&config.color);` (line 161), add:

```rust
            if let Some(ref edit_url) = config.edit_url {
                builder = builder.edit_endpoint(edit_url);
            }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib openai_compat`
Expected: PASS

- [ ] **Step 3: Also update the factory test for openai_compat**

In the same file, update `test_create_openai_compat_provider` (around line 351) to use full endpoint URL:

```rust
    #[test]
    fn test_create_openai_compat_provider() {
        let config = GenerationProviderConfig {
            provider_type: "openai_compat".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: Some("https://api.example.com/v1/images/generations".to_string()),
            models: vec!["custom-model".to_string()],
            color: "#ff5500".to_string(),
            capabilities: vec![GenerationType::Image, GenerationType::Video],
            ..Default::default()
        };

        let provider = create_provider("my-proxy", &config).unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff5500");
        assert_eq!(provider.default_model(), Some("custom-model"));
        assert!(provider.supports(GenerationType::Image));
        assert!(provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Speech));
    }
```

Also update `test_create_compat_with_custom_base_url` (around line 441):

```rust
    #[test]
    fn test_create_compat_with_custom_base_url() {
        let config = GenerationProviderConfig {
            provider_type: "openai_compat".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: Some("https://custom.api.com/v2/generate".to_string()),
            ..Default::default()
        };

        let provider = create_provider("custom", &config).unwrap();
        assert_eq!(provider.name(), "custom");
    }
```

- [ ] **Step 4: Run all generation tests**

Run: `cargo test -p alephcore --lib generation`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/generation/providers/mod.rs
git commit -m "generation: wire edit_url through factory, update tests for full-URL"
```

---

## Chunk 2: Startup Wiring + Hot-Reload

### Task 6: Add `shared_token_mgr` parameter to `register_agent_handlers`

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs:87-100` (function signature)
- Modify: `src/bin/aleph/commands/start/mod.rs:461-466` (call site)

- [ ] **Step 1: Extend function signature**

In `src/bin/aleph/commands/start/builder/agent_init.rs`, add a parameter to `register_agent_handlers` (line 87-100):

After `daemon: bool,` (line 100), add:

```rust
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
```

- [ ] **Step 2: Update call site in mod.rs**

In `src/bin/aleph/commands/start/mod.rs` (line 461-466), add the vault parameter to the call (after `args.daemon`):

```rust
    let agent_result = register_agent_handlers(
        &mut server, session_manager.clone(), event_bus.clone(),
        router.clone(), &full_config, &*app_config.read().await, app_config.clone(), &memory_db,
        workspace_manager.clone(), agent_manager.clone(), acp_manager.clone(),
        cron_service.clone(), args.daemon,
        auth_bundle.auth_ctx.shared_token_mgr.clone(),
    ).await;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p aleph`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph/commands/start/builder/agent_init.rs src/bin/aleph/commands/start/mod.rs
git commit -m "generation: pass SharedTokenManager to register_agent_handlers"
```

---

### Task 7: Create generation registry at startup and inject into BuiltinToolConfig

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs:101-197`

- [ ] **Step 1: Add imports**

At top of `agent_init.rs`, add to imports (after existing `use` statements around line 7):

```rust
use alephcore::generation::{GenerationProviderRegistry, providers as gen_providers};
```

- [ ] **Step 2: Create generation registry before the AI provider conditional**

In `register_agent_handlers`, after line 110 (`let mut swappable_reg = ...`) and before the `let initial_provider = ...` block (line 112), insert:

```rust
    // Build generation provider registry (independent of chat AI provider)
    let generation_registry = {
        let mut registry = GenerationProviderRegistry::new();
        for (name, provider_cfg) in &app_config.generation.providers {
            if !provider_cfg.enabled { continue; }
            if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) { continue; }
            match gen_providers::create_provider(name, provider_cfg) {
                Ok(provider) => {
                    if registry.register(name.clone(), provider).is_ok() {
                        tracing::info!(provider = %name, "Registered generation provider");
                    }
                }
                Err(e) => {
                    tracing::warn!(provider = %name, error = %e, "Skip generation provider");
                }
            }
        }
        if !registry.is_empty() && !daemon {
            println!("  Generation providers: {} registered", registry.len());
        }
        Arc::new(std::sync::RwLock::new(registry))
    };
```

- [ ] **Step 3: Inject into BuiltinToolConfig**

In the `BuiltinToolConfig` construction (line 183-197), replace `..Default::default()` with explicit `generation_registry`:

Replace:
```rust
            cron_service: cron_service.clone(),
            ..Default::default()
```

With:
```rust
            cron_service: cron_service.clone(),
            generation_registry: Some(generation_registry.clone()),
            ..Default::default()
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p aleph`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph/commands/start/builder/agent_init.rs
git commit -m "generation: wire up generation provider registry at startup"
```

---

### Task 8: Spawn hot-reload background task

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs` (after generation registry creation)

- [ ] **Step 1: Add hot-reload task**

After the generation registry creation block (from Task 7), and still before the `let initial_provider = ...` line, add:

```rust
    // Hot-reload: rebuild generation registry when Panel updates providers
    {
        let gen_reg = generation_registry.clone();
        let config_handle = app_config_arc.clone();
        let vault = shared_token_mgr.clone();
        let mut rx = event_bus.subscribe();

        tokio::spawn(async move {
            while let Ok(event_json) = rx.recv().await {
                let is_gen_event = serde_json::from_str::<serde_json::Value>(&event_json)
                    .ok()
                    .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string()))
                    == Some("config.generation.providers.changed".to_string());
                if !is_gen_event {
                    continue;
                }

                // Snapshot config (drop read guard before creating providers)
                let providers_snapshot = {
                    let cfg = config_handle.read().await;
                    cfg.generation.providers.clone()
                };

                let mut new_registry = GenerationProviderRegistry::new();
                for (name, mut provider_cfg) in providers_snapshot {
                    if !provider_cfg.enabled {
                        continue;
                    }
                    // Resolve API key from vault (RPC handlers store keys in vault, not config)
                    if provider_cfg.api_key.is_none() {
                        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                            provider_cfg.api_key = Some(secret.expose().to_string());
                        }
                    }
                    if provider_cfg
                        .api_key
                        .as_ref()
                        .map(|k| k.is_empty())
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    match gen_providers::create_provider(&name, &provider_cfg) {
                        Ok(provider) => {
                            new_registry.register(name.clone(), provider).ok();
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %name, error = %e,
                                "Skip generation provider on reload"
                            );
                        }
                    }
                }

                let mut guard = gen_reg.write().unwrap_or_else(|e| e.into_inner());
                *guard = new_registry;
                tracing::info!(
                    "Generation provider registry reloaded ({} providers)",
                    guard.len()
                );
            }
        });
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p aleph`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph/commands/start/builder/agent_init.rs
git commit -m "generation: add hot-reload for generation provider registry"
```

---

## Chunk 3: Panel UI

### Task 9: Add `edit_url` to Panel API types

**Files:**
- Modify: `apps/panel/src/api.rs:1324-1340` (GenerationProviderConfig struct)

- [ ] **Step 1: Add field**

In `apps/panel/src/api.rs`, add after `pub timeout_seconds: u64,` (line 1337):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,
```

- [ ] **Step 2: Compile check**

Run: `cd apps/panel && cargo check`
Expected: PASS (serde default handles missing field)

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/api.rs
git commit -m "panel: add edit_url field to GenerationProviderConfig"
```

---

### Task 10: Update Panel UI labels and add edit_url input

**Files:**
- Modify: `apps/panel/src/views/settings/generation_providers.rs:898-908` (add form)
- Modify: `apps/panel/src/views/settings/generation_providers.rs:583` (detail view)

- [ ] **Step 1: Update "Base URL" label to "API Endpoint URL" in add form**

In `apps/panel/src/views/settings/generation_providers.rs`, around line 900, change:

```rust
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Base URL"</label>
```

To:

```rust
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Endpoint URL"</label>
```

And update the placeholder (line 905):

```rust
                        placeholder="https://api.example.com/v1/images/generations"
```

- [ ] **Step 2: Add edit_url signal and input field**

Add a new signal near line 728 (after `let base_url = RwSignal::new(String::new());`):

```rust
    let edit_url = RwSignal::new(String::new());
```

Add the `edit_url` field to `build_config()` closure. In the config construction, add after the `base_url` field:

```rust
            edit_url: {
                let url = edit_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
```

Add an input field after the base_url input block (after line 908):

```rust
                // Edit Endpoint URL (optional)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Edit Endpoint URL (optional)"</label>
                    <input
                        type="text"
                        value=move || edit_url.get()
                        on:input=move |ev| edit_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/edits"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"For image editing. Leave empty to auto-derive from endpoint URL."</p>
                </div>
```

- [ ] **Step 3: Update detail view label**

Around line 583, change:

```rust
                <DetailField label="Base URL" value=provider.config.base_url.clone().unwrap_or_else(|| "N/A".to_string()) />
```

To:

```rust
                <DetailField label="API Endpoint URL" value=provider.config.base_url.clone().unwrap_or_else(|| "N/A".to_string()) />
```

- [ ] **Step 4: Build Panel WASM**

Run: `cd apps/panel && cargo build --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/views/settings/generation_providers.rs
git commit -m "panel: update generation provider UI labels and add edit_url input"
```

---

### Task 11: Final integration check

- [ ] **Step 1: Run core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (pre-existing `markdown_skill` test failures are known and not our issue)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings from our changes

- [ ] **Step 3: Build full project**

Run: `cargo build --bin aleph`
Expected: PASS

- [ ] **Step 4: Final commit if any fixups needed**

```bash
git add -A && git commit -m "generation: fixup from integration check"
```

(Only if there are changes to commit)
