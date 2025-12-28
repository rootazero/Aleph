# embedding-inference Specification

## Purpose

The embedding-inference capability enables Aether to convert text into dense vector representations (embeddings) using a locally-running machine learning model, enabling semantic similarity search for memory retrieval.

## ADDED Requirements

### Requirement: Embedding Model Loading
The system SHALL load a pre-trained sentence embedding model (`all-MiniLM-L6-v2`) from local storage using ONNX Runtime.

#### Scenario: Load model on first use (lazy loading)
- **GIVEN** memory feature is enabled
- **AND** no embedding model is currently loaded in memory
- **WHEN** the first text embedding is requested
- **THEN** the system loads the tokenizer from `~/.config/aether/models/all-MiniLM-L6-v2/tokenizer.json`
- **AND** loads the ONNX model from `~/.config/aether/models/all-MiniLM-L6-v2/model.onnx`
- **AND** initializes an ONNX Runtime session
- **AND** caches the session in memory for subsequent requests
- **AND** completes model loading within 2 seconds

#### Scenario: Model files missing
- **WHEN** model files do not exist at expected paths
- **THEN** the system returns an error: `AetherError::EmbeddingModelNotFound`
- **AND** logs detailed path information for debugging
- **AND** provides user-friendly error message: "Embedding model not found. Please download model files."
- **AND** disables memory functionality for the session

#### Scenario: Model load failure (corrupt files)
- **WHEN** model files exist but are corrupted or incompatible
- **THEN** ONNX Runtime throws an error during session initialization
- **AND** the system catches the error and returns `AetherError::EmbeddingModelLoadFailed`
- **AND** logs the underlying ONNX error details
- **AND** disables memory functionality

#### Scenario: Reuse loaded model
- **GIVEN** model successfully loaded in previous request
- **WHEN** subsequent embedding is requested
- **THEN** the system reuses the cached ONNX session
- **AND** does NOT reload model files from disk
- **AND** inference starts immediately

---

### Requirement: Text Embedding Generation
The system SHALL generate 384-dimensional float32 vector embeddings for input text.

#### Scenario: Embed single text string
- **GIVEN** input text: "What are the key milestones for Phase 2?"
- **WHEN** `embed_text(text)` is called
- **THEN** the system tokenizes the text using the model tokenizer
- **AND** truncates to 256 tokens if longer (model max length)
- **AND** runs ONNX inference to generate token embeddings
- **AND** applies mean pooling over token embeddings
- **AND** L2-normalizes the resulting vector
- **AND** returns a Vec<f32> of length 384
- **AND** completes within 100ms on modern hardware (Apple Silicon M1+, Intel i7+)

#### Scenario: Embed empty string
- **GIVEN** input text: ""
- **WHEN** `embed_text("")` is called
- **THEN** the system returns a zero vector (384 zeros)
- **OR** returns an error: `AetherError::EmptyInput`
- **DECISION**: Return zero vector for simplicity (less error handling)

#### Scenario: Embed very long text
- **GIVEN** input text with 1000 tokens (exceeds 256 token limit)
- **WHEN** `embed_text(text)` is called
- **THEN** the system truncates to first 256 tokens
- **AND** logs a warning: "Text truncated to 256 tokens for embedding"
- **AND** generates embedding from truncated text
- **AND** returns 384-dim vector

#### Scenario: Embed text with special characters
- **GIVEN** input text: "Hello! @user #tag $100 https://example.com"
- **WHEN** `embed_text(text)` is called
- **THEN** the tokenizer handles special characters according to model training
- **AND** generates embedding without errors
- **AND** returns 384-dim vector

---

### Requirement: Embedding Model Configuration
The system SHALL support configurable embedding model selection (future-proofing).

#### Scenario: Use default model
- **GIVEN** config does not specify embedding_model
- **WHEN** EmbeddingModel is initialized
- **THEN** defaults to "all-MiniLM-L6-v2"
- **AND** loads from default path

#### Scenario: Use custom model path
- **GIVEN** config specifies: `embedding_model = "custom-model"`
- **WHEN** EmbeddingModel is initialized
- **THEN** the system looks for model at `~/.config/aether/models/custom-model/`
- **AND** loads `model.onnx` and `tokenizer.json` from that directory
- **AND** verifies output dimensions match expected (384)

#### Scenario: Verify embedding dimensions
- **WHEN** model is loaded
- **THEN** the system inspects ONNX model output shape
- **AND** verifies output dimension is 384
- **AND** returns error if dimension mismatch

---

### Requirement: Embedding Determinism
The system SHALL generate identical embeddings for identical input text across runs.

#### Scenario: Reproduce embeddings
- **GIVEN** text: "Hello world"
- **WHEN** `embed_text("Hello world")` is called twice
- **THEN** both calls return identical Vec<f32> (bitwise equality)
- **AND** no randomness or non-determinism in inference

---

### Requirement: Concurrency Safety
The system SHALL support concurrent embedding requests from multiple threads.

#### Scenario: Concurrent embedding requests
- **GIVEN** 5 threads each calling `embed_text()` simultaneously
- **WHEN** all requests execute concurrently
- **THEN** the system uses a Mutex or similar to serialize access to ONNX session
- **AND** all 5 requests complete successfully
- **AND** no data races or panics occur
- **AND** embeddings are correct

