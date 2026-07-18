# Multimodal Probe Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 7 structured log probes across the multimodal pipeline, Rust integration tests (Layer A), and a Python E2E monitor (Layer B) for production verification.

**Architecture:** First add probes to existing code (non-breaking, log-only changes). Then add integration tests that exercise MediaProcessor and verify probe output. Finally create the Python E2E script. Tasks are mostly independent — probes can be added in any order.

**Tech Stack:** Rust (tracing), Python (asyncio, websockets for E2E)

**Spec:** `docs/superpowers/specs/2026-03-22-multimodal-probe-tests-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/gateway/interfaces/telegram/mod.rs` | Modify | P1_inbound probe |
| `src/gateway/inbound_router/executor.rs` | Modify | P2_resolve probe |
| `src/media/processor.rs` | Modify | P3_download + P4_process probes, add `run_id` param |
| `src/gateway/execution_engine/run_loop.rs` | Modify | P5_inject probe, pass `run_id` to processor |
| `src/providers/protocols/openai.rs` | Modify | P6_provider probe |
| `src/providers/protocols/anthropic.rs` | Modify | P6_provider probe (Anthropic) |
| `src/gateway/reply_emitter.rs` | Modify | P7_reaction probe |
| `tests/multimodal_probe.rs` | Create | Layer A integration tests |
| `e2e_tests/multimodal_e2e.py` | Create | Layer B Telegram E2E monitor |

---

