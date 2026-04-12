# Transcription Provider Separation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Separate Whisper/STT from Speech/TTS into an independent `Transcription` generation type with its own provider configuration and Panel UI tab.

**Architecture:** Add `Transcription` as the fifth `GenerationType` variant. Create `transcription_providers` config map. Rewire `TranscriptionService` and `SttConfig` initialization to read from this new config instead of deriving from Speech providers. Add `voices_url` field to `GenerationProviderConfig` for explicit voice endpoint configuration. Update Panel UI with a Transcription tab and clean up Speech form.

**Tech Stack:** Rust (alephcore), Leptos (WASM Panel UI), TOML config

**Spec:** `docs/superpowers/specs/2026-04-03-transcription-provider-separation-design.md`

---

### Task 1: Add `Transcription` variant to `GenerationType`

**Files:**
- Modify: `src/generation/types/generation_type.rs`

- [ ] **Step 1: Add `Transcription` to the enum**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationType {
    /// Image generation (DALL-E, Stable Diffusion, Midjourney, etc.)
    Image,
    /// Video generation (Runway, Pika, Sora, etc.)
    Video,
    /// Audio/music generation (Suno, Udio, etc.)
    Audio,
    /// Text-to-speech synthesis (ElevenLabs, OpenAI TTS, etc.)
    Speech,
    /// Speech-to-text transcription (Whisper, etc.)
    Transcription,
}
```

- [ ] **Step 2: Update all helper methods**

Add `Transcription` arms to every `match` in the impl block:

```rust
impl GenerationType {
    pub fn supports_style(&self) -> bool {
        matches!(self, GenerationType::Image | GenerationType::Video)
    }

    pub fn supports_voice(&self) -> bool {
        matches!(self, GenerationType::Speech)
    }

