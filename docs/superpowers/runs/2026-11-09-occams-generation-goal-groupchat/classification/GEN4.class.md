# Classification & Routing — GEN4 (Jev Tier)

| ID | Category | Route | fix_shape | risk | note |
|---|---|---|---|---|---|
| GEN4-01 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Broken CWD Traversal Guard in Deepgram STT (Symlink TOCTOU E |
| GEN4-02 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Data URL MIME Parsing Corrupted by Semicolon Parameters |
| GEN4-03 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | `VolcengineTtsProvider` Omits Model from GenerationMetadata |
| GEN4-06 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `DeepgramTtsProvider::AVAILABLE_VOICES` Is Dead Code |
| GEN4-07 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | Hardcoded `DEFAULT_TIMEOUT_SECS` in `GenerationError::timeou |
| GEN4-08 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Unchecked Float Speed Clamping Passes Through `NaN`, Crashin |
| GEN4-09 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Duplicated MIME and Audio Extension Handling with Allocation |
| GEN4-10 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant Clones in `CartesiaProvider::generate` |
| GEN4-11 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Vector Allocation in `azure_speech::locale_from_voice` |
| GEN4-12 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Unnecessary `String` Cloning in `OpenAiTtsProvider::speech_u |
| GEN4-13 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Excessive Memory Buffering in `MiniMax` & `Volcengine` TTS |
