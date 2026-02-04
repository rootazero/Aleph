# Protocol Adapter Phase 3 - OpenAI-Compatible Provider Expansion

**Date**: 2026-02-04
**Status**: 🚧 Planning
**Depends On**: Phase 2 (Complete)
**Branch**: feature/protocol-adapter-phase3

---

## 📋 Overview

Expand Aether's provider ecosystem by adding presets for popular OpenAI-compatible AI services. These providers all use the OpenAI chat completions API format, requiring only preset configurations (no new protocol adapters).

---

## 🎯 Objectives

1. Add presets for 8+ popular OpenAI-compatible providers
2. Verify compatibility with existing `OpenAiProtocol`
3. Add integration tests for each new provider
4. Update documentation with provider list
5. Maintain zero-code-change for protocol layer

---

## 🌐 Target Providers

### Tier 1: High Priority (Widely Used)

| Provider | Base URL | Color | Notes |
|----------|----------|-------|-------|
| **Groq** | `https://api.groq.com/openai/v1` | `#f55036` | Ultra-fast inference |
| **Together.ai** | `https://api.together.xyz/v1` | `#6366f1` | Open source models |
| **Perplexity** | `https://api.perplexity.ai` | `#20808d` | Search-augmented LLMs |
| **Mistral** | `https://api.mistral.ai/v1` | `#ff7000` | European AI leader |

### Tier 2: Medium Priority (Growing Popularity)

| Provider | Base URL | Color | Notes |
|----------|----------|-------|-------|
| **Cohere** | `https://api.cohere.ai/v1` | `#39594d` | Enterprise focus |
| **Fireworks.ai** | `https://api.fireworks.ai/inference/v1` | `#ff6b35` | Fast API |
| **Anyscale** | `https://api.endpoints.anyscale.com/v1` | `#00d4aa` | Ray ecosystem |
| **Replicate** | `https://api.replicate.com/v1` | `#0c0c0d` | OSS model hosting |

### Tier 3: Specialized/Regional

| Provider | Base URL | Color | Notes |
|----------|----------|-------|-------|
| **OpenRouter** | `https://openrouter.ai/api/v1` | `#7c3aed` | Multi-model router |
| **Lepton AI** | `https://api.lepton.ai/api/v1` | `#4f46e5` | Model deployment |
| **Hyperbolic** | `https://api.hyperbolic.xyz/v1` | `#8b5cf6` | GPU marketplace |

---

## 🏗️ Implementation Plan

### Phase 3a: Core Providers (Tier 1)

**Tasks**:
1. Add Groq preset
2. Add Together.ai preset
3. Add Perplexity preset
4. Add Mistral preset
5. Add validation tests
6. Update documentation

**Estimated Effort**: ~30 minutes
**Code Changes**: +60 lines (presets only)

### Phase 3b: Extended Providers (Tier 2)

**Tasks**:
1. Add Cohere preset
2. Add Fireworks.ai preset
3. Add Anyscale preset
4. Add Replicate preset
5. Add validation tests
6. Update documentation

**Estimated Effort**: ~20 minutes
**Code Changes**: +50 lines

### Phase 3c: Specialized Providers (Tier 3)

**Tasks**:
1. Add OpenRouter preset
2. Add Lepton AI preset
3. Add Hyperbolic preset
4. Add validation tests
5. Final documentation update

**Estimated Effort**: ~15 minutes
**Code Changes**: +40 lines

---

## 📝 Implementation Template

### Preset Format

```rust
// Groq - Ultra-fast inference
m.insert(
    "groq",
    ProviderPreset {
        base_url: "https://api.groq.com/openai/v1",
        protocol: "openai",
        color: "#f55036",
    },
);
```

### Test Format

```rust
#[test]
fn test_groq_preset() {
    let preset = get_preset("groq").unwrap();
    assert_eq!(preset.base_url, "https://api.groq.com/openai/v1");
    assert_eq!(preset.protocol, "openai");
    assert_eq!(preset.color, "#f55036");
}
```

---

## ✅ Validation Checklist

For each new provider:

- [ ] Preset added to `presets.rs`
- [ ] Base URL verified (official documentation)
- [ ] Brand color selected (from official website)
- [ ] Test added to `presets::tests`
- [ ] Compatible with `OpenAiProtocol` (chat completions endpoint)
- [ ] Documentation updated

---

## 🧪 Testing Strategy

### Unit Tests

```rust
#[test]
fn test_all_tier1_presets() {
    // Groq
    assert!(PRESETS.contains_key("groq"));
    // Together
    assert!(PRESETS.contains_key("together"));
    // Perplexity
    assert!(PRESETS.contains_key("perplexity"));
    // Mistral
    assert!(PRESETS.contains_key("mistral"));
}
```