## Task 1: Add probes P1-P2 (Channel + RunRequest)

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/inbound_router/executor.rs`

- [ ] **Step 1: Add P1_inbound probe to Telegram convert_message**

In `telegram/mod.rs`, in `convert_message()`, after attachments are extracted and before returning the `InboundMessage`, add (only when attachments exist):

```rust
if !attachments.is_empty() {
    let mime_types: Vec<&str> = attachments.iter().map(|a| a.mime_type.as_str()).collect();
    tracing::info!(
        target: "multimodal",
        probe = "P1_inbound",
        channel = "telegram",
        chat_id = %msg.chat.id.0,
        message_id = %msg.id.0,
        attachment_count = attachments.len(),
        mime_types = %mime_types.join(","),
        "Inbound message with attachments"
    );
}
```

- [ ] **Step 2: Add P2_resolve probe to executor.rs**

In `executor.rs`, right after `RunRequest` is constructed, add (only when attachments exist):

```rust
if !request.attachments.is_empty() {
    tracing::info!(
        target: "multimodal",
        probe = "P2_resolve",
        run_id = %request.run_id,
        session_key = %request.session_key,
        attachment_count = request.attachments.len(),
        "RunRequest created with attachments"
    );
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "probes: add P1_inbound and P2_resolve multimodal log probes"
```

---

## Task 2: Add probes P3-P4 (MediaProcessor) + run_id propagation

**Files:**
- Modify: `src/media/processor.rs` (add `run_id` param, add P3+P4 probes)
- Modify: `src/gateway/execution_engine/run_loop.rs` (pass `run_id` to processor)

- [ ] **Step 1: Add `run_id` parameter to MediaProcessor::process()**

Change the signature:
```rust
pub async fn process(
    &self,
    attachments: &[Attachment],
    supports_vision: bool,
    session_id: &str,
    run_id: &str,  // NEW
) -> Vec<ContentBlock> {
```

Propagate `run_id` to `process_one()`, `process_image()`, `process_audio()` — add `run_id: &str` to each.

- [ ] **Step 2: Add P3_download probe after cache.resolve()**

In `process_image()` and `process_audio()`, after the `self.cache.resolve()` call succeeds:

```rust
tracing::info!(
    target: "multimodal",
    probe = "P3_download",
    run_id = %run_id,
    attachment_id = %attachment.id,
    mime_type = %cached.mime_type,
    size_bytes = cached.size,
    source = if attachment.data.is_some() { "data" } else if attachment.path.is_some() { "path" } else { "url" },
    "Media attachment resolved"
);
```

- [ ] **Step 3: Add P4_process probe after ContentBlock is determined**

In `process_one()`, after each branch returns the final `ContentBlock`:

For image native:
```rust
tracing::info!(
    target: "multimodal",
    probe = "P4_process",
    run_id = %run_id,
    attachment_id = %attachment.id,
    media_type = "image",
    action = "native",
    "Attachment processed"
);
```

For image vision fallback: `action = "vision_fallback"`
For audio transcribe: `action = "transcribe"`
For placeholder: `action = "placeholder"`
For error: `action = "error_fallback"`

The simplest approach: add the probe in `process_one()` after matching the result, or add it in each specific handler method just before returning.

- [ ] **Step 4: Update run_loop.rs to pass run_id**

In `run_loop.rs`, where `media_processor.process()` is called, add `&request.run_id`:

```rust
let media_blocks = media_processor
    .process(&request.attachments, supports_vision, &session_id, &request.run_id)
    .await;
```

- [ ] **Step 5: Fix any tests that call process() with old signature**

Run `cargo check -p alephcore` and fix tests in `processor.rs` that call `process()` — add `"test-run-id"` as the new parameter.

- [ ] **Step 6: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib media`

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "probes: add P3_download and P4_process probes with run_id propagation"
```

---

## Task 3: Add probes P5-P6 (Inject + Provider)

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs` (P5)
- Modify: `src/providers/protocols/openai.rs` (P6)
- Modify: `src/providers/protocols/anthropic.rs` (P6)

- [ ] **Step 1: Add P5_inject probe in run_loop.rs**

After the multimodal `UnifiedMessage` is built (after `content.extend(media_blocks)`):

```rust
let has_images = content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));
let has_transcripts = content.iter().any(|b| {
    if let ContentBlock::Text { text } = b {
        text.starts_with("[Voice message transcript]")
    } else {
        false
    }
});
tracing::info!(
    target: "multimodal",
    probe = "P5_inject",
    run_id = %request.run_id,
    content_blocks = content.len(),
    has_images = has_images,
    has_transcripts = has_transcripts,
    "Multimodal UnifiedMessage built"
);
```

- [ ] **Step 2: Add P6_provider probe in openai.rs**

In `convert_messages()`, in the `has_images` branch (where `MessageContent::Multimodal` is built):

```rust
let image_count = blocks.iter().filter(|b| matches!(b, OaiBlock::ImageUrl { .. })).count();
tracing::info!(
    target: "multimodal",
    probe = "P6_provider",
    role = "user",
    content_type = "multimodal",
    image_count = image_count,
    "OpenAI multimodal message converted"
);
```

- [ ] **Step 3: Add P6_provider probe in anthropic.rs**

In the Anthropic `convert_messages()`, in the Image handling branch:

```rust
tracing::info!(
    target: "multimodal",
    probe = "P6_provider",
    role = "user",
    content_type = "multimodal",
    image_count = /* count Image blocks */,
    "Anthropic multimodal message converted"
);
```

Read the actual anthropic.rs code first to find the exact location where Image blocks are converted.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "probes: add P5_inject and P6_provider multimodal probes"
```

---

## Task 4: Add probe P7 (Reaction)

**Files:**
- Modify: `src/gateway/reply_emitter.rs`

- [ ] **Step 1: Add P7_reaction probe in react_on_inbound**

In `react_on_inbound()` (line ~440), add the probe:

```rust
async fn react_on_inbound(&self, emoji: &str) {
    if let Some(ref msg_id) = self.route.inbound_message_id {
        tracing::info!(
            target: "multimodal",
            probe = "P7_reaction",
            run_id = %self.run_id,
            emoji = %emoji,
            message_id = %msg_id.as_str(),
            "Processing status reaction"
        );
        let _ = self.channel_registry.react(
            &self.route.channel_id,
            &self.route.conversation_id,
            msg_id,
            emoji,
        ).await;
    }
}
```

- [ ] **Step 2: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib reply_emitter`

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "probes: add P7_reaction multimodal probe"
```

---

## Task 5: Layer A — Rust integration tests

**Files:**
- Create: `tests/multimodal_probe.rs`

- [ ] **Step 1: Create integration test file**

```rust
//! Multimodal pipeline integration tests.
//!
//! Tests MediaProcessor with mock attachments and verifies ContentBlock output.
//! These tests exercise the actual processing logic without external dependencies.

use alephcore::gateway::channel::Attachment;
use alephcore::media::processor::MediaProcessor;
use alephcore::media::transcription::{TranscriptionResult, TranscriptionService};
use alephcore::media::cache::CachedMedia;
use alephcore::providers::message::ContentBlock;
use async_trait::async_trait;

/// Mock transcription service that returns fixed text
struct MockTranscription(String);

#[async_trait]
impl TranscriptionService for MockTranscription {
    async fn transcribe(&self, _audio: &CachedMedia) -> anyhow::Result<TranscriptionResult> {
        Ok(TranscriptionResult {
            text: self.0.clone(),
            language: Some("en".to_string()),
        })
    }
}

fn make_attachment(id: &str, mime: &str, data: Vec<u8>) -> Attachment {
    Attachment {
        id: id.to_string(),
        mime_type: mime.to_string(),
        filename: Some(format!("{}.test", id)),
        size: Some(data.len() as u64),
        url: None,
        path: None,
        data: Some(data),
    }
}

// Minimal valid-ish bytes for testing (not real images, just for pipeline)
const FAKE_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
const FAKE_PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A];
const FAKE_OGG: &[u8] = &[0x4F, 0x67, 0x67, 0x53, 0x00, 0x02];

#[tokio::test]
async fn test_image_native_injection() {
    let processor = MediaProcessor::new(None, None);
    let attachment = make_attachment("img1", "image/jpeg", FAKE_JPEG.to_vec());

    let blocks = processor.process(&[attachment], true, "test-native", "run-001").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Image { data, mime_type } => {
            assert_eq!(mime_type, "image/jpeg");
            assert!(!data.is_empty(), "base64 data should not be empty");
        }
        other => panic!("Expected Image block, got {:?}", other),
    }
    processor.cleanup("test-native");
}

