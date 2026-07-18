# Multimodal Pipeline Probe Tests Design

**Date**: 2026-03-22
**Status**: Approved
**Scope**: Full-chain structured log probes + two-layer verification (automated + Telegram E2E)

## Overview

Add 7 structured log probes at key pipeline stages, create Rust integration tests (Layer A) for CI-runnable verification, and a Python E2E monitor (Layer B) for real Telegram validation. All probes use `target: "multimodal"` with `run_id` correlation.

## Section 1: Log Probes (7 Points)

All probes use unified format:
```rust
tracing::info!(
    target: "multimodal",
    run_id = %run_id,
    probe = "P3_download",
    field1 = %value1,
    "Human-readable message"
);
```

### Probe Definitions

| Probe | Location | Structured Fields |
|-------|----------|-------------------|
| **P1_inbound** | `telegram/mod.rs` in `convert_message()` | `channel`, `chat_id`, `attachment_count`, `mime_types` (comma-separated) |
| **P2_resolve** | `executor.rs` at RunRequest construction | `run_id`, `session_key`, `attachment_count` |
| **P3_download** | `media/cache.rs` in `resolve()` on success | `run_id`, `attachment_id`, `mime_type`, `size_bytes`, `source` (data/path/url) |
| **P4_process** | `media/processor.rs` after each attachment | `run_id`, `attachment_id`, `media_type` (image/audio/other), `action` (native/vision_fallback/transcribe/placeholder/error_fallback) |
| **P5_inject** | `run_loop.rs` after building multimodal UnifiedMessage | `run_id`, `content_blocks` (count), `has_images` (bool), `has_transcripts` (bool) |
| **P6_provider** | `openai.rs` in convert_messages User arm | `role`, `content_type` (text/multimodal), `image_count` |
| **P7_reaction** | `reply_emitter.rs` in react_on_inbound | `run_id`, `emoji`, `message_id` |

### Probe Placement Details

**P1_inbound** — In `convert_message()`, after attachments are extracted but before returning `InboundMessage`. Only emitted when `!attachments.is_empty()`. No `run_id` at this point (not yet assigned); use `message_id` instead.

**P2_resolve** — After `RunRequest` is built in executor. This is the first point with `run_id`.

**P3_download** — Inside `MediaCache::resolve()`. Requires passing `run_id` as parameter (currently not available). Alternative: log without `run_id` at cache level, add `run_id` correlation at `MediaProcessor` level instead. **Decision**: P3 is emitted from `MediaProcessor.process_image/process_audio` after calling `cache.resolve()`, not inside cache.rs. This keeps cache.rs independent and gives us `run_id` context.

**P4_process** — Same location as P3 (MediaProcessor), after the final ContentBlock is determined.

**P5_inject** — In `run_loop.rs`, after `media_processor.process()` returns and the multimodal UnifiedMessage is built.

**P6_provider** — In `openai.rs` `convert_messages()`, when the `has_images` branch is taken. Also add to `anthropic.rs` for completeness.

**P7_reaction** — Already in `react_on_inbound()`, just need to add the structured probe.

### run_id Propagation

`run_id` is available from RunRequest. Propagation chain:
- P1: no run_id (channel layer, before assignment)
- P2-P7: run_id available via RunRequest or execution context
- MediaProcessor needs run_id passed to `process()` — add as parameter

### Files Modified

| File | Probes Added |
|------|-------------|
| `gateway/interfaces/telegram/mod.rs` | P1 |
| `gateway/inbound_router/executor.rs` | P2 |
| `media/processor.rs` | P3, P4 |
| `gateway/execution_engine/run_loop.rs` | P5 |
| `providers/protocols/openai.rs` | P6 |
| `providers/protocols/anthropic.rs` | P6 (Anthropic variant) |
| `gateway/reply_emitter.rs` | P7 |

## Section 2: Layer A — Rust Integration Tests

**File**: `tests/multimodal_probe/mod.rs` (new)

### Test Scenarios

**Test 1: Image native injection** (`supports_vision=true`)
- Input: `Attachment { mime_type: "image/jpeg", data: Some(JPEG_BYTES) }`
- Expect: `ContentBlock::Image { data: base64, mime_type: "image/jpeg" }`
- Probes: P3(source=data) → P4(action=native)

**Test 2: Image vision fallback** (`supports_vision=false`, mock VisionPipeline)
- Input: `Attachment { mime_type: "image/png", data: Some(PNG_BYTES) }`
- Expect: `ContentBlock::Text { text: "[Image: ...]" }`
- Probes: P3 → P4(action=vision_fallback)

