# Static Occam's Razor Review Report: Batch GEN2 (`src/generation/providers`)

**Batch ID**: GEN2  
**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-gen-goal-gc`  
**Files Reviewed (11)**:
- `src/generation/providers/mod.rs`
- `src/generation/providers/http.rs`
- `src/generation/providers/url_normalize.rs`
- `src/generation/providers/factory.rs`
- `src/generation/providers/fal/mod.rs`
- `src/generation/providers/bfl/mod.rs`
- `src/generation/providers/bfl/tests.rs`
- `src/generation/providers/bfl/types.rs`
- `src/generation/providers/stability.rs`
- `src/generation/providers/google_imagen.rs`
- `src/generation/providers/openai_image.rs`

---

## 1. Summary of Findings

| ID | File:Line | Category | Severity | Title / Smell |
|---|---|---|---|---|
| **GEN2-01** | `stability.rs:280`<br>`google_imagen.rs:298, 313` | Error Semantics / Correctness | HIGH | 404 Model Not Found error reporting hardcodes `DEFAULT_MODEL` instead of requested model |
| **GEN2-02** | `google_imagen.rs:226-258` | Contract / Bug | HIGH | `determine_aspect_ratio` ignores `request.params.aspect_ratio` |
| **GEN2-03** | `stability.rs:180`<br>`google_imagen.rs:166` | Network / Normalization | MEDIUM | Missing trailing slash trim on `base_url` produces double-slash paths (`//v1/...`) |
| **GEN2-04** | `bfl/mod.rs:293`<br>`bfl/types.rs:49-51` | Contract / Dead Code | MEDIUM | Hardcoded `extras: None` in `bfl/mod.rs` drops model knobs and leaves `extras` dead |
| **GEN2-05** | `openai_image.rs:246-249, 372-418` | Retry / Correctness | MEDIUM | Missing `Retry-After` header extraction on HTTP 429 breaks `retry_transient` backoff |
| **GEN2-06** | `fal/mod.rs:267-269, 335, 483` | Allocation / Inefficiency | LOW | High-frequency allocation of invariant `auth_header` and `status_url` in 10-minute poll loop |
| **GEN2-07** | `openai_image.rs:165, 426-442, 477-495` | Allocation / Inefficiency | LOW | Redundant `.clone()` of URLs, base64 strings, and endpoint across request/response path |
| **GEN2-08** | `fal/mod.rs:429` | Dead Code | LOW | Dead `Serialize` derive on internal deserialization-only struct `FalStatusResponse` |
| **GEN2-09** | `bfl/mod.rs:92-95`<br>`stability.rs:172-175`<br>`google_imagen.rs:158-161`<br>`openai_image.rs:142-145` | Indirection / Duplication | LOW | Duplicated `reqwest::Client` inline construction bypassing `http::generation_http_client` |
| **GEN2-10** | `bfl/mod.rs:146, 336` | Allocation / Inefficiency | LOW | Unconditional text buffering in BFL poll loop and redundant sample URL cloning |

---

## 2. Detailed Findings

### GEN2-01: 404 Model Not Found Error Reporting Hardcodes `DEFAULT_MODEL`

- **Location**: src/generation/providers/stability.rs:280, src/generation/providers/google_imagen.rs:298, 313
- **Category**: Error Semantics / Correctness
- **Concrete Failure Mode**:
  - *Trigger*: An operator configures a custom model (e.g., `model = "stable-diffusion-v1-6"` or `model = "imagen-4.0-ultra-generate-001"`) that is nonexistent, deprecated, or restricted in their region, returning HTTP 404 from the provider API.
  - *Wrong Outcome*: `parse_error_response` ignores the provider instance's configured model and hardcodes `DEFAULT_MODEL`, returning `GenerationError::ModelNotFoundError("stable-diffusion-xl-1024-v1-0")` or `GenerationError::ModelNotFoundError("imagen-3.0-generate-002")`. Operators diagnosing the error are misled into believing their custom model config was silently ignored and the default model was queried instead.
- **Proposed Minimal Fix**:
  Update `parse_error_response` in both providers to accept `model: &str` (or take `&self`) and pass `&self.model` to `GenerationError::model_not_found(model, ...)`, matching `OpenAiImageProvider:255`.

### GEN2-02: `GoogleImagenProvider::determine_aspect_ratio` Ignores `request.params.aspect_ratio`

