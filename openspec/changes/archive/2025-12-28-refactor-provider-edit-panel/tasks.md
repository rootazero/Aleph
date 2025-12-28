# Implementation Tasks

## 1. Research and Planning
- [x] 1.1 Research OpenAI API official parameters (temperature, max_tokens, top_p, frequency_penalty, presence_penalty)
- [x] 1.2 Research Anthropic Claude API official parameters (max_tokens, temperature, top_p, top_k, stop_sequences)
- [x] 1.3 Research Google Gemini API official parameters (temperature, maxOutputTokens, topP, topK, stopSequences, thinking_level)
- [x] 1.4 Research Ollama API official parameters (temperature, num_predict, top_k, top_p, repeat_penalty, stop)
- [x] 1.5 Document parameter mappings and validation rules for each provider type

## 2. Update Rust Core Configuration Schema
- [x] 2.1 Extend ProviderConfig in `config/mod.rs` to support additional parameters
  - [x] Add optional fields: `top_p`, `top_k`, `frequency_penalty`, `presence_penalty`, `stop_sequences`
  - [x] Ensure backward compatibility with existing configs
- [x] 2.2 Update validation logic to check provider-specific parameter constraints
  - [x] OpenAI: temperature 0-2, max_tokens > 0, top_p 0-1, frequency_penalty -2 to 2, presence_penalty -2 to 2
  - [x] Claude: temperature 0-1, max_tokens > 0, top_p 0-1, top_k > 0
  - [x] Gemini: temperature 0-2, maxOutputTokens > 0, topP 0-1, topK > 0, thinking_level enum
  - [x] Ollama: temperature >= 0, num_predict > 0, top_k > 0, top_p 0-1, repeat_penalty >= 0
- [x] 2.3 Update UniFFI interface definition in `aether.udl` with new ProviderConfig fields
- [x] 2.4 Regenerate Swift bindings: `cargo run --bin uniffi-bindgen generate src/aether.udl --language swift --out-dir ../Sources/Generated/`

## 3. Update Provider Type Detection
- [x] 3.1 Enhance `infer_provider_type()` to support Gemini provider detection
- [x] 3.2 Add Gemini default color and icon to preset providers
- [x] 3.3 Update provider type list in ProviderEditPanel to include "gemini"

## 4. Refactor ProviderEditPanel UI Logic
- [x] 4.1 Remove "Configure this Provider" intermediate state in `presetProviderView()`
- [x] 4.2 Modify `selectProvider()` in ProvidersView.swift:
  - [x] When clicking a provider, always set `isEditing = true` (remove the intermediate view mode)
  - [x] Auto-populate form with existing config if provider is configured
  - [x] Auto-populate form with preset defaults if provider is not yet configured
- [x] 4.3 Update ProviderEditPanel.swift:
  - [x] Remove `viewModeContent` and `presetProviderView` logic
  - [x] Always display `editModeFormContent` when a provider is selected
  - [x] Pre-fill form fields from config or preset defaults
- [x] 4.4 Update form field organization:
  - [x] Group "Basic Settings" (name, type, API key, model, base URL)
  - [x] Group "Generation Parameters" (temperature, max_tokens, top_p, top_k, etc.)
  - [x] Group "Advanced Settings" (timeout, stop_sequences, penalties)
- [x] 4.5 Add conditional field rendering based on provider_type:
  - [x] OpenAI: Show temperature, max_tokens, top_p, frequency_penalty, presence_penalty
  - [x] Claude: Show temperature, max_tokens, top_p, top_k, stop_sequences
  - [x] Gemini: Show temperature, maxOutputTokens (label as "Max Tokens"), topP, topK, thinking_level dropdown
  - [x] Ollama: Show temperature, num_predict (label as "Max Tokens"), top_k, top_p, repeat_penalty, stop

## 5. Enhance Form Field UI
- [x] 5.1 Add help text for each parameter explaining its purpose
  - [x] temperature: "Controls randomness (0=deterministic, higher=more creative)"
  - [x] max_tokens: "Maximum length of generated response"
  - [x] top_p: "Nucleus sampling threshold (0-1)"
  - [x] top_k: "Top-K sampling (consider top K tokens)"
  - [x] frequency_penalty: "Reduce repetition based on token frequency (-2 to 2)"
  - [x] presence_penalty: "Encourage new topics (-2 to 2)"
  - [x] stop_sequences: "Sequences that stop generation (comma-separated)"
  - [x] thinking_level (Gemini): "Depth of reasoning (LOW/HIGH)"
- [x] 5.2 Add placeholder text with recommended default values
  - [x] OpenAI defaults: temp=1.0, max_tokens=1024, top_p=1.0
  - [x] Claude defaults: temp=1.0, max_tokens=1024, top_p=1.0
  - [x] Gemini defaults: temp=1.0, max_tokens=2048, top_p=0.95, thinking_level=HIGH
  - [x] Ollama defaults: temp=0.8, num_predict=512, top_k=40, top_p=0.9
- [x] 5.3 Mark required fields with asterisk (*) and optional fields with "(Optional)"
- [x] 5.4 Add inline validation error messages for invalid parameter values

## 6. Update Form Validation Logic
- [x] 6.1 Implement provider-specific validation in `isFormValid()`
  - [x] Validate parameter ranges based on provider_type
  - [x] Show specific error messages for invalid values
- [x] 6.2 Update `saveProviderConfig()` to persist new parameters
- [x] 6.3 Ensure `testConnection()` sends all configured parameters

## 7. Update Preset Provider Definitions
- [x] 7.1 Add Gemini preset to PresetProviders.swift (if not exists)
  - [x] name: "Google Gemini"
  - [x] id: "gemini"
  - [x] providerType: "gemini"
  - [x] defaultModel: "gemini-3-flash"
  - [x] baseUrl: "https://generativelanguage.googleapis.com/v1beta"
  - [x] color: "#4285F4" (Google Blue)
  - [x] icon: "sparkles"
- [x] 7.2 Update OpenAI preset with correct default model and parameters
- [x] 7.3 Update Claude preset with correct default model and parameters
- [x] 7.4 Update Ollama preset with correct default parameters

## 8. Testing
- [x] 8.1 Manual UI testing:
  - [x] Click each preset provider → verify form shows with correct defaults
  - [x] Click configured provider → verify form shows with saved values
  - [x] Change provider type → verify form fields update correctly
  - [x] Save config → verify all parameters persist correctly
  - [x] Test connection → verify parameters are sent to API
- [x] 8.2 Test parameter validation:
  - [x] Enter invalid temperature → verify error message
  - [x] Enter negative max_tokens → verify error message
  - [x] Enter out-of-range top_p → verify error message
- [x] 8.3 Test provider-specific fields:
  - [x] OpenAI: Verify frequency_penalty and presence_penalty fields appear
  - [x] Claude: Verify stop_sequences field appears
  - [x] Gemini: Verify thinking_level dropdown appears
  - [x] Ollama: Verify repeat_penalty field appears
- [x] 8.4 Regression testing:
  - [x] Verify existing configured providers load correctly
  - [x] Verify API key storage/retrieval from Keychain still works
  - [x] Verify Active/Inactive toggle still works
  - [x] Verify Test Connection still works

## 9. Documentation
- [x] 9.1 Update CLAUDE.md with new provider configuration parameters
- [x] 9.2 Add comments in code explaining provider-specific parameter mappings
- [x] 9.3 Update config.example.toml with examples for all new parameters
