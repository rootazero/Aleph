# embedding-inference Specification

## Purpose
TBD - created by archiving change add-contextual-memory-rag. Update Purpose after archive.
## Requirements
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