- **Location**: src/generation/providers/google_imagen.rs:226-258
- **Category**: Contract / Bug
- **Concrete Failure Mode**:
  - *Trigger*: Caller constructs a generation request with `request.params.aspect_ratio` set via `GenerationParams::builder().aspect_ratio("16:9").build()`.
  - *Wrong Outcome*: `determine_aspect_ratio` only checks `request.params.style` and width/height math. Because it never checks `request.params.aspect_ratio` (the canonical field on `GenerationParams`), the requested aspect ratio is completely ignored, falling back to `DEFAULT_ASPECT_RATIO` (`"1:1"`) and generating a square image instead of the requested aspect ratio.
- **Proposed Minimal Fix**:
  In `determine_aspect_ratio`, check `request.params.aspect_ratio` first before falling back to `style` and dimensions:
  ```rust
  if let Some(ar) = &request.params.aspect_ratio {
      if ASPECT_RATIOS.contains(&ar.as_str()) {
          return ar.clone();
      }
  }
  ```

### GEN2-03: Missing Trailing Slash Trim on `base_url` Produces Double-Slash Paths

- **Location**: src/generation/providers/stability.rs:180, src/generation/providers/google_imagen.rs:166
- **Category**: Network / Normalization
- **Concrete Failure Mode**:
  - *Trigger*: Operator configures `base_url` with a trailing slash (e.g. `https://api.stability.ai/` or `https://my-proxy.internal/` in `config.toml`).
  - *Wrong Outcome*: `text_to_image_url()` produces `https://...//v1/generation/...` and `predict_url()` produces `https://...//v1beta/models/...`. Reverse proxies (nginx, Cloudflare, Traefik) and strict REST servers reject double slashes with HTTP 400 Bad Request or 404 Not Found.
- **Proposed Minimal Fix**:
  Apply `.trim_end_matches('/')` in `StabilityImageProvider::new` and `GoogleImagenProvider::new` (identical to `bfl:97` and `fal:160`):
  ```rust
  let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
  let endpoint = base.trim_end_matches('/').to_string();
  ```

### GEN2-04: Hardcoded `extras: None` in `bfl/mod.rs` Drops Model Knobs and Leaves `extras` Dead

- **Location**: src/generation/providers/bfl/mod.rs:293, src/generation/providers/bfl/types.rs:49-51
- **Category**: Contract / Dead Code
- **Concrete Failure Mode**:
  - *Trigger*: Caller invokes BFL with model-specific knobs (e.g., `aspect_ratio` for FLUX.1.1 Pro Ultra, or custom parameters in `request.params.extra`).
  - *Wrong Outcome*: Line 293 hardcodes `extras: None`, discarding all non-guidance/non-safety parameters and `aspect_ratio`. BFL renders with default dimensions, ignoring caller intent. The `extras` field on `BflGenerateRequest` is proven dead code because no code path ever constructs it with `Some`.
- **Proposed Minimal Fix**:
  Collect remaining entries from `request.params.extra` (excluding `"guidance"` and `"safety_tolerance"` which are mapped to typed fields) and `request.params.aspect_ratio` into `Option<serde_json::Value>` and assign to `body.extras`.

### GEN2-05: Missing `Retry-After` Header Extraction on HTTP 429 Breaks `retry_transient` Backoff

- **Location**: src/generation/providers/openai_image.rs:246-249, 372-418
- **Category**: Retry / Correctness
- **Concrete Failure Mode**:
  - *Trigger*: OpenAI or an OpenAI-compatible proxy returns HTTP 429 Too Many Requests with header `Retry-After: 3` (or 5s).
  - *Wrong Outcome*: `parse_error_response` does not receive the response headers and hardcodes `GenerationError::rate_limit("Rate limit exceeded", None)`. `http::retry_transient` calls `retry_after_of(&e)`, finds `None`, and falls back to 250ms and 750ms backoff. The retries fire while the server's rate-limit window is still active, exhausting all 3 attempts and failing the request when waiting the requested seconds would have succeeded.
- **Proposed Minimal Fix**:
  In `OpenAiImageProvider::generate`'s `attempt` closure, extract `reqwest::header::RETRY_AFTER` if status is 429, parse it into `Duration`, and pass to `GenerationError::rate_limit`.

### GEN2-06: Redundant String Allocations in High-Frequency Fal Polling Loop

