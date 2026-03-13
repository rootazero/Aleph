# Agent Design Philosophy

> *"The mind is not a vessel to be filled, but a fire to be kindled."* — Plutarch

This document outlines the core design principles that guide Aleph's Agent architecture.

---

## 1. LLM Sovereignty (LLM 主权原则)

**Core Idea**: Let the LLM do what it's good at; the system only provides what the LLM cannot do alone.

Aleph follows a strict **Think → Act** loop where the LLM handles all reasoning — intent understanding, tool selection, safety assessment, and completion judgment — in a single inference call. The system provides capabilities (tools, memory, channels) but never substitutes deterministic code for LLM reasoning.

```
❌ Over-engineering: User Request → Intent Engine → Tool Filter → POE Manager → Evaluate → ...
✅ Aleph:          User Request → LLM Thinks → LLM Acts → Done
```

**Why it matters**: Every middleware layer between the user and the LLM is a tax on intelligence. Removing orchestration overhead releases the model's full reasoning capability.

---

## 2. Heuristic Thinking (启发式思考)

**Core Idea**: Combine fast intuition with deep reasoning.

Inspired by Daniel Kahneman's dual-process theory, Aleph implements a **System 1 + System 2** cognitive architecture:

| System | Characteristics | Role in Aleph |
|--------|-----------------|----------------|
| **System 1** | Fast, intuitive, experience-driven | Heuristic rules, experience retrieval, pattern matching |
| **System 2** | Slow, logical, deliberate | LLM reasoning, semantic validation, contract generation |

**How it works**:
- System 1 provides quick "gut feelings" based on accumulated experience
- System 2 handles complex reasoning when intuition isn't enough
- The two systems collaborate, with System 1 guiding initial direction and System 2 verifying correctness

This mirrors how expert humans solve problems: experienced developers don't analyze every line from scratch — they recognize patterns instantly, then apply careful reasoning where needed.

---

## 3. Memory-Augmented Context (记忆增强上下文)

**Core Idea**: Persistent memory gives the LLM long-term continuity.

The LLM is powerful but stateless. Aleph's memory system bridges this gap by providing relevant context from past interactions:

- **Hybrid retrieval** — vector ANN + full-text search surfaces the most relevant facts
- **Tiered storage** — ephemeral, short-term, long-term, archive with automatic decay
- **Compression** — background consolidation distills accumulated experiences into concise insights

This transforms Aleph from a stateless chatbot into a contextually aware assistant that remembers what matters.

---

## Putting It All Together

These three principles work in concert:

1. **LLM Sovereignty** ensures the model's reasoning is unimpeded by middleware
2. **Heuristic Thinking** provides fast intuition alongside deep reasoning
3. **Memory-Augmented Context** gives the LLM persistent knowledge across sessions

The result: a minimal system that maximizes LLM capability — the intelligence lives in the model and the prompt, not in orchestration code.

---

## Further Reading

- [Thinking, Fast and Slow](https://en.wikipedia.org/wiki/Thinking,_Fast_and_Slow) — Kahneman's dual-process theory
- [First Principles Thinking](https://fs.blog/first-principles/) — Farnam Street's guide