---

### Requirement: Performance Monitoring
The system SHALL log embedding inference time for performance debugging.

#### Scenario: Log slow inference
- **WHEN** embedding inference takes longer than 150ms
- **THEN** the system logs a warning with timing details
- **AND** includes text length and token count in log
- **EXAMPLE**: "Slow embedding inference: 200ms for 1500 chars (250 tokens)"

#### Scenario: Track average inference time
- **WHEN** embedding service is running
- **THEN** the system maintains a moving average of inference times
- **AND** exposes this metric via stats API (optional)

---

## MODIFIED Requirements

N/A - This is a new capability.

---

## REMOVED Requirements

N/A - No removals.

---

## Cross-References

### Dependencies
- **core-library**: Uses tokio async runtime for non-blocking operations
- **External**: ONNX Runtime library (via `ort` crate)
- **External**: Hugging Face tokenizers (via `tokenizers` crate)

### Consumers
- **memory-storage**: Calls embedding generation before storing memories
- **memory-augmentation**: Embeds query text for retrieval

---

## Implementation Notes

### Model Specifications
- **Model**: `sentence-transformers/all-MiniLM-L6-v2`
- **Architecture**: 6-layer MiniLM (BERT-style transformer)
- **Input**: Text string (max 256 tokens)
- **Output**: 384-dimensional float32 vector (L2-normalized)
- **Size**: ~23MB (ONNX format)
- **License**: Apache 2.0
- **Source**: https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2

### Rust Crates
```toml
[dependencies]
ort = { version = "2.0", features = ["download-binaries"] }
tokenizers = "0.15"
```

### Inference Pipeline
1. **Tokenization**: Convert text to token IDs
2. **Truncation**: Limit to 256 tokens
3. **Padding**: Add padding tokens if needed (for batching)
4. **Inference**: Run through ONNX model
5. **Pooling**: Mean pool over token embeddings
6. **Normalization**: L2-normalize to unit vector

### Memory Management
- Model session: ~200MB resident memory
- Keep loaded for lifetime of AetherCore (no unloading)
- Consider unloading if inactive for >1 hour (future optimization)

### Error Types
```rust
pub enum EmbeddingError {
    ModelNotFound(PathBuf),
    ModelLoadFailed(String),
    TokenizationFailed(String),
    InferenceFailed(String),
    DimensionMismatch { expected: usize, got: usize },
}
```

---

## Performance Targets

### Latency
- **Cold start** (first load): <2 seconds
- **Inference** (single text): <100ms (target), <150ms (acceptable)
- **Batch inference** (10 texts): <300ms

### Throughput
- **Sequential**: ~10-15 embeddings/second
- **Parallel** (future): ~30-50 embeddings/second with batching

### Memory Usage
- **Model**: ~200MB resident
- **Per-request overhead**: <10MB (tokenization + inference buffers)

---

## Testing Strategy

### Unit Tests
```rust
#[tokio::test]
async fn test_embed_text() {
    let model = EmbeddingModel::new(model_path()).unwrap();
    let embedding = model.embed_text("Hello world").await.unwrap();
    assert_eq!(embedding.len(), 384);
    assert!(embedding.iter().all(|x| x.is_finite()));
}

#[tokio::test]
async fn test_embedding_determinism() {
    let model = EmbeddingModel::new(model_path()).unwrap();
    let e1 = model.embed_text("Test").await.unwrap();
    let e2 = model.embed_text("Test").await.unwrap();
    assert_eq!(e1, e2);
}

#[tokio::test]
async fn test_embedding_normalization() {
    let model = EmbeddingModel::new(model_path()).unwrap();
    let embedding = model.embed_text("Test").await.unwrap();
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);  // L2-normalized
}
```

### Integration Tests
- Test with real model files (not mocks)
- Verify embeddings match known reference vectors
- Test concurrent requests (10 threads)

### Benchmarks
```rust
#[bench]
fn bench_embedding_inference(b: &mut Bencher) {
    let model = EmbeddingModel::new(model_path()).unwrap();
    b.iter(|| {
        model.embed_text("Sample text for benchmarking").await.unwrap()
    });
}
```

---

## Security Considerations

### Model Provenance
- Model must be downloaded from trusted source (Hugging Face)
- Verify SHA256 checksum after download (future enhancement)
- Document model version in config

### Input Validation
- No untrusted code execution (ONNX is data only)
- Tokenizer handles arbitrary Unicode safely
- No SQL injection risk (not applicable)

### Resource Limits
- Enforce max input length (256 tokens)
- Cap concurrent inference requests (prevent DoS)
- Monitor memory usage, warn if excessive

---

## Acceptance Criteria

Implementation is complete when:
- [ ] All unit tests pass
- [ ] Can embed text and return 384-dim vector
- [ ] Inference completes in <100ms on target hardware
- [ ] Embeddings are deterministic (reproducible)
- [ ] Handles missing model files gracefully (error, not panic)
- [ ] Supports concurrent requests without crashes
- [ ] Integration test with real model passes
- [ ] Benchmark shows acceptable performance
- [ ] Manual test: Embed sample text, verify output shape
