# embedding-inference Specification

## Purpose
Local embedding inference for semantic similarity search in the Memory module.

## Requirements

### Requirement: Embedding Model Loading
The system SHALL load the `bge-small-zh-v1.5` embedding model using fastembed library.

#### Scenario: Load model on first use (lazy loading)
- **GIVEN** memory feature is enabled
- **AND** no embedding model is currently loaded in memory
- **WHEN** the first text embedding is requested
- **THEN** the system initializes fastembed with `BGESmallZHV15` model
- **AND** fastembed manages model download to `~/.cache/huggingface/hub/`
- **AND** caches the model session in memory for subsequent requests
- **AND** completes model loading within 5 seconds (includes download if needed)

#### Scenario: Model download on first use
- **GIVEN** model files do not exist in fastembed cache
- **WHEN** first embedding is requested
- **THEN** fastembed automatically downloads model from HuggingFace Hub
- **AND** shows download progress if configured
- **AND** stores model in `~/.cache/huggingface/hub/models--Xenova--bge-small-zh-v1.5/`

#### Scenario: Reuse loaded model
- **GIVEN** model successfully loaded in previous request
- **WHEN** subsequent embedding is requested
- **THEN** the system reuses the cached fastembed session
- **AND** does NOT reload model files from disk
- **AND** inference starts immediately

---

### Requirement: Text Embedding Generation
The system SHALL generate 512-dimensional float32 vector embeddings for input text.

#### Scenario: Embed single text string
- **GIVEN** input text: "What are the key milestones for Phase 2?"
- **WHEN** `embed_text(text)` is called
- **THEN** fastembed tokenizes and generates embedding
- **AND** returns a Vec<f32> of length 512
- **AND** completes within 100ms on modern hardware (Apple Silicon M1+, Intel i7+)

#### Scenario: Embed Chinese text (optimized)
- **GIVEN** input text: "这个项目的主要里程碑是什么？"
- **WHEN** `embed_text(text)` is called
- **THEN** the bge-small-zh-v1.5 model processes Chinese characters natively
- **AND** returns semantically meaningful 512-dim vector
- **AND** provides better Chinese similarity matching than general models

#### Scenario: Embed empty string
- **GIVEN** input text: ""
- **WHEN** `embed_text("")` is called
- **THEN** the system returns a zero vector (512 zeros)
- **OR** returns an error: `AetherError::EmptyInput`
- **DECISION**: Return zero vector for simplicity (less error handling)

#### Scenario: Embed very long text
- **GIVEN** input text with tokens exceeding model limit
- **WHEN** `embed_text(text)` is called
- **THEN** fastembed truncates to model's max token limit
- **AND** generates embedding from truncated text
- **AND** returns 512-dim vector

---

### Requirement: Embedding Model Configuration
The system SHALL support configurable embedding model selection (future-proofing).

#### Scenario: Use default model
- **GIVEN** config does not specify embedding_model
- **WHEN** EmbeddingModel is initialized
- **THEN** defaults to "bge-small-zh-v1.5"
- **AND** uses fastembed's `BGESmallZHV15` variant

#### Scenario: Verify embedding dimensions
- **WHEN** model is loaded
- **THEN** the system uses EMBEDDING_DIM constant (512)
- **AND** all vectors are 512-dimensional

---

### Requirement: Embedding Dimension Migration
The system SHALL handle embedding dimension changes gracefully.

#### Scenario: Detect dimension change
- **GIVEN** existing memories table with 384-dim vectors
- **AND** new model produces 512-dim vectors
- **WHEN** database is opened
- **THEN** the system detects dimension mismatch
- **AND** clears the memories table
- **AND** updates schema_info with new dimension
- **AND** logs migration details

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
- **THEN** the system uses Mutex to serialize access to fastembed session
- **AND** all 5 requests complete successfully
- **AND** no data races or panics occur
- **AND** embeddings are correct

---

### Requirement: Performance Monitoring
The system SHALL log embedding inference time for performance debugging.

#### Scenario: Log slow inference
- **WHEN** embedding inference takes longer than 100ms
- **THEN** the system logs a warning with timing details
- **AND** includes text length in log
- **EXAMPLE**: "Slow embedding inference: 150ms for 1500 chars"

---

### Requirement: SIMD Optimization
The system SHALL use SIMD-optimized cosine similarity calculation.

#### Scenario: Fast similarity search
- **GIVEN** query embedding and candidate embeddings
- **WHEN** similarity search is performed
- **THEN** the system uses SIMD instructions (AVX2/NEON) if available
- **AND** falls back to scalar operations otherwise
- **AND** completes vector comparison within 1ms per 1000 candidates
