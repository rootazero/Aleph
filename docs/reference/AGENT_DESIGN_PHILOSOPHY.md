# Agent Design Philosophy

> *"The mind is not a vessel to be filled, but a fire to be kindled."* — Plutarch

This document outlines the four core design principles that guide Aleph's Agent architecture.

---

## 1. First Principles Thinking (第一性原理)

**Core Idea**: Define success before starting execution.

Traditional AI agents jump straight into action, hoping to stumble upon the right solution. Aleph takes a different approach: **anchor on the fundamental goal first**.

Before any task execution, the Agent generates a **Success Manifest** — a contract that explicitly defines:
- What does "done" look like?
- What are the hard constraints that must be satisfied?
- What are the soft metrics for optimization?

This prevents the common failure mode where an agent "completes" a task but misses the actual intent.

```
❌ Traditional: User Request → Execute → Hope it's right
✅ Aleph:     User Request → Define Success → Execute → Validate Against Contract
```

**Why it matters**: When you know what success looks like, every action becomes purposeful. No more aimless exploration or self-deception.

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

## 3. Self-Learning (自我学习)

**Core Idea**: Crystallize successful experiences for future reuse.

Every successful task execution is an opportunity to learn. Aleph implements an **Experience Crystallizer** that:

1. **Records** successful solution paths
2. **Detects patterns** across similar tasks
3. **Promotes** recurring patterns to reusable skills

```
Single Success → Experience Entry (vector DB)
     ↓
3+ Similar Successes → Candidate Skill
     ↓
5+ Successes + High Reuse Rate → Permanent Skill
```

**The learning loop**:
- When facing a new task, retrieve similar past experiences
- Apply proven solution patterns as starting points
- Continuously refine based on outcomes

This transforms Aleph from a stateless chatbot into an **adaptive intelligence** that genuinely improves over time.

---

## 4. POE Architecture (原则-执行-评估)

**Core Idea**: Separate the roles of planner, executor, and critic.

POE stands for **Principle-Operation-Evaluation**, a three-phase loop that ensures goal-directed behavior:

```
┌─────────────────────────────────────────────────────┐
│                    POE Loop                          │
├─────────────────────────────────────────────────────┤
│                                                      │
│  ┌─────────────────────────────────────────────┐   │
│  │ P - Principle (第一性原理锚定)                │   │
│  │   Generate Success Manifest from user intent  │   │
│  └─────────────────────────────────────────────┘   │
│                         ↓                           │
│  ┌─────────────────────────────────────────────┐   │
│  │ O - Operation (启发式执行)                    │   │
│  │   Execute with heuristic guidance             │   │
│  │   ← Retrieve similar experiences              │   │
│  └─────────────────────────────────────────────┘   │
│                         ↓                           │
│  ┌─────────────────────────────────────────────┐   │
│  │ E - Evaluation (结果导向校验)                 │   │
│  │   Validate output against Success Manifest    │   │
│  │   Independent critic, not self-assessment     │   │
│  └─────────────────────────────────────────────┘   │
│                         ↓                           │
│  ┌─────────────────────────────────────────────┐   │
│  │ Decision Branch                               │   │
│  │   ✓ Pass    → Crystallize experience → Done   │   │
│  │   ✗ Stuck   → Switch strategy                 │   │
│  │   ✗ Budget  → Escalate to human               │   │
│  │   ? Retry   → Inject feedback → Back to O     │   │
│  └─────────────────────────────────────────────┘   │
│                                                      │
└─────────────────────────────────────────────────────┘
```

**Key innovations**:
- **Physical separation**: The POE Manager stands at a higher dimension, orchestrating Workers (executors)
- **Independent evaluation**: The Critic doesn't trust the Worker's self-assessment
- **Entropy budget**: Prevents infinite retry loops with diminishing returns detection
- **Graceful degradation**: When stuck, switch strategies or escalate rather than spin

---

## Putting It All Together

These four principles work in concert:

1. **First Principles** ensures we know where we're going
2. **Heuristic Thinking** helps us get there efficiently
3. **Self-Learning** makes us better at similar journeys
4. **POE Architecture** orchestrates the entire process with accountability

The result: an Agent that doesn't just react to prompts, but **pursues goals with purpose, learns from experience, and knows when to ask for help**.

---

## Further Reading

- [POE Architecture Design](plans/2026-02-01-poe-architecture-design.md) — Detailed technical specification
- [Thinking, Fast and Slow](https://en.wikipedia.org/wiki/Thinking,_Fast_and_Slow) — Kahneman's dual-process theory
- [First Principles Thinking](https://fs.blog/first-principles/) — Farnam Street's guide