**Test 3: Audio transcription** (mock TranscriptionService)
- Input: `Attachment { mime_type: "audio/ogg", data: Some(OGG_BYTES) }`
- Mock returns: `TranscriptionResult { text: "Hello world", language: None }`
- Expect: `ContentBlock::Text` containing "Hello world"
- Probes: P3 → P4(action=transcribe)

**Test 4: Unknown type placeholder**
- Input: `Attachment { mime_type: "application/pdf", filename: Some("doc.pdf") }`
- Expect: `ContentBlock::Text { text: "[Attachment: doc.pdf (application/pdf)]" }`
- Probes: P4(action=placeholder)

**Test 5: Mixed message (image + audio)**
- Input: `[image_attachment, audio_attachment]`
- Expect: `[ContentBlock::Image, ContentBlock::Text(transcript)]`
- Probes: P3×2 → P4×2

**Test 6: Download failure graceful degradation**
- Input: `Attachment { url: Some("http://invalid.test/img.png"), data: None, path: None }`
- Expect: `ContentBlock::Text { text: "[Image: processing failed]" }`
- Probes: P4(action=error_fallback)

**Test 7: OpenAI adapter multimodal serialization**
- Input: `UnifiedMessage::User { content: [Text("hi"), Image(b64, "image/jpeg")] }`
- Expect: Serialized JSON contains `"type":"image_url"` and `data:image/jpeg;base64,...`

### Log Capture in Tests

Use `tracing_subscriber::fmt::TestWriter` for in-test log capture:

```rust
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt;

fn setup_test_tracing() -> Arc<Mutex<Vec<u8>>> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = buffer.clone();
    let subscriber = fmt::Subscriber::builder()
        .with_writer(move || writer.clone())
        .with_target(true)
        .with_env_filter("multimodal=info")
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    buffer
}

fn assert_probe_in_logs(logs: &str, probe: &str) {
    assert!(logs.contains(probe), "Probe {} not found in logs", probe);
}
```

### Mock Infrastructure

```rust
/// Mock transcription that returns fixed text
struct MockTranscription(String);

#[async_trait]
impl TranscriptionService for MockTranscription {
    async fn transcribe(&self, _audio: &CachedMedia) -> anyhow::Result<TranscriptionResult> {
        Ok(TranscriptionResult { text: self.0.clone(), language: None })
    }
}
```

No mock needed for VisionPipeline in Test 1 (vision=true skips it). Test 2 needs either a mock or the actual VisionPipeline with a mock provider — follow existing test patterns.

### Test Data

Use minimal valid binary data:
- JPEG: `[0xFF, 0xD8, 0xFF, 0xE0]` (JPEG magic bytes + padding)
- PNG: `[0x89, 0x50, 0x4E, 0x47]` (PNG magic bytes + padding)
- OGG: `[0x4F, 0x67, 0x67, 0x53]` (OGG magic bytes + padding)