#[tokio::test]
async fn test_image_vision_fallback() {
    // No VisionPipeline provided → falls back to "vision not configured"
    let processor = MediaProcessor::new(None, None);
    let attachment = make_attachment("img2", "image/png", FAKE_PNG.to_vec());

    let blocks = processor.process(&[attachment], false, "test-fallback", "run-002").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text } => {
            assert!(
                text.contains("Image") || text.contains("image"),
                "Should contain image fallback text, got: {}",
                text
            );
        }
        other => panic!("Expected Text block for vision fallback, got {:?}", other),
    }
    processor.cleanup("test-fallback");
}

#[tokio::test]
async fn test_audio_transcription() {
    let mock_stt = Box::new(MockTranscription("Hello world".to_string()));
    let processor = MediaProcessor::new(Some(mock_stt), None);
    let attachment = make_attachment("aud1", "audio/ogg", FAKE_OGG.to_vec());

    let blocks = processor.process(&[attachment], true, "test-stt", "run-003").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text } => {
            assert!(
                text.contains("Hello world"),
                "Transcript should contain 'Hello world', got: {}",
                text
            );
        }
        other => panic!("Expected Text block with transcript, got {:?}", other),
    }
    processor.cleanup("test-stt");
}

#[tokio::test]
async fn test_unknown_type_placeholder() {
    let processor = MediaProcessor::new(None, None);
    let mut attachment = make_attachment("doc1", "application/pdf", vec![0x25, 0x50, 0x44, 0x46]);
    attachment.filename = Some("report.pdf".to_string());

    let blocks = processor.process(&[attachment], true, "test-unknown", "run-004").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text } => {
            assert!(text.contains("report.pdf"), "Should contain filename, got: {}", text);
            assert!(text.contains("application/pdf"), "Should contain mime, got: {}", text);
        }
        other => panic!("Expected Text placeholder, got {:?}", other),
    }
    processor.cleanup("test-unknown");
}