    pub fn is_long_running(&self) -> bool {
        matches!(self, GenerationType::Video | GenerationType::Audio)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            GenerationType::Image => "Image",
            GenerationType::Video => "Video",
            GenerationType::Audio => "Audio",
            GenerationType::Speech => "Speech",
            GenerationType::Transcription => "Transcription",
        }
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -60`

Expected: Compilation errors in files that have non-exhaustive `match` on `GenerationType` — this is expected and will be fixed in subsequent tasks. Note down all files with errors.

- [ ] **Step 4: Commit**

```bash
git add src/generation/types/generation_type.rs
git commit -m "generation: add Transcription variant to GenerationType"
```

---

### Task 2: Add `voices_url` to `GenerationProviderConfig` and remove `stt_model` from `GenerationDefaults`

**Files:**
- Modify: `src/config/types/generation/provider.rs`
- Modify: `src/config/types/generation/defaults.rs`

- [ ] **Step 1: Add `voices_url` field to `GenerationProviderConfig`**

In `src/config/types/generation/provider.rs`, add after the `edit_url` field:

```rust
    /// Optional explicit voices endpoint URL (for fetching available TTS voices)
    /// When omitted, auto-derived as {base_url}/v1/audio/voices
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voices_url: Option<String>,
```

Update `Default` impl to include `voices_url: None`.

- [ ] **Step 2: Remove `stt_model` from `GenerationDefaults`**

In `src/config/types/generation/defaults.rs`, delete these lines:

```rust
    /// STT (speech-to-text) model name (default: whisper-1)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -60`

Expected: Errors where `stt_model` and `voices_url` are referenced. Note them for subsequent tasks.

- [ ] **Step 4: Commit**

```bash
git add src/config/types/generation/provider.rs src/config/types/generation/defaults.rs
git commit -m "config: add voices_url to provider, remove stt_model from defaults"
```

---

### Task 3: Update `GenerationConfig` with `transcription_providers`

**Files:**
- Modify: `src/config/types/generation/config.rs`

- [ ] **Step 1: Add new fields to `GenerationConfig` struct**

Add after `default_speech_provider`:

```rust
    /// Default provider for transcription/STT
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_transcription_provider: Option<String>,
```

Add after `audio_providers`:

```rust
    /// Transcription/STT providers (typed format)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub transcription_providers: HashMap<String, GenerationProviderConfig>,
```

- [ ] **Step 2: Update `Default` impl**

Add to the `Default::default()` impl:

```rust
    default_transcription_provider: None,
    transcription_providers: HashMap::new(),
```

- [ ] **Step 3: Update `get_default_provider()`**

Add match arm:

```rust
    GenerationType::Transcription => self.default_transcription_provider.as_deref(),
```

- [ ] **Step 4: Update `get_provider()` lookup chain**

Add `transcription_providers` to the chain:

```rust
    pub fn get_provider(&self, name: &str) -> Option<&GenerationProviderConfig> {
        self.image_providers
            .get(name)
            .or_else(|| self.video_providers.get(name))
            .or_else(|| self.speech_providers.get(name))
            .or_else(|| self.audio_providers.get(name))
            .or_else(|| self.transcription_providers.get(name))
            .or_else(|| self.providers.get(name))
    }
```

- [ ] **Step 5: Update `get_enabled_providers()`**

Add `&self.transcription_providers` to the `typed_maps` array:

```rust
    let typed_maps: &[&HashMap<String, GenerationProviderConfig>] = &[
        &self.image_providers,
        &self.video_providers,
        &self.speech_providers,
        &self.audio_providers,
        &self.transcription_providers,
    ];
```

- [ ] **Step 6: Update `get_providers_for_type()`**

Add match arm:

```rust
    GenerationType::Transcription => &self.transcription_providers,
```

- [ ] **Step 7: Update `merged_providers()`**

Add after the `audio_providers` block:

```rust
    for (name, cfg) in &self.transcription_providers {
        seen.insert(name.clone());
        let mut cfg = cfg.clone();
        cfg.capabilities = vec![GenerationType::Transcription];
        result.push((name.clone(), cfg, GenerationType::Transcription));
    }
```

- [ ] **Step 8: Update `validate()`**

Add validation for `default_transcription_provider`:

```rust
    if let Some(ref provider) = self.default_transcription_provider {
        self.validate_provider_reference(provider, "default_transcription_provider")?;
    }
```

Add validation for `transcription_providers`:

```rust
    for (name, config) in &self.transcription_providers {
        config.validate(name)?;
    }
```

- [ ] **Step 9: Update `validate_provider_reference()`**

Add `&self.transcription_providers` to the `all_maps` array:

```rust
    let all_maps: &[&HashMap<String, GenerationProviderConfig>] = &[
        &self.image_providers,
        &self.video_providers,
        &self.speech_providers,
        &self.audio_providers,
        &self.transcription_providers,
        &self.providers,
    ];
```

- [ ] **Step 10: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -40`

Expected: Fewer errors now. Config layer should compile cleanly.

- [ ] **Step 11: Commit**

```bash
git add src/config/types/generation/config.rs
git commit -m "config: add transcription_providers and default_transcription_provider"
```

---

### Task 4: Update `url_normalize.rs` — remove Speech secondary endpoint, add Transcription primary

**Files:**
- Modify: `src/generation/providers/url_normalize.rs`

- [ ] **Step 1: Add `Transcription` to `primary_endpoint()`**

Add a match arm in `primary_endpoint()`:

```rust
    GenerationType::Transcription => "/v1/audio/transcriptions",
```

- [ ] **Step 2: Remove Speech from `secondary_endpoint()`**

Change the `secondary_endpoint` method — remove the Speech arm:

```rust
    pub fn secondary_endpoint(&self, gen_type: GenerationType) -> Option<String> {
        match self {
            ResolvedUrl::Custom(_) => None,
            ResolvedUrl::Standard(base) => {
                let suffix = match gen_type {
                    GenerationType::Image => Some("/v1/images/edits"),
                    _ => None,
                };
                suffix.map(|s| format!("{}{}", base, s))
            }
        }
    }
```

- [ ] **Step 3: Update tests**

Remove `test_secondary_endpoint_speech_stt` test. Update other tests if needed to account for the change.

- [ ] **Step 4: Compile and test**

Run: `cargo test -p alephcore --lib url_normalize 2>&1`

Expected: All remaining tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/generation/providers/url_normalize.rs
git commit -m "url_normalize: add Transcription primary endpoint, remove Speech secondary"
```

---

### Task 5: Clean up `OpenAiTtsProvider` — remove `stt_url()` and `resolved` field

**Files:**
- Modify: `src/generation/providers/openai_tts.rs`

- [ ] **Step 1: Remove `stt_url()` method**

Delete these lines (~214-218):

```rust
    /// Get the STT (speech-to-text / transcription) endpoint URL.
    /// Returns None for custom URLs or when secondary endpoint is unavailable.
    pub fn stt_url(&self) -> Option<String> {
        self.resolved.secondary_endpoint(GenerationType::Speech)
    }
```

- [ ] **Step 2: Remove `resolved` field from struct if no longer used**

Check if `self.resolved` is used elsewhere in the file besides `stt_url()` and the constructor. If `primary_endpoint` is the only other use, keep `resolved` for that. If `endpoint` already stores the resolved primary URL, consider removing `resolved` — but only if it's not used. The `resolved` field is currently used in `new()` to derive `endpoint` via `resolved.primary_endpoint(GenerationType::Speech)`, so it can be kept or replaced with storing the endpoint string directly. Since the endpoint is already stored in `self.endpoint`, and `stt_url()` was the only other use of `resolved`, remove the `resolved` field:

In the struct definition, remove:
```rust
    /// Resolved URL for deriving secondary endpoints (e.g., STT)
    resolved: ResolvedUrl,
```

In `new()`, change the tail to just compute the endpoint string without storing `resolved`:
```rust
    let resolved = resolved_url.unwrap_or_else(|| {
        let url = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        super::url_normalize::resolve_base_url(&url)
    });
    let endpoint = resolved.primary_endpoint(GenerationType::Speech);

    Ok(Self {
        client,
        api_key,
        endpoint,
        model,
        default_voice: voice,
    })
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -40`

Expected: Check callers of `stt_url()` — they will be fixed in Task 6.

- [ ] **Step 4: Commit**

```bash
git add src/generation/providers/openai_tts.rs
git commit -m "openai_tts: remove stt_url() and resolved field, speech is TTS-only"
```

---

### Task 6: Rewire `subsystems.rs` — STT config from transcription provider

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs` (~lines 390-430)

- [ ] **Step 1: Replace speech-provider-based STT init with transcription provider lookup**

Replace the block that finds a speech provider and derives STT config (lines ~390-430) with:

```rust
    // Wire STT config from dedicated transcription provider
    {
        let transcription_provider = gen_cfg
            .transcription_providers
            .iter()
            .find(|(name, pcfg)| {
                // Prefer default, else first enabled with API key
                if let Some(ref default_name) = gen_cfg.default_transcription_provider {
                    name.as_str() == default_name.as_str() && pcfg.enabled
                } else {
                    pcfg.enabled
                        && !pcfg.api_key.as_deref().unwrap_or("").is_empty()
                }
            })
            .or_else(|| {
                // Fallback: any enabled transcription provider with an API key
                gen_cfg.transcription_providers.iter().find(|(_, pcfg)| {
                    pcfg.enabled
                        && !pcfg.api_key.as_deref().unwrap_or("").is_empty()
                })
            });

        if let Some((_name, pcfg)) = transcription_provider {
            if let Some(ref key) = pcfg.api_key {
                let base = pcfg
                    .base_url
                    .as_deref()
                    .unwrap_or("https://api.openai.com");
                let resolved =
                    alephcore::generation::providers::url_normalize::resolve_base_url(base);
                let stt_endpoint =
                    resolved.primary_endpoint(alephcore::generation::GenerationType::Transcription);
                // Derive base_url for SttConfig: strip the /audio/transcriptions suffix
                let stt_base = stt_endpoint
                    .trim_end_matches("/audio/transcriptions")
                    .to_string();
                let stt_model = pcfg
                    .models
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "whisper-1".to_string());
                let stt = alephcore::gateway::voice::inbound::SttConfig {
                    api_key: key.clone(),
                    base_url: stt_base,
                    model: stt_model,
                };
                inbound_router = inbound_router.with_stt_config(stt);
                if !daemon {
                    println!("  Inbound router: voice STT transcription enabled (from transcription provider)");
                }
            }
        }
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check --bin aleph-server 2>&1 | head -40`

Expected: Should compile cleanly if previous tasks are done.

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "subsystems: read STT config from transcription provider instead of speech"
```

---

### Task 7: Rewire `agent_init.rs` — MediaProcessor transcription from transcription provider

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (~lines 920-958)

- [ ] **Step 1: Replace LLM-provider-based Whisper init with transcription provider lookup**

Replace the block at lines ~920-958 with:

```rust
        // Wire MediaProcessor for multimodal attachment handling (images, audio)
        {
            use alephcore::media::processor::MediaProcessor;
            use alephcore::media::transcription::TranscriptionService;
            use alephcore::media::whisper::WhisperTranscription;

            // Read transcription config from dedicated transcription provider
            let transcription: Option<Box<dyn TranscriptionService>> = {
                let gen_cfg = &app_config.generation;
                let tcfg = gen_cfg
                    .default_transcription_provider
                    .as_ref()
                    .and_then(|name| gen_cfg.transcription_providers.get(name))
                    .or_else(|| {
                        gen_cfg.transcription_providers.values().find(|pcfg| {
                            pcfg.enabled
                                && !pcfg.api_key.as_deref().unwrap_or("").is_empty()
                        })
                    });

                if let Some(pcfg) = tcfg {
                    if let Some(ref key) = pcfg.api_key {
                        if !key.is_empty() {
                            let whisper = WhisperTranscription::new(
                                key.clone(),
                                pcfg.base_url.clone(),
                                pcfg.models.first().cloned(),
                            );
                            if !daemon {
                                println!("  MediaProcessor: Whisper transcription enabled (from transcription provider)");
                            }
                            Some(Box::new(whisper))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
```

Note: The `app_config` variable here may be a `Config` struct or wrapped in `Arc<RwLock<>>`. Check the surrounding context — the existing code accesses `app_config.providers` (LLM providers), so the new code should access `app_config.generation.transcription_providers` through the same path. Adapt the variable access as needed.

- [ ] **Step 2: Compile check**

Run: `cargo check --bin aleph-server 2>&1 | head -40`

Expected: Should compile. If there are remaining `stt_model` references elsewhere, fix them.

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "agent_init: read transcription config from transcription provider"
```

---

### Task 8: Update `handle_voices` to use `voices_url`

**Files:**
- Modify: `src/gateway/handlers/generation_providers.rs` (~lines 825-842)

- [ ] **Step 1: Use `voices_url` from provider config if available**

Replace the voices URL derivation logic in `handle_voices()`. Change the block at lines ~825-842:

```rust
    // Step 1: Try dynamic fetch from provider API
    if let (Some(ref key), Some((_, pcfg, _))) = (&api_key, &provider_info) {
        // Use explicit voices_url if configured, otherwise derive from base_url
        let voices_url = if let Some(ref explicit_url) = pcfg.voices_url {
            explicit_url.clone()
        } else if let Some(ref base) = pcfg.base_url {
            // Normalize: strip trailing slash and /v1 so we always get {base}/v1/audio/voices
            let base = base.trim_end_matches('/');
            let base = base
                .strip_suffix("/v1")
                .unwrap_or(base)
                .trim_end_matches('/');
            format!("{}/v1/audio/voices", base)
        } else {
            String::new()
        };

        if !voices_url.is_empty() {
            if let Ok(voices) = fetch_voices_from_api(&voices_url, key).await {
                if !voices.is_empty() {
                    return JsonRpcResponse::success(
                        request.id,
                        serde_json::to_value(voices).unwrap_or_default(),
                    );
                }
            }
        }
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check --bin aleph-server 2>&1 | head -40`

Expected: Clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/generation_providers.rs
git commit -m "generation_providers: use voices_url from config for voice list fetch"
```

---

### Task 9: Fix remaining compile errors from `GenerationType` exhaustiveness

**Files:**
- Modify: Any files that have non-exhaustive `match` on `GenerationType`

- [ ] **Step 1: Find all remaining compile errors**

Run: `cargo check 2>&1 | grep "error\[" | head -30`

Common locations that need `Transcription` arms:
- Gateway handlers that match on `GenerationType`
- Panel-side `GenerationType` enum (if duplicated in webchat crate)
- Any `match gen_type { ... }` blocks

- [ ] **Step 2: Fix each file**

For each file with a non-exhaustive match, add the `Transcription` arm. The typical pattern:
- If the match handles "provider type mapping" → map `Transcription` to `"openai_compat"`
- If the match handles "display" → return `"Transcription"`
- If the match handles "capability routing" → include `transcription_providers`

- [ ] **Step 3: Full build check**

Run: `cargo check 2>&1 | head -40`

Expected: Clean compile across all crates.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "fix: add Transcription arms to all GenerationType match blocks"
```

---

### Task 10: Panel UI — Add Transcription category tab

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers.rs` (~lines 106-127)
- Modify: `interfaces/webchat/src/api/generation_providers.rs` (~lines 88-100)

- [ ] **Step 1: Add `Transcription` to `effective_generation_type()` mapping**

In `interfaces/webchat/src/api/generation_providers.rs`, add a match arm in `effective_generation_type()`:

```rust
    "transcription" => Some(GenerationType::Transcription),
```

- [ ] **Step 2: Add Transcription tab to category tabs**

In `interfaces/webchat/src/views/settings/generation_providers.rs`, add after the Speech tab (~line 126):

```rust
                        <CategoryTab
                            category=GenerationType::Transcription
                            selected=selected_category
                            on_select=set_selected_category
                        />
```

- [ ] **Step 3: Update `AddCustomProviderPanel` default provider type**

In `AddCustomProviderPanel` (~line 1345), add a match arm:

```rust
    let default_provider_type = match category {
        GenerationType::Speech => "openai_tts",
        GenerationType::Transcription => "openai_compat",
        GenerationType::Image => "openai",
        _ => "openai_compat",
    };
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p aleph-webchat 2>&1 | head -40`

Expected: Should compile. The Transcription provider form will use the generic form (no voice/speed fields) since `is_speech` is false for Transcription.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/generation_providers.rs interfaces/webchat/src/api/generation_providers.rs
git commit -m "panel: add Transcription category tab and provider type mapping"
```

---

### Task 11: Panel UI — Clean up Speech form (remove STT, add voices URL)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers.rs`

- [ ] **Step 1: Remove `form_stt_model` signal and STT section**

Remove the `form_stt_model` signal declaration (~line 542-549):

```rust
    // DELETE these lines:
    let form_stt_model = RwSignal::new(
        provider
            .config
            .defaults
            .stt_model
            .clone()
            .unwrap_or_else(|| "whisper-1".to_string()),
    );
```

Remove the `stt_model` from `build_config` closure (~line 599-600):

```rust
    // DELETE these lines:
    let stt = form_stt_model.get();
    defaults.stt_model = if stt.is_empty() { None } else { Some(stt) };
```

Remove the STT Model input section in the Voice Configuration card (~lines 954-965):

```rust
    // DELETE the entire STT Model <div> block
```

- [ ] **Step 2: Remove the "Derived Endpoints" STT line**

In the "ENDPOINTS (auto-derived)" section (~lines 981-990), remove the STT endpoint display:

```rust
    // DELETE the STT div:
    <div class="flex gap-2">
        <span class="text-text-tertiary w-8 shrink-0">"STT"</span>
        <span class="text-text-secondary break-all">
            {move || {
                let base = form_base_url.get();
                let base = extract_base_url(&base);
                format!("{}/v1/audio/transcriptions", base)
            }}
        </span>
    </div>
```

- [ ] **Step 3: Add `voices_url` input field to Speech provider form**

Add a `form_voices_url` signal after `form_edit_url` (~line 520):

```rust
    let form_voices_url = RwSignal::new(provider.config.voices_url.clone().unwrap_or_default());
```

Add `voices_url` to the `build_config` closure (after `edit_url` block):

```rust
    voices_url: {
        let url = form_voices_url.get();
        if url.is_empty() {
            None
        } else {
            Some(url)
        }
    },
```

Add a Voices URL input field in the Voice Configuration card (before the Default Voice dropdown):

```rust
    // Voices URL
    <div>
        <label class="block text-sm font-medium text-text-secondary mb-1">"Voices URL"</label>
        <input
            type="text"
            prop:value=move || form_voices_url.get()
            on:input=move |ev| form_voices_url.set(event_target_value(&ev))
            placeholder="https://example.com/v1/audio/voices"
            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
        />
        <p class="mt-1 text-xs text-text-tertiary">"Optional. Auto-derived from base URL if empty."</p>
    </div>
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p aleph-webchat 2>&1 | head -40`

Expected: Clean compile.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/generation_providers.rs
git commit -m "panel: remove STT from speech form, add voices_url input"
```

---

### Task 12: Full build verification and manual test

**Files:**
- No new files

- [ ] **Step 1: Full workspace build**

Run: `cargo build 2>&1 | tail -20`

Expected: Clean build with no errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test 2>&1 | tail -30`

Expected: All tests pass. Note any failures — they may be tests that reference `stt_model` or `stt_url` that were not caught earlier.

- [ ] **Step 3: Update local config for manual testing**

Edit `~/.aleph/config.toml` to add a transcription provider:

```toml
[generation]
default_transcription_provider = "t8star-stt"

[generation.transcription_providers.t8star-stt]
provider_type = "openai_compat"
api_key = "sk-xxx"
base_url = "https://ai.t8star.cn/v1/audio/transcriptions"
model = "whisper-1"
enabled = true
```

Also add `voices_url` to any existing speech provider if desired:

```toml
[generation.speech_providers.t8star]
voices_url = "https://ai.t8star.cn/v1/audio/voices"
```

- [ ] **Step 4: Manual smoke test**

Start the server: `cargo run --bin aleph-server -- start`

Verify:
1. Console shows "Whisper transcription enabled (from transcription provider)" 
2. Console shows "voice STT transcription enabled (from transcription provider)"
3. Open Panel → Generation → Transcription tab is visible
4. Can add/edit transcription providers
5. Speech provider form no longer shows STT model field
6. Speech provider form shows Voices URL field

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix any issues found during manual testing"
```