### Integration Tests

```rust
#[test]
fn test_create_groq_provider() {
    let config = ProviderConfig::test_config("llama3-70b-8192");
    let provider = create_provider("groq", config);
    assert!(provider.is_ok());
    assert_eq!(provider.unwrap().name(), "groq");
}
```

---

## 📚 Documentation Updates

### User Documentation

Update `docs/PROVIDERS.md` with:

1. List of all supported providers (now 15+)
2. Configuration examples for each
3. Model recommendations per provider
4. Rate limits and pricing links

### Developer Documentation

Update `ARCHITECTURE.md` with:

1. Current provider count
2. OpenAI-compatible ecosystem
3. How to add new presets (5-line guide)

---

## 🎯 Success Metrics

| Metric | Target | Status |
|--------|--------|--------|
| **Tier 1 Providers** | 4/4 | 🚧 |
| **Tier 2 Providers** | 4/4 | 🚧 |
| **Tier 3 Providers** | 3/3 | 🚧 |
| **Total Providers** | 11+ new | 🚧 |
| **Code Added** | ~150 lines | 🚧 |
| **Tests Added** | 11+ tests | 🚧 |
| **Zero Protocol Changes** | ✅ | 🚧 |

---

## 🚀 Deployment Strategy

### Commit Strategy

1. **Commit 1**: Tier 1 providers (Groq, Together, Perplexity, Mistral)
2. **Commit 2**: Tier 2 providers (Cohere, Fireworks, Anyscale, Replicate)
3. **Commit 3**: Tier 3 providers (OpenRouter, Lepton, Hyperbolic)
4. **Commit 4**: Documentation and final tests

### Rollout Plan

- **Phase 3a**: Deploy Tier 1 first (most demand)
- **Phase 3b**: Deploy Tier 2 after validation
- **Phase 3c**: Deploy Tier 3 as bonus features

---

## 🔮 Future Considerations

### Phase 4 Candidates (Non-OpenAI Protocols)

1. **Cohere Native API**: Dedicated protocol for Cohere-specific features
2. **Replicate Predictions API**: Full Replicate protocol support
3. **HuggingFace Inference API**: HF-specific protocol
4. **AWS Bedrock**: Enterprise cloud deployment

### Automation Opportunities

1. **Provider Registry**: Auto-discover OpenAI-compatible APIs
2. **Health Checks**: Auto-verify endpoint availability
3. **Model Discovery**: Auto-fetch available models per provider
4. **Cost Tracking**: Per-provider usage analytics

---

## 📊 Impact Analysis

### User Benefits

- **16+ AI providers** available out-of-box (was 4)
- **Zero configuration** for popular services
- **Easy switching** between providers
- **Cost optimization** through provider comparison

### Developer Benefits

- **Minimal maintenance**: Presets only, no protocol code
- **Easy expansion**: 5 lines per new provider
- **Consistent behavior**: All use same `OpenAiProtocol`
- **Better testing**: Comprehensive preset validation

---

## 🎓 Lessons from Phase 2

### Apply These Wins

✅ Incremental commits per tier
✅ Comprehensive testing before merge
✅ Clear documentation updates
✅ Brand color consistency

### Avoid These Pitfalls

⚠️ Don't add untested providers
⚠️ Verify official base URLs
⚠️ Check for breaking API changes
⚠️ Maintain alphabetical preset order

---

## 📋 Execution Checklist

### Pre-Implementation

- [x] Review official API docs for each provider
- [x] Verify OpenAI compatibility
- [x] Select brand colors
- [ ] Create implementation branch

### Implementation (Per Tier)

- [ ] Add presets to `presets.rs`
- [ ] Add validation tests
- [ ] Verify factory routing works
- [ ] Update test count assertions
- [ ] Run full test suite
- [ ] Commit with descriptive message

### Post-Implementation

- [ ] Update ARCHITECTURE.md
- [ ] Create/update PROVIDERS.md
- [ ] Add migration guide (if needed)
- [ ] Final verification
- [ ] Merge to main

---

## 🎯 Definition of Done

Phase 3 is complete when:

1. ✅ All 11 provider presets added
2. ✅ All tests passing (65+ provider tests)
3. ✅ Documentation updated
4. ✅ Zero protocol adapter changes
5. ✅ Backward compatible with Phase 2
6. ✅ Ready for user testing

---

**Next Steps**: Execute Phase 3a (Tier 1 providers)