#[tokio::test]
async fn test_mixed_image_and_audio() {
    let mock_stt = Box::new(MockTranscription("Transcribed audio".to_string()));
    let processor = MediaProcessor::new(Some(mock_stt), None);

    let image = make_attachment("mix_img", "image/jpeg", FAKE_JPEG.to_vec());
    let audio = make_attachment("mix_aud", "audio/ogg", FAKE_OGG.to_vec());

    let blocks = processor.process(&[image, audio], true, "test-mixed", "run-005").await;

    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], ContentBlock::Image { .. }), "First block should be Image");
    assert!(matches!(&blocks[1], ContentBlock::Text { .. }), "Second block should be Text (transcript)");
    processor.cleanup("test-mixed");
}

#[tokio::test]
async fn test_download_failure_graceful() {
    let processor = MediaProcessor::new(None, None);
    let attachment = Attachment {
        id: "fail1".to_string(),
        mime_type: "image/jpeg".to_string(),
        filename: None,
        size: None,
        url: Some("http://invalid.test.invalid/img.jpg".to_string()),
        path: None,
        data: None,
    };

    let blocks = processor.process(&[attachment], true, "test-fail", "run-006").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text } => {
            assert!(
                text.to_lowercase().contains("fail") || text.to_lowercase().contains("error") || text.to_lowercase().contains("unavailable"),
                "Should contain error fallback text, got: {}",
                text
            );
        }
        other => panic!("Expected Text error fallback, got {:?}", other),
    }
    processor.cleanup("test-fail");
}