- **Location**: src/generation/providers/fal/mod.rs:267-269, 335, 483
- **Category**: Allocation / Inefficiency
- **Concrete Failure Mode**:
  - *Trigger*: A Fal generation job polls status every 1.5s–6s across a 10-minute timeout window (up to ~100-200 iterations).
  - *Inefficiency*: On every poll tick, `auth_header()` allocates a new `String` (`format!("Key {}", self.api_key)`), and `status_url()` formats and allocates a new URL `String`. Over the lifespan of the job, hundreds of identical auth and status URL strings are allocated and immediately dropped.
- **Proposed Minimal Fix**:
  Compute `status_url` once before entering the polling loop in `generate()`, and pre-calculate `auth_header` in `FalProvider` or reuse a borrowed string slice format.

### GEN2-07: Redundant `.clone()` of URLs, Base64 Strings, and Endpoint in `OpenAiImageProvider`

- **Location**: src/generation/providers/openai_image.rs:165, 426-442, 477-495
- **Category**: Allocation / Inefficiency
- **Concrete Failure Mode**:
  - *Trigger*: Every image generation request in `OpenAiImageProvider`.
  - *Inefficiency*:
    1. `generations_url(&self) -> String` clones `self.endpoint` into a new heap `String` on every request, despite `Client::post` accepting `&str`.
    2. `api_response` is an owned `ImageGenerationResponse` with `data: Vec<ImageData>`, but line 427 clones `first_image.url.clone()` and lines 482/484 clone `img.url.clone()` and `img.b64_json`, rather than consuming the owned vector with `into_iter()`. This causes large image URLs and multi-megabyte base64 strings to be duplicated on the heap right before the original response structure is dropped.
- **Proposed Minimal Fix**:
  Change `generations_url(&self) -> &str { &self.endpoint }`. Use `into_iter()` on `api_response.data` to take ownership of image data directly without cloning.

### GEN2-08: Dead `Serialize` Derive on Internal Struct `FalStatusResponse`

- **Location**: src/generation/providers/fal/mod.rs:429
- **Category**: Dead Code
- **Concrete Failure Mode**:
  - *Trigger*: Compilation and static analysis.
  - *Inefficiency*: `FalStatusResponse` is private to `fal/mod.rs` and is only ever deserialized from GET responses (`serde_json::from_str(&body)`). Deriving `Serialize` produces dead code and increases compile time.
- **Proposed Minimal Fix**:
  Change `#[derive(Debug, Deserialize, Serialize)]` to `#[derive(Debug, Deserialize)]`.

### GEN2-09: Duplicated `reqwest::Client` Inline Construction Bypassing `generation_http_client`

- **Location**: src/generation/providers/bfl/mod.rs:92-95, src/generation/providers/stability.rs:172-175, src/generation/providers/google_imagen.rs:158-161, src/generation/providers/openai_image.rs:142-145
- **Category**: Indirection / Duplication
- **Concrete Failure Mode**:
  - *Trigger*: Construction of any of the 4 image providers.
  - *Inefficiency*: `src/generation/providers/http.rs` defines `pub(crate) fn generation_http_client(timeout: Duration) -> reqwest::Result<reqwest::Client>` specifically so all image/video providers share one constructor. Instead, each of the four providers duplicates the inline `Client::builder().timeout(...).build()` call and error formatting. Any client policy improvements added to `generation_http_client` will not reach these 4 providers.
- **Proposed Minimal Fix**:
  Use `http::generation_http_client(Duration::from_secs(DEFAULT_TIMEOUT_SECS))` in `new()` across all four providers.

### GEN2-10: Inefficient Text Buffering in BFL Poll Loop and Redundant Sample URL Cloning

- **Location**: src/generation/providers/bfl/mod.rs:146, 336
- **Category**: Allocation / Inefficiency
- **Concrete Failure Mode**:
  - *Trigger*: Polling for FLUX image completion (up to 60 iterations every 2 seconds).
  - *Inefficiency*:
    1. In `poll_result`, `response.text().await` is unconditionally buffered into a new `String` on every single polling tick even when the response is HTTP 200 OK, unlike `submit()` which directly streams via `response.json().await`.
    2. At line 336, `result.sample.clone()` allocates a new clone of the signed URL from `result`, even though `result` is only referenced for scalar `Copy` fields (`seed`, `duration`) afterwards.
- **Proposed Minimal Fix**:
  In `poll_result`, stream directly via `response.json::<types::BflPollResponse>().await` on success, buffering text only on non-success status codes. In `generate`, replace `result.sample.clone()` with `result.sample.take()`.
