# Memory Retrieval

> `NoteFactRetrieval` — hybrid search over notes, scoring, context assembly, tools, and audit.

## 1. Entry Points

## 2. Hybrid Search Algorithm

## 3. Bridge to Legacy Types

## 4. Scoring Pipeline

### 4.1 Stages Overview

### 4.2 `importance_weight` (and ValueEstimator)

### 4.3 `cosine_rerank`

### 4.4 `mmr_diversity`

### 4.5 `time_decay`

### 4.6 `recency_boost`

### 4.7 `length_normalization`

### 4.8 `hard_min_score`

## 5. Reranker (Optional, Not Wired)

## 6. Query Expander (Optional, Not Wired)

## 7. Embedding Provider

## 8. Context Assembly

### 8.1 `ContextComposer`

### 8.2 `ContextComptroller`

## 9. `AiMemoryRetriever`

## 10. RippleTask

## 11. Memory Tools

### 11.1 `memory_search`

### 11.2 `memory_browse`

### 11.3 `memory_explore`

### 11.4 `recall_context`

## 12. Audit and Explainability

## 13. Cortex (Independent Subsystem)

## Appendix: Retrieval Tuning Tips

## See Also