#[tokio::test]
async fn test_openai_multimodal_serialization() {
    use alephcore::providers::message::UnifiedMessage;

    let msg = UnifiedMessage::user_with_content(vec![
        ContentBlock::Text { text: "What is this?".to_string() },
        ContentBlock::Image {
            data: "dGVzdA==".to_string(), // base64 of "test"
            mime_type: "image/jpeg".to_string(),
        },
    ]);

    // Verify the structure is correct
    if let UnifiedMessage::User { content } = &msg {
        assert_eq!(content.len(), 2);
        assert!(matches!(&content[0], ContentBlock::Text { .. }));
        assert!(matches!(&content[1], ContentBlock::Image { .. }));
    } else {
        panic!("Expected User message");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --test multimodal_probe`
Expected: ALL PASS (adjust import paths if needed — `alephcore` may be `aleph_core` or similar)

- [ ] **Step 3: Commit**

```bash
git add tests/multimodal_probe.rs && git commit -m "tests: add Layer A multimodal pipeline integration tests"
```

---

## Task 6: Layer B — Python E2E monitor

**Files:**
- Create: `e2e_tests/multimodal_e2e.py`

- [ ] **Step 1: Create the E2E monitor script**

```python
#!/usr/bin/env python3
"""
Multimodal Pipeline E2E Verification Monitor

Monitors Aleph server logs for multimodal probe events while a human
performs test actions in Telegram. Validates probe sequences per scene.

Usage:
    # Terminal 1: Start server
    RUST_LOG=info,multimodal=info cargo run --bin aleph-server start

    # Terminal 2: Start monitor
    python e2e_tests/multimodal_e2e.py

    # Follow prompts, perform actions in Telegram
"""

import re
import sys
import time
import glob
import os
from dataclasses import dataclass, field
from typing import List, Dict, Optional
from pathlib import Path
from datetime import datetime

# ──────────────────────────────────────────────────────────────
# Probe parsing
# ──────────────────────────────────────────────────────────────

@dataclass
class Probe:
    timestamp: str
    name: str
    fields: Dict[str, str]
    raw_line: str

PROBE_RE = re.compile(r'(P\d+_\w+)')
FIELD_RE = re.compile(r'(\w+)=(".*?"|\S+)')

def parse_probe(line: str) -> Optional[Probe]:
    """Parse a log line into a Probe if it contains a multimodal probe."""
    if 'multimodal' not in line:
        return None
    m = PROBE_RE.search(line)
    if not m:
        return None
    name = m.group(1)
    fields = {}
    for fm in FIELD_RE.finditer(line):
        key = fm.group(1)
        val = fm.group(2).strip('"')
        fields[key] = val
    # Extract timestamp (first token)
    ts = line.split()[0] if line.strip() else ""
    return Probe(timestamp=ts, name=name, fields=fields, raw_line=line.strip())

# ──────────────────────────────────────────────────────────────
# Probe collector
# ──────────────────────────────────────────────────────────────

class ProbeCollector:
    def __init__(self, log_path: str):
        self.log_path = log_path
        self.probes: List[Probe] = []
        self._file_pos = 0

    def _seek_to_end(self):
        """Position file cursor at end (for tail-like behavior)."""
        if os.path.exists(self.log_path):
            self._file_pos = os.path.getsize(self.log_path)

    def collect_until(self, timeout: int = 60, stop_probe: str = "P7_reaction") -> List[Probe]:
        """Tail log file, collect probes until timeout or stop_probe with success emoji."""
        self._seek_to_end()
        self.probes = []
        start = time.time()

        while time.time() - start < timeout:
            try:
                with open(self.log_path, 'r', errors='replace') as f:
                    f.seek(self._file_pos)
                    new_lines = f.readlines()
                    self._file_pos = f.tell()

                for line in new_lines:
                    probe = parse_probe(line)
                    if probe:
                        self.probes.append(probe)
                        print(f"  📡 {probe.name:20s} {' '.join(f'{k}={v}' for k,v in probe.fields.items() if k != 'probe')}")
                        # Stop if we see completion reaction
                        if probe.name == stop_probe and probe.fields.get('emoji') in ('👍', '👎'):
                            return self.probes
            except FileNotFoundError:
                pass
            time.sleep(0.3)

        return self.probes

    def assert_sequence(self, expected: List[str]) -> tuple:
        """Check probes appear in order. Returns (passed, details)."""
        found = [p.name for p in self.probes]
        missing = []
        idx = 0
        for exp in expected:
            while idx < len(found) and found[idx] != exp:
                idx += 1
            if idx >= len(found):
                missing.append(exp)
            else:
                idx += 1
        passed = len(missing) == 0
        return passed, missing

    def assert_fields(self, probe_name: str, **expected) -> tuple:
        """Check a probe has expected field values."""
        for p in self.probes:
            if p.name == probe_name:
                mismatches = {}
                for k, v in expected.items():
                    actual = p.fields.get(k)
                    if actual != str(v):
                        mismatches[k] = f"expected={v}, actual={actual}"
                return len(mismatches) == 0, mismatches
        return False, {"error": f"Probe {probe_name} not found"}

# ──────────────────────────────────────────────────────────────
# Test scenes
# ──────────────────────────────────────────────────────────────

@dataclass
class Scene:
    name: str
    instruction: str
    expected_probes: List[str]
    field_checks: Dict[str, Dict] = field(default_factory=dict)

SCENES = [
    Scene(
        name="Image Understanding",
        instruction='发送一张风景照片，caption: "这是哪里？"',
        expected_probes=["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"],
        field_checks={"P4_process": {"action": "native"}, "P5_inject": {"has_images": "true"}},
    ),
    Scene(
        name="Voice Transcription",
        instruction='发送一段语音消息: "今天天气怎么样"',
        expected_probes=["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"],
        field_checks={"P4_process": {"action": "transcribe"}, "P5_inject": {"has_transcripts": "true"}},
    ),
    Scene(
        name="Sticker",
        instruction="发送一个表情贴纸",
        expected_probes=["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"],
        field_checks={"P4_process": {"media_type": "image"}},
    ),
    Scene(
        name="No-text Image",
        instruction="发送一张照片，不写 caption",
        expected_probes=["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"],
    ),
    Scene(
        name="Image+Text Mix",
        instruction='发送照片 + caption: "帮我翻译图片中的文字"',
        expected_probes=["P1_inbound", "P3_download", "P4_process", "P5_inject", "P7_reaction"],
        field_checks={"P5_inject": {"has_images": "true"}},
    ),
    Scene(
        name="Reaction Lifecycle",
        instruction="发送任意文本消息（如 '你好'），观察消息上的 reaction 变化",
        expected_probes=["P7_reaction"],
        field_checks={},
    ),
]

# ──────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────

def find_latest_log() -> str:
    """Find the latest aleph-server log file."""
    log_dir = Path.home() / ".aleph" / "logs"
    pattern = str(log_dir / "aleph-server.log.*")
    files = sorted(glob.glob(pattern), key=os.path.getmtime, reverse=True)
    if not files:
        # Try gateway.log as alternative
        alt = str(log_dir / "gateway.log")
        if os.path.exists(alt):
            return alt
        print(f"❌ No log files found in {log_dir}")
        sys.exit(1)
    return files[0]

def print_report(results: List[tuple]):
    """Print final summary."""
    print("\n" + "=" * 60)
    print("=== Multimodal Pipeline Verification Report ===")
    print(f"Date: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 60)

    passed = 0
    total = len(results)
    for name, ok, details in results:
        status = "✅ PASS" if ok else "❌ FAIL"
        print(f"  {name:30s} {status}")
        if not ok and details:
            for d in details:
                print(f"    ↳ {d}")
        if ok:
            passed += 1

    print(f"\nResult: {passed}/{total} {'PASS' if passed == total else 'FAIL'}")
    print("=" * 60)

def main():
    log_path = find_latest_log()
    print(f"📋 Multimodal E2E Test Suite")
    print(f"📁 Monitoring: {log_path}")
    print(f"⏱  Timeout per scene: 60s")
    print()

    collector = ProbeCollector(log_path)
    results = []

    for i, scene in enumerate(SCENES, 1):
        print(f"\n{'─' * 60}")
        print(f"Scene {i}/{len(SCENES)}: {scene.name}")
        print(f"Action: {scene.instruction}")
        print(f"Expected: {' → '.join(scene.expected_probes)}")
        input("\n  按 ENTER 开始监控（先在 Telegram 执行操作）...")

        print(f"\n  ⏳ Waiting for probes (60s timeout)...")
        collector.collect_until(timeout=60)

        # Check sequence
        seq_ok, missing = collector.assert_sequence(scene.expected_probes)

        # Check fields
        field_issues = []
        for probe_name, expected_fields in scene.field_checks.items():
            fok, mismatches = collector.assert_fields(probe_name, **expected_fields)
            if not fok:
                field_issues.append(f"{probe_name}: {mismatches}")

        ok = seq_ok and len(field_issues) == 0
        details = []
        if missing:
            details.append(f"Missing probes: {missing}")
        details.extend(field_issues)

        results.append((scene.name, ok, details))
        print(f"\n  → {'✅ PASS' if ok else '❌ FAIL'}")

    print_report(results)

if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Make executable**

```bash
chmod +x e2e_tests/multimodal_e2e.py
```

- [ ] **Step 3: Commit**

```bash
git add e2e_tests/multimodal_e2e.py && git commit -m "e2e: add Layer B multimodal Telegram E2E monitor"
```

---

## Task 7: Final verification

- [ ] **Step 1: Run all Rust tests**

Run: `cargo test -p alephcore --lib && cargo test -p alephcore --test multimodal_probe`
Expected: ALL PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore`
Expected: No new warnings from probe code

- [ ] **Step 3: Commit if needed**

```bash
git add -A && git commit -m "probes: final cleanup for multimodal verification"
```
