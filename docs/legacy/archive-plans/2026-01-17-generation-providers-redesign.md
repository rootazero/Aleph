# Generation Providers Redesign

## Date: 2026-01-17

## Problem

1. **Bug**: `update_generation_provider` saves config to file but doesn't update in-memory `generation_registry`. Providers are unusable until app restart.
2. **UX**: Current flat list of 3 presets is insufficient. Need categorized view with more provider options.

## Solution

### Part 1: Bug Fix (Rust)

**File**: `Aether/core/src/ffi/config.rs`

Modify `update_generation_provider` to sync registry after saving:

```rust
pub fn update_generation_provider(&self, name: String, provider: GenerationProviderConfigFFI) -> Result<(), AlephFfiError> {
    let internal_config: GenerationProviderConfig = provider.into();

    // 1. Save to config file
    {
        let mut config = self.lock_config();
        config.generation.providers.insert(name.clone(), internal_config.clone());
        config.save()?;
    }

    // 2. Sync to in-memory registry
    if internal_config.enabled {
        let provider_instance = create_provider(&name, &internal_config)?;
        let mut registry = self.generation_registry.write().unwrap();
        // Remove existing if any
        let _ = registry.unregister(&name);
        registry.register(name.clone(), provider_instance)?;
        info!(provider = %name, "Generation provider registered to registry");
    }

    Ok(())
}
```

Modify `delete_generation_provider` to remove from registry:

```rust
pub fn delete_generation_provider(&self, name: String) -> Result<(), AlephFfiError> {
    // 1. Remove from config
    {
        let mut config = self.lock_config();
        config.generation.providers.remove(&name);
        config.save()?;
    }

    // 2. Remove from registry
    {
        let mut registry = self.generation_registry.write().unwrap();
        let _ = registry.unregister(&name);
        info!(provider = %name, "Generation provider removed from registry");
    }

    Ok(())
}
```

**File**: `Aether/core/src/generation/registry.rs`

Add `unregister` method:

```rust
pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
    self.providers.remove(name)
}
```

### Part 2: UI Redesign (Swift)

**File**: `Aether/Sources/GenerationProvidersView.swift`

#### 2.1 Add GenerationCategory Enum

```swift
enum GenerationCategory: String, CaseIterable {
    case image = "image"
    case video = "video"
    case audio = "audio"

    var displayName: String {
        switch self {
        case .image: return L("settings.generation.tab.image")
        case .video: return L("settings.generation.tab.video")
        case .audio: return L("settings.generation.tab.audio")
        }
    }

    var icon: String {
        switch self {
        case .image: return "photo"
        case .video: return "video"
        case .audio: return "waveform"
        }
    }
}
```

#### 2.2 Expand Preset Providers

**Image Providers (5+1)**:
- OpenAI DALL-E (openai, dall-e-3)
- Stability AI (stability, stable-diffusion-xl-1024-v1-0)
- Google Imagen (google, imagen-3.0-generate-001)
- Replicate (replicate, black-forest-labs/flux-schnell)
- Custom Image (openai_compat)

**Video Providers (4+1)**:
- Google Veo (google, veo-3)
- Runway (runway, gen-3)
- Pika (pika, pika-1.0)
- Luma (luma, dream-machine)
- Custom Video (openai_compat)

**Audio Providers (4+1)**:
- OpenAI TTS (openai, tts-1-hd)
- ElevenLabs (elevenlabs, eleven_multilingual_v2)
- Google TTS (google, en-US-Neural2-A)
- Azure TTS (azure, en-US-JennyNeural)
- Custom Audio (openai_compat)

#### 2.3 Tab Bar UI Layout

```
┌─────────────────────────────────────────────────────────────┐
│ [Search...]                              [+ Add Custom]     │
├─────────────────────────────────────────────────────────────┤
│ ┌───────────────────┐  ┌─────────────────────────────────┐  │
│ │ ┌───┬───┬───┐     │  │                                 │  │
│ │ │ 🖼 │ 🎬 │ 🔊 │     │  │      Edit Panel                │  │
│ │ └───┴───┴───┘     │  │                                 │  │
│ │ ─────────────     │  │      - Provider Name            │  │
│ │ • DALL-E          │  │      - API Key                  │  │
│ │ • Stability AI    │  │      - Model                    │  │
│ │ • Google Imagen   │  │      - Base URL                 │  │
│ │ • Replicate       │  │      - [Test Connection]        │  │
│ │ • Custom Image    │  │                                 │  │
│ │                   │  │                                 │  │
│ └───────────────────┘  └─────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Steps

1. [ ] Add `unregister` method to `GenerationProviderRegistry`
2. [ ] Fix `update_generation_provider` to sync registry
3. [ ] Fix `delete_generation_provider` to remove from registry
4. [ ] Add `GenerationCategory` enum in Swift
5. [ ] Expand preset providers list (15 presets + 3 custom)
6. [ ] Add `CategoryTab` component
7. [ ] Refactor `GenerationProvidersView` with tab bar layout
8. [ ] Add localization keys for new UI strings
9. [ ] Test save/delete/switch functionality
10. [ ] Build and verify

## Files to Modify

- `Aether/core/src/ffi/config.rs`
- `Aether/core/src/generation/registry.rs`
- `Aether/Sources/GenerationProvidersView.swift`
- `Aether/Sources/Localizable.xcstrings` (localization)
