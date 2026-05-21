# Change: Add Contextual Memory (Local RAG)

**Status**: ✅ Validated | 📋 Ready for Review
**Change ID**: `add-contextual-memory-rag`

## Quick Overview

This OpenSpec change proposal adds a context-aware local RAG (Retrieval-Augmented Generation) memory system to Aleph, enabling it to remember past interactions and provide contextually relevant AI responses.

## 📁 Structure

```
openspec/changes/add-contextual-memory-rag/
├── proposal.md          # Full proposal with problem statement, solution, risks
├── design.md            # Detailed architecture and component design
├── tasks.md             # 27 implementation tasks with dependencies
└── specs/               # Capability specifications
    ├── memory-storage/spec.md
    ├── embedding-inference/spec.md
    ├── context-capture/spec.md
    ├── memory-augmentation/spec.md
    └── memory-privacy/spec.md
```

## 🎯 Key Features

1. **Context Anchors**: Tag memories with app_bundle_id + window_title
2. **Local Embeddings**: Use all-MiniLM-L6-v2 model (23MB, Apache 2.0)
3. **Vector Database**: SQLite + sqlite-vec for Phase 4 (migrate to LanceDB later)
4. **Privacy-First**: Zero-knowledge cloud, PII scrubbing, retention policies
5. **User Control**: Settings UI for memory management (view/delete/configure)

## 📊 Specifications

### New Capabilities (5)

| Capability | Requirements | Scenarios |
|-----------|--------------|-----------|
| **memory-storage** | 6 | 20+ |
| **embedding-inference** | 6 | 18+ |
| **context-capture** | 3 | 6+ |
| **memory-augmentation** | 2 | 3+ |
| **memory-privacy** | 3 | 6+ |

### Performance Targets

- **Embedding inference**: <100ms
- **Vector search**: <50ms
- **Total overhead**: <150ms
- **Database size**: ~1.5KB per memory

### Privacy Guarantees

- ✅ All data stored locally (`~/.aleph/memory.db`)
- ✅ PII scrubbed before storage
- ✅ Only augmented prompts sent to cloud LLMs
- ✅ User controls: view, delete, configure retention
- ✅ App exclusion list (password managers, etc.)

## 🚀 Implementation Plan

### Timeline: ~8 weeks (2 engineers)

- **Week 1-2**: Foundation (database, config, module structure)
- **Week 3-4**: Embedding integration (ONNX model, inference)
- **Week 5**: Context capture (Swift Accessibility API)
- **Week 6**: Augmentation & testing
- **Week 7**: Privacy features & UX
- **Week 8**: Documentation & finalization

### Task Count: 27 tasks

See `tasks.md` for detailed breakdown with dependencies.

## 📋 Validation Status

```bash
$ openspec validate add-contextual-memory-rag --strict
✅ Change 'add-contextual-memory-rag' is valid
```

All requirements pass strict validation:
- ✅ All requirements use SHALL/MUST
- ✅ All requirements have ≥1 scenario
- ✅ No syntax errors
- ✅ Proper spec delta format

## 🔗 Dependencies

### New Rust Crates
```toml
rusqlite = { version = "0.30", features = ["bundled"] }
ort = { version = "2.0", features = ["download-binaries"] }
tokenizers = "0.15"
```

### External Resources
- **Embedding Model**: `sentence-transformers/all-MiniLM-L6-v2` from Hugging Face
- **SQLite Extension**: `sqlite-vec` from https://github.com/asg017/sqlite-vec
- **macOS APIs**: Accessibility API for window title capture

## 📖 Next Steps

### For Reviewers
1. Read `proposal.md` for high-level overview
2. Review `design.md` for architectural details
3. Check `specs/*/spec.md` for detailed requirements
4. Validate `tasks.md` for feasibility and sequencing

### For Implementers
1. Create feature branch: `git checkout -b feature/add-contextual-memory-rag`
2. Follow tasks in `tasks.md` sequentially
3. Run tests after each task: `cargo test memory::`
4. Update progress by checking off tasks

### For Project Managers
- Estimated effort: 320 hours (~8 weeks, 2 engineers)
- Critical dependencies: None (can start immediately)
- Blocks: Phase 6 Settings UI (memory tab)
- Risk level: Medium (performance, privacy concerns mitigated)

## 🔍 Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Vector DB | SQLite + sqlite-vec | Simpler, familiar, sufficient for Phase 4 |
| Embedding Engine | ONNX Runtime | Mature, faster than pure Rust alternatives |
| Context Capture | Swift-side | Easier access to macOS APIs |
| Model | all-MiniLM-L6-v2 | Lightweight (23MB), Apache 2.0, fast inference |
| Storage Location | `~/.aleph/` | Standard macOS config directory |

## ⚠️ Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Performance overhead | High | Medium | Async ops, lazy loading, benchmarks |
| Memory bloat | Medium | Low | Retention policies, auto-cleanup |
| Context accuracy | Medium | Medium | Graceful degradation, error handling |
| Privacy concerns | High | Medium | Clear docs, prominent controls |

## 📚 References

- **OpenSpec**: See `openspec/AGENTS.md` for conventions
- **Related Changes**: Phase 5 (AI Integration), Phase 6 (Settings UI)
- **External Docs**:
  - [Sentence Transformers](https://www.sbert.net/)
  - [SQLite Vec Extension](https://github.com/asg017/sqlite-vec)
  - [ONNX Runtime](https://onnxruntime.ai/)

## 📝 Change Log

- **2025-12-24**: Initial proposal created and validated
- **Status**: Awaiting review

---

**To proceed**: Use `openspec:apply` to begin implementation after approval.