These are not valid images/audio but are sufficient for testing the pipeline (we're not decoding them, just passing through to base64).

## Section 3: Layer B — Telegram E2E Monitor

**File**: `e2e_tests/multimodal_e2e.py` (new)

### Architecture

```
Python script (monitor mode)
    ├─ tail ~/.aleph/logs/aleph-server.log.*
    ├─ Parse multimodal probes in real-time
    ├─ Prompt human to perform actions in Telegram
    ├─ Wait for probe sequence per scene
    └─ Print pass/fail report
```

### 6 Test Scenes

| Scene | Telegram Action | Expected Probe Sequence | Success Criteria |
|-------|----------------|------------------------|-----------------|
| 1. Image Understanding | Send photo + "这是哪里？" | P1→P3→P4(native)→P5(has_images)→P6(multimodal)→P7(👍) | Bot describes image |
| 2. Voice Transcription | Send voice message "今天天气怎么样" | P1(audio/ogg)→P3→P4(transcribe)→P5(has_transcripts)→P7(👍) | Bot responds to spoken content |
| 3. Sticker | Send emoji sticker | P1(image/webp)→P3→P4→P5→P7 | Bot acknowledges sticker |
| 4. No-text Image | Send photo without caption | P1(attachment_count=1)→P3→P4→P5→P7 | Bot describes image (not "empty message") |
| 5. Image+Text Mix | Photo + "翻译图中文字" | P1→P3→P4(native)→P5(content_blocks=2)→P6(image_count=1)→P7(👍) | Bot translates text in image |
| 6. Reaction Lifecycle | Send any text | P7(👀)→...→P7(👍) | 👀 appears then changes to 👍 |

### ProbeCollector Class

```python
import re
import time
from dataclasses import dataclass
from typing import List, Optional, Dict

@dataclass
class Probe:
    timestamp: str
    name: str        # "P3_download"
    fields: Dict[str, str]
    raw_line: str

class ProbeCollector:
    PROBE_RE = re.compile(r'multimodal:\s+(P\d+_\w+)\s+(.*)')
    FIELD_RE = re.compile(r'(\w+)=(".*?"|\S+)')

    def __init__(self, log_path: str):
        self.log_path = log_path
        self.probes: List[Probe] = []

    def collect_from_stream(self, timeout: int = 60) -> List[Probe]:
        """Tail log file, collect probes until timeout or P7_reaction(👍/👎)"""

    def filter_by_run(self, run_id: str) -> List[Probe]:
        """Filter probes by run_id"""

    def assert_sequence(self, expected: List[str]) -> bool:
        """Verify probes appear in order (not necessarily consecutive)"""

    def assert_fields(self, probe_name: str, **expected) -> bool:
        """Verify specific probe has expected field values"""

    def print_timeline(self):
        """Print chronological probe timeline with timing"""

    def print_report(self, scene_name: str, expected_sequence: List[str]):
        """Print pass/fail report for a scene"""
```

### Run Instructions

```bash
# Terminal 1: Start server with multimodal logging
RUST_LOG=info,multimodal=info cargo run --bin aleph-server start

# Terminal 2: Start E2E monitor
python e2e_tests/multimodal_e2e.py

# Follow prompts in Terminal 2, send messages in Telegram
```

### Script Flow

```python
async def main():
    collector = ProbeCollector(find_latest_log())

    scenes = [
        Scene("Image Understanding", "发送一张风景照片 + caption '这是哪里？'",
              ["P1_inbound", "P3_download", "P4_process", "P5_inject", "P6_provider", "P7_reaction"]),
        Scene("Voice Transcription", "发送语音消息: '今天天气怎么样'",
              ["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"]),
        # ... scenes 3-6
    ]

    for scene in scenes:
        print(f"\n{'='*50}")
        print(f"Scene: {scene.name}")
        print(f"Action: {scene.instruction}")
        input("Press ENTER after performing the action in Telegram...")

        probes = collector.collect_from_stream(timeout=60)
        passed = collector.assert_sequence(scene.expected_probes)
        collector.print_report(scene.name, scene.expected_probes)

    print_final_summary(scenes)
```

## Section 4: Probe Report Format

### Timeline Output (Debug)

```
=== Run abc-123 Timeline ===
+0ms     P1_inbound     channel=telegram chat_id=12345 attachment_count=1 mime_types=image/jpeg
+15ms    P2_resolve     session_key=tg:12345 attachment_count=1
+135ms   P3_download    attachment_id=AgAC... mime_type=image/jpeg size_bytes=245760 source=url
+140ms   P4_process     media_type=image action=native attachment_id=AgAC...
+145ms   P5_inject      content_blocks=2 has_images=true has_transcripts=false
+150ms   P6_provider    content_type=multimodal image_count=1
+3200ms  P7_reaction    emoji=👍 message_id=456
```

### Summary Report

```
=== Multimodal Pipeline Verification Report ===
Date: 2026-03-22 15:30:00

Scene 1: Image Understanding     PASS  (3.2s)
Scene 2: Voice Transcription     PASS  (5.1s)
Scene 3: Sticker                 PASS  (2.8s)
Scene 4: No-text Image           PASS  (3.0s)
Scene 5: Image+Text Mix          PASS  (4.2s)
Scene 6: Reaction Lifecycle      PASS  (2.5s)

Result: 6/6 PASS
```

## Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `gateway/interfaces/telegram/mod.rs` | Modify | P1 probe |
| `gateway/inbound_router/executor.rs` | Modify | P2 probe |
| `media/processor.rs` | Modify | P3, P4 probes (+ add `run_id` param to `process()`) |
| `gateway/execution_engine/run_loop.rs` | Modify | P5 probe |
| `providers/protocols/openai.rs` | Modify | P6 probe |
| `providers/protocols/anthropic.rs` | Modify | P6 probe (Anthropic variant) |
| `gateway/reply_emitter.rs` | Modify | P7 probe |
| `tests/multimodal_probe/mod.rs` | Create | Layer A integration tests (7 tests) |
| `e2e_tests/multimodal_e2e.py` | Create | Layer B Telegram E2E monitor |

## Not In Scope

- Performance benchmarking (timing is logged but not asserted)
- LLM response quality validation (non-deterministic)
- Automated Telegram message sending (too complex, manual is fine)
- Load testing / concurrent multimodal requests
- Video processing tests (not implemented yet)
