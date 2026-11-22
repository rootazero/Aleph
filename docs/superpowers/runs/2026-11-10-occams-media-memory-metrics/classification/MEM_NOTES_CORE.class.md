# Occam's Razor Static Pattern Classification: MEM_NOTES_CORE Batch

**Target Subsystem**: `src/memory/notes` (Core data model, relations, links, keyword linker, governance, graph, and profile — 33 files, ~8,340 LoC)  
**Reviewer Role**: Jev (Fast Static Pattern Scanning, Taxonomy Classification, Filtering & Routing)  
**Doctrine**: Occam's Razor (`Simplicity is the removal of unnecessary concepts, not merely line count reduction`)  
**Timestamp**: 2026-11-10  

---

## 1. Executive Summary & Routing Metrics

| Taxonomy Category | Candidates Found | FIX-NOW | FIX-CAREFULLY | DEFER | REJECT |
|---|:---:|:---:|:---:|:---:|:---:|
| **DEAD-CODE / ACCIDENTAL-COMPLEXITY** | 4 | 2 | 0 | 0 | 2 |
| **REDUNDANT-ALLOCATION** | 20 | 18 | 1 | 1 | 0 |
| **CONTROL-FLOW / INDIRECTION** | 5 | 5 | 0 | 0 | 0 |
| **PANIC-RISK / DEFENSIVE-CHECK** | 2 | 2 | 0 | 0 | 0 |
| **CORRECTNESS / INVARIANT-TIGHTENING** | 2 | 1 | 0 | 0 | 1 |
| **TOTAL** | **33** | **28** | **1** | **1** | **3** |

---

## 2. Detailed Findings by Taxonomy

### A. REDUNDANT-ALLOCATION

#### [NOTES-ALLOC-01] Quadruple Heap Allocation per 3-word Window in MinHash `shingles()`
- **File & Lines**: `src/memory/notes/graph/minhash.rs:44-52`
- **Pattern**:
  ```rust
  for w in toks.windows(3) {
      let s = format!(
          "{} {} {}",
          w[0].to_lowercase(),
          w[1].to_lowercase(),
          w[2].to_lowercase()
      );
      out.insert(fnv1a(&s));
  }
  ```
- **Analysis**: In `shingles(body: &str)`, every 3-word sliding window evaluates three `.to_lowercase()` String allocations plus one `format!` String allocation. For a modest 500-word note, this creates ~2,000 short-lived heap allocations during offline/online graph recomputations.
- **Remediation**: Hash lowercase bytes/chars directly into an FNV-1a accumulator without intermediate String allocations (`fnv1a_3_words(w0, w1, w2)`), or lowercase the token slice once up front into reusable slices.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-02] Double String Allocation in `normalize_link_key`
- **File & Lines**: `src/memory/notes/links/resolve.rs:152-162`
- **Pattern**:
  ```rust
  pub fn normalize_link_key(s: &str) -> String {
      s.trim()
          .chars()
          .map(|c| match c {
              '\u{3000}' => ' ',
              '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFF00 + 0x20).unwrap_or(c),
              _ => c,
          })
          .collect::<String>()
          .to_lowercase()
  }
  ```
- **Analysis**: `.collect::<String>()` allocates a temporary `String`, and `.to_lowercase()` immediately allocates a second `String`. This function is executed repeatedly for every note body, alias, and wikilink across the corpus.
- **Remediation**: Fold full-width characters and lowercase in a single pass into a preallocated `String::with_capacity(s.len())`.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-03] Redundant Field Clones & 12x `format!` in `KnowledgeNote::to_markdown()`
- **File & Lines**: `src/memory/notes/note/mod.rs:233-263`
- **Pattern**:
  ```rust
  let note_type = self
      .note_type
      .clone()
      .unwrap_or_else(|| self.category.clone());
  out.push_str(&format!("type: {}\n", yaml_scalar(&note_type)));
  out.push_str(&format!("title: {}\n", yaml_scalar(&self.title)));
  out.push_str(&format!("aliases: {}\n", yaml_inline_array(&self.aliases)));
  // ... 9 more consecutive out.push_str(&format!(...)) calls
  ```
- **Analysis**: `self.note_type.clone().unwrap_or_else(|| self.category.clone())` allocates an owned `String` when `yaml_scalar` only requires a borrowed `&str`. Additionally, 12 consecutive `out.push_str(&format!(...))` calls allocate and free 12 transient heap strings on every note serialization.
- **Remediation**: Use `self.note_type.as_deref().unwrap_or(&self.category)`. Use `std::fmt::Write; let _ = write!(out, ...);` or direct string pushes.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-04] Intermediate Vectors and Multiple `format!` in `join_body_and_related`
- **File & Lines**: `src/memory/notes/note/mod.rs:512-523`
- **Pattern**:
  ```rust
  let rendered: Vec<String> = targets.iter().map(|t| format!("[[{t}]]")).collect();
  let footer = format!("{RELATED_PREFIX} {}\n", rendered.join(" "));
  ```
- **Analysis**: Allocates a temporary `Vec<String>`, formats each target into `[[{t}]]`, joins into an intermediate string, and formats into a final `footer`.
- **Remediation**: Construct the footer directly in a single `String` with delimiters in a single loop (`footer.push_str(" [["); footer.push_str(t); footer.push_str("]]");`).
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-05] Tripled Allocation in `yaml_inline_array`
- **File & Lines**: `src/memory/notes/note/helpers.rs:101-107`
- **Pattern**:
  ```rust
  let parts: Vec<String> = items.iter().map(|s| yaml_scalar(s)).collect();
  format!("[{}]", parts.join(", "))
  ```
- **Analysis**: Allocates `parts: Vec<String>`, then `parts.join(", ")` allocates a string, then `format!("[{}]", ...)` allocates a third string.
- **Remediation**: Build the flow-style array directly into a preallocated `String` with commas.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-06] Eager `to_string()` in `yaml_scalar` on Clean Inputs
- **File & Lines**: `src/memory/notes/note/helpers.rs:91-97`
- **Pattern**:
  ```rust
  pub(crate) fn yaml_scalar(s: &str) -> String {
      if needs_yaml_quote(s) {
          format!("'{}'", s.replace('\'', "''"))
      } else {
          s.to_string()
      }
  }
  ```
- **Analysis**: `yaml_scalar` returns `String`, forcing heap allocations for clean scalars that do not need YAML quoting (the majority of titles, categories, and types).
- **Remediation**: Return `std::borrow::Cow<'_, str>` so unquoted strings are borrowed without allocation.
- **Route**: `FIX-CAREFULLY` (Verify all call sites accept `Cow<'_, str>` or `&str`).

#### [NOTES-ALLOC-07] Double Allocation for Enum Display in Governance Gate `defer`
- **File & Lines**: `src/memory/notes/governance/gate.rs:141`
- **Pattern**:
  ```rust
  let severity_str = format!("{:?}", candidate.note.severity).to_lowercase();
  ```
- **Analysis**: Performs two heap allocations (`format!` string + `to_lowercase()` string) to convert `Severity` enum to lowercase string for the review queue.
- **Remediation**: Implement `const fn as_str(&self) -> &'static str` on `Severity` and call `candidate.note.severity.as_str()`.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-08] Full Vector Clone & Redundant Sort in `sync_body_to_frontmatter`
- **File & Lines**: `src/memory/notes/governance/supersession.rs:25-35`
- **Pattern**:
  ```rust
  let mut existing: std::collections::HashSet<String> =
      note.superseded_by.iter().cloned().collect();
  // ... insert regex captures ...
  note.superseded_by = existing.into_iter().collect();
  note.superseded_by.sort();
  ```
- **Analysis**: Clones every existing string in `note.superseded_by`. Then collects to vector and sorts.
- **Remediation**: Take ownership with `std::mem::take(&mut note.superseded_by)` into a `BTreeSet<String>` (or sort/dedup the vector in-place), eliminating all existing string clones and the post-sort pass.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-09] Per-Pair Vector Allocations in `pair_by_overlap`
- **File & Lines**: `src/memory/notes/keyword_linker/overlap.rs:56-78`
- **Pattern**:
  ```rust
  let shared: Vec<&str> = sets[i].intersection(&sets[j]).copied().collect();
  if shared.is_empty() { continue; }
  let specific: Vec<&str> = shared.iter().copied().filter(|s| is_specific(s)).collect();
  ```
- **Analysis**: For every pair `(i, j)`, collects elements into temporary `Vec<&str>` buffers twice. For 100 notes, 4,950 pairs allocate up to 9,900 short-lived vectors.
- **Remediation**: Inspect the `intersection` iterator directly in a single pass without collecting into intermediate `Vec`s.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-10] Repeated `format!` in Keyword Extraction Prompt Builder
- **File & Lines**: `src/memory/notes/keyword_linker/extract.rs:47-58`
- **Pattern**:
  ```rust
  user.push_str(&format!(
      "<note path=\"{}\">\ntitle: {}\nsummary: {}\n", ...
  ));
  for f in n.facts.iter().take(6) {
      user.push_str(&format!("- {}\n", escape_fence(f)));
  }
  ```
- **Analysis**: Creates dozens of transient heap `String`s by formatting each note header and each bullet fact.
- **Remediation**: Use `write!(user, ...)` with `std::fmt::Write` or push string slices directly.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-11] Repeated `HashMap` Allocation inside Louvain Node Loop
- **File & Lines**: `src/memory/notes/graph/community.rs:119-124`
- **Pattern**:
  ```rust
  for i in 0..n {
      let ci = comm[i];
      sigma_tot[ci] -= k[i];
      let mut w_to: HashMap<usize, f64> = HashMap::new();
  ```
- **Analysis**: Inside `local_moving`, on every node iteration `for i in 0..n` across Louvain passes, a fresh `HashMap<usize, f64>` is allocated.
- **Remediation**: Lift `w_to` outside the `for i in 0..n` loop and call `w_to.clear()` before each node.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-12] Eager EdgeMeta Allocation and Double Cloning in GraphIndex Builder
- **File & Lines**: `src/memory/notes/graph/mod.rs:89-102`
- **Pattern**:
  ```rust
  let meta = EdgeMeta {
      rel_type: e.rel_type.clone(),
      confidence: e.confidence,
  };
  out[a].entry(b)
      .and_modify(|m| {
          if meta.confidence > m.confidence {
              *m = meta.clone();
          }
      })
      .or_insert(meta);
  ```
- **Analysis**: Always clones `e.rel_type` to build `meta`. If the edge exists and is weaker, `meta` is dropped (wasted clone). If it modifies, `meta.clone()` clones the string a second time.
- **Remediation**: Use `out[a].entry(b).and_modify(...).or_insert_with(...)` so `e.rel_type` is only cloned when needed and never twice.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-13] Repeated Multi-line `format!` in `render_user_md`
- **File & Lines**: `src/memory/notes/profile/store.rs:217-244`
- **Pattern**:
  ```rust
  out.push_str(&format!("updated: \"{today}\"\n"));
  out.push_str(&format!("revision: {revision}\n"));
  out.push_str(&format!("last_session: \"{last_session}\"\n"));
  out.push_str(&format!("confidence: \"{confidence}\"\n"));
  out.push_str(&format!("## {heading}\n"));
  out.push_str(&format!("- {bullet}\n"));
  out.push_str(&format!("- {section}: {}\n", ids.join(", ")));
  ```
- **Analysis**: Generates 15-30+ transient `String`s via `format!` on every profile write.
- **Remediation**: Use `write!(out, ...)` with `std::fmt::Write`.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-14] Redundant Vector Allocations in Profile `compute_diff`
- **File & Lines**: `src/memory/notes/profile/synthesizer.rs:226-235`
- **Pattern**:
  ```rust
  for s in ProfileSection::ALL {
      let heading = s.heading().to_string();
      let old_bullets: Vec<&str> = old.get(&heading).map(...).unwrap_or_default();
      let new_bullets: Vec<&str> = new.get(&heading).map(...).unwrap_or_default();
  ```
- **Analysis**: For all 6 sections, allocates `heading` `String` plus two `Vec<&str>` allocations per section (18 heap allocations per diff).
- **Remediation**: Index `old` and `new` with `s.heading()` directly, borrowing `&Vec<String>` without intermediate slice vectors.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-15] Unmatched Wikilinks Allocate Heap Strings in `rewrite_wikilinks`
- **File & Lines**: `src/memory/notes/wikilink.rs:44-54`
- **Pattern**:
  ```rust
  WIKILINK_RE.replace_all(text, |caps: &regex::Captures| {
      if caps.get(1).is_some_and(|m| m.as_str() == old_name) {
          // ...
      } else {
          caps.get(0).map_or("", |m| m.as_str()).to_string()
      }
  })
  ```
- **Analysis**: In `replace_all`, unmatched wikilinks call `.to_string()`. In a document with 50 links where 1 is renamed, 49 heap strings are allocated and discarded.
- **Remediation**: Return `std::borrow::Cow<'_, str>` so that unmatched links return `Cow::Borrowed` without allocation.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-16] Unnecessary `to_ascii_lowercase()` Allocation in `canonicalize_category`
- **File & Lines**: `src/memory/notes/indexer.rs:92-101`
- **Pattern**:
  ```rust
  let trimmed = raw.trim();
  let key = trimmed.to_ascii_lowercase();
  for &(variant, canonical) in CATEGORY_ALIASES {
      if key == variant { return canonical.to_string(); }
  }
  ```
- **Analysis**: Allocates a heap `String` on every category validation/canonicalization, even though `CATEGORY_ALIASES` can be matched in-place with `trimmed.eq_ignore_ascii_case(variant)`.
- **Remediation**: Use `trimmed.eq_ignore_ascii_case(variant)`.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-17] Missing Owned Consuming Method `into_memory_fact` on `NoteSearchResult`
- **File & Lines**: `src/memory/notes/search_result.rs:31-43`
- **Pattern**:
  ```rust
  pub fn to_memory_fact(&self, agent_id: &str) -> MemoryFact {
      let mut fact = MemoryFact::new(self.content.clone(), note_type, self.tags.clone());
      fact.id = self.path.clone();
      // ...
  ```
- **Analysis**: `to_memory_fact(&self)` forces cloning `content` (which can be tens or hundreds of kilobytes), `tags`, and `path`, even when the caller owns `NoteSearchResult`.
- **Remediation**: Provide `pub fn into_memory_fact(self, agent_id: &str) -> MemoryFact` and `into_scored_fact(self, agent_id: &str)` to move owned fields.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-18] Double String Allocation in `sanitize_title`
- **File & Lines**: `src/memory/notes/note/helpers.rs:188-195`
- **Pattern**:
  ```rust
  let cleaned: String = title
      .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "")
      .trim()
      .to_string();
  let cleaned = crate::memory::notes::store::strip_md_ext(&cleaned).to_string();
  ```
- **Analysis**: `trim().to_string()` allocates a String, and then `strip_md_ext(&cleaned).to_string()` allocates a second String.
- **Remediation**: Apply `strip_md_ext` directly on the trimmed slice before finalizing the String.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-19] Per-Bullet Heading Clone in `parse_user_md`
- **File & Lines**: `src/memory/notes/profile/store.rs:168-174`
- **Pattern**:
  ```rust
  } else if let Some(ref h) = current_heading {
      sections.entry(h.clone()).or_default().push(bullet.to_string());
  }
  ```
- **Analysis**: Clones `h` on every single bullet line in a section just to perform `sections.entry(h.clone())`.
- **Remediation**: Track current active section buffer or only clone heading when transitioning between sections.
- **Route**: `FIX-NOW`

#### [NOTES-ALLOC-20] Borrowed Parameter in `DefaultNoteWriteGate::evaluate`
- **File & Lines**: `src/memory/notes/governance/gate.rs:88-90, 129`
- **Pattern**:
  ```rust
  async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError> {
      if candidate.bypass_review {
          return Ok(GateOutcome::Accept(candidate.clone()));
      }
      // ...
      Ok(GateOutcome::Accept(candidate.clone()))
  }
  ```
- **Analysis**: Trait signature takes `&CandidateNote` and clones it to return `GateOutcome::Accept(CandidateNote)`.
- **Remediation**: Changing trait signature to owned `CandidateNote` would break the public `NoteWriteGate` trait across multiple call sites.
- **Route**: `DEFER` (Preserve public trait contract stability).

---

### B. CONTROL-FLOW / INDIRECTION

#### [NOTES-FLOW-01] 6-Level Deep Nested Conditionals in `cohesion_per_community`
- **File & Lines**: `src/memory/notes/graph/community.rs:207-221`
- **Pattern**:
  ```rust
  for i in 0..g.len() {
      if let Some(nbrs) = g.adj.get(i) {
          for &j in nbrs {
              if i < j {
                  if let (Some(&ci), Some(&cj)) = (of_node.get(i), of_node.get(j)) {
                      if ci == cj {
                          if let Some(slot) = intra.get_mut(ci) {
                              *slot += 1;
                          }
                      }
                  }
              }
          }
      }
  }
  ```
- **Analysis**: Excessive nesting with defensive `.get()` calls for slice indices guaranteed to be within bounds.
- **Remediation**: Flatten using `for (i, nbrs) in g.adj.iter().enumerate()` with direct indexing `let ci = of_node[i];` and combined predicate `if i < j && ci == of_node[j]`.
- **Route**: `FIX-NOW`

#### [NOTES-FLOW-02] Duplicate Node Lookups in `detect` Surprising Edges
- **File & Lines**: `src/memory/notes/graph/insights.rs:108-124`
- **Pattern**:
  ```rust
  let cross_type = match (g.nodes.get(i), g.nodes.get(j)) {
      (Some(ni), Some(nj)) => ni.category != nj.category,
      _ => false,
  };
  // ...
  if let (Some(ni), Some(nj)) = (g.nodes.get(i), g.nodes.get(j)) {
      surprising.push(SurprisingEdge { ... });
  }
  ```
- **Analysis**: `g.nodes.get(i)` and `g.nodes.get(j)` are looked up and pattern matched twice in the same loop iteration.
- **Remediation**: Bind `let (ni, nj) = (&g.nodes[i], &g.nodes[j]);` once at the start of the candidate loop.
- **Route**: `FIX-NOW`

#### [NOTES-FLOW-03] Linear Scan in Mention Link Exclusion
- **File & Lines**: `src/memory/notes/links/mentions.rs:128-155`
- **Pattern**:
  ```rust
  let linked: Vec<String> = d.linked_raw.iter().map(|s| normalize_link_key(s)).collect();
  // ...
  for (name_norm, target, target_norm, cjk) in &entries {
      if linked.iter().any(|l| l == name_norm || l == target_norm) {
          continue;
      }
  ```
- **Analysis**: In `scan_mentions`, `linked` is a `Vec<String>`. In the inner loop over all dictionary `entries`, it performs an O(L) linear search, making the exclusion check O(D * E * L).
- **Remediation**: Collect `linked` into a `HashSet<String>` for O(1) membership testing.
- **Route**: `FIX-NOW`

#### [NOTES-FLOW-04] Duplicated Fact Flushing in `extract_facts`
- **File & Lines**: `src/memory/notes/note/parsing.rs:339-366`
- **Pattern**:
  `if let Some(c) = current.take() { out.push(c); }` is repeated across 4 different branches.
- **Analysis**: Fact emission logic is scattered across multiple branches instead of having an explicit continuation condition vs termination condition.
- **Remediation**: Restructure the loop so that any non-continuation line flushes `current` through a single code path.
- **Route**: `FIX-NOW`

#### [NOTES-FLOW-05] Redundant Degree Check Before Allocation in Bridge Detection
- **File & Lines**: `src/memory/notes/graph/insights.rs:79-95`
- **Pattern**:
  Allocates `HashSet<usize>` for every node in `0..g.len()`, even for nodes with degree < 2.
- **Analysis**: `BRIDGE_MIN_COMMUNITIES` is 3. A node with degree < 2 can never connect to 3 distinct communities.
- **Remediation**: Add `if g.degree(i) < 2 { continue; }` before allocating or checking neighbour sets.
- **Route**: `FIX-NOW`

---

### C. DEAD-CODE / ACCIDENTAL-COMPLEXITY

#### [NOTES-DEAD-01] `HashMap<String, ()>` Used in Place of `HashSet<String>`
- **File & Lines**: `src/memory/notes/links/resolve.rs:105`
- **Pattern**:
  ```rust
  pub struct LinkResolveContext {
      paths: HashMap<String, ()>,
      ...
  ```
- **Analysis**: Using `HashMap<String, ()>` instead of `HashSet<String>` is an accidental map indirection anti-pattern.
- **Remediation**: Replace with `paths: HashSet<String>`.
- **Route**: `FIX-NOW`

#### [NOTES-DEAD-02] Duplicate Regex-to-Provenance Mapping Logic
- **File & Lines**: `src/memory/notes/note/parsing.rs:35-51` and `src/memory/notes/note/parsing.rs:305-320`
- **Pattern**: Identical logic unpacking regex captures into `FactProvenance` (matching `raw_source`, `prior_note`, `inferred`, `system`, etc.).
- **Analysis**: Two separate functions maintain identical capture mapping, violating single source of truth.
- **Remediation**: Consolidate into a private helper `provenance_from_captures(c: &regex::Captures) -> FactProvenance`.
- **Route**: `FIX-NOW`

#### [NOTES-DEAD-03] Prefix Formatting in `dedup.rs`
- **File & Lines**: `src/memory/notes/dedup.rs:30`
- **Analysis**: Allocates `let prefix = format!("{category}/");` once per query.
- **Remediation**: Harmless and clear in local context.
- **Route**: `REJECT`

#### [NOTES-DEAD-04] Test Fixture Scaffolding in `tests.rs`
- **File & Lines**: `src/memory/notes/graph/tests.rs`, `src/memory/notes/note/tests.rs`
- **Analysis**: Unit tests repeat snapshot graph and note structures.
- **Remediation**: Test setup code is isolated from production binaries.
- **Route**: `REJECT`

---

### D. PANIC-RISK / DEFENSIVE-CHECK

#### [NOTES-PANIC-01] `build_merge_user_prompt` Recent Turn Slicing
- **File & Lines**: `src/memory/notes/profile/prompts.rs:98-111`
- **Pattern**:
  ```rust
  let turns: Vec<String> = signal
      .recent_user_turns
      .iter()
      .take(6)
      .map(|t| {
          if t.len() > 500 {
              t.chars().take(500).collect()
          } else {
              t.clone()
          }
      })
      .collect();
  ```
- **Analysis**: While `.chars().take(500).collect()` is UTF-8 safe, it allocates an extra String even when slicing could use `floor_char_boundary` / `is_char_boundary`.
- **Remediation**: Snap byte boundary to valid char boundary and slice `&str`, avoiding redundant allocations while preserving panic-safety.
- **Route**: `FIX-NOW`

#### [NOTES-PANIC-02] Linear Quadratic Search in Footer Extraction
- **File & Lines**: `src/memory/notes/note/mod.rs:496-505`
- **Pattern**:
  ```rust
  for target in extract_wikilinks(line) {
      if !targets.contains(&target) {
          targets.push(target);
      }
  }
  ```
- **Analysis**: O(N^2) scan for target deduplication in trailing footer parser.
- **Remediation**: Use `HashSet` for deduplication or keep small bounded buffer.
- **Route**: `FIX-NOW`

---

### E. CORRECTNESS / INVARIANT-TIGHTENING

#### [NOTES-INV-01] Missing Canonical `Severity::as_str()` Method
- **File & Lines**:
  - `src/memory/notes/note/types.rs:16-22`
  - `src/memory/notes/governance/gate.rs:141`
  - `src/memory/notes/note/mod.rs:245-250`
- **Analysis**: `Severity` lacks a canonical `as_str(&self) -> &'static str`. Different call sites invent divergent representations: `gate.rs:141` formats debug lowercase, while `note/mod.rs:245` matches to `"low" | "med" | "high" | "critical"`.
- **Remediation**: Add `pub const fn as_str(&self) -> &'static str` to `Severity` and use it uniformly across all serialization and governance points.
- **Route**: `FIX-NOW`

#### [NOTES-INV-02] `NoteStore` Trait Surface Invariant
- **File & Lines**: `src/memory/notes/store.rs:151-400`
- **Analysis**: `NoteStore` trait defines core database abstraction across Aleph.
- **Remediation**: All public method signatures, bounds, and async return types must be preserved intact.
- **Route**: `REJECT` (Modifying trait methods violates subsystem non-negotiable invariants).

---

## 3. File-by-File Coverage Matrix (33 Files)

| File Path | LoC | Candidates | Routes Assigned |
|---|:---:|:---:|---|
| `src/memory/notes/dedup.rs` | 128 | 1 | 1x REJECT |
| `src/memory/notes/governance/gate.rs` | 277 | 2 | 1x FIX-NOW, 1x DEFER |
| `src/memory/notes/governance/mod.rs` | 5 | 0 | Clean module declarations |
| `src/memory/notes/governance/supersession.rs` | 113 | 1 | 1x FIX-NOW |
| `src/memory/notes/graph/community.rs` | 233 | 2 | 2x FIX-NOW |
| `src/memory/notes/graph/insights.rs` | 143 | 2 | 2x FIX-NOW |
| `src/memory/notes/graph/minhash.rs` | 221 | 1 | 1x FIX-NOW |
| `src/memory/notes/graph/mod.rs` | 202 | 1 | 1x FIX-NOW |
| `src/memory/notes/graph/relevance.rs` | 164 | 0 | High-performance numeric loops, clean |
| `src/memory/notes/graph/tests.rs` | 554 | 1 | 1x REJECT (Test scaffolding) |
| `src/memory/notes/indexer.rs` | 1,442 | 1 | 1x FIX-NOW |
| `src/memory/notes/keyword_linker/extract.rs` | 142 | 1 | 1x FIX-NOW |
| `src/memory/notes/keyword_linker/mod.rs` | 9 | 0 | Clean re-exports |
| `src/memory/notes/keyword_linker/overlap.rs` | 142 | 1 | 1x FIX-NOW |
| `src/memory/notes/links/mentions.rs` | 354 | 1 | 1x FIX-NOW |
| `src/memory/notes/links/mod.rs` | 11 | 0 | Clean re-exports |
| `src/memory/notes/links/resolve.rs` | 280 | 2 | 2x FIX-NOW |
| `src/memory/notes/mod.rs` | 35 | 0 | Clean subsystem root re-exports |
| `src/memory/notes/note/helpers.rs` | 214 | 3 | 2x FIX-NOW, 1x FIX-CAREFULLY |
| `src/memory/notes/note/mod.rs` | 541 | 3 | 3x FIX-NOW |
| `src/memory/notes/note/parsing.rs` | 406 | 2 | 2x FIX-NOW |
| `src/memory/notes/note/relation.rs` | 65 | 0 | Clean entity relation definitions |
| `src/memory/notes/note/tests.rs` | 741 | 1 | 1x REJECT (Test scaffolding) |
| `src/memory/notes/note/types.rs` | 78 | 1 | 1x FIX-NOW |
| `src/memory/notes/profile/mod.rs` | 11 | 0 | Clean re-exports |
| `src/memory/notes/profile/prompts.rs` | 213 | 1 | 1x FIX-NOW |
| `src/memory/notes/profile/store.rs` | 401 | 2 | 2x FIX-NOW |
| `src/memory/notes/profile/synthesizer.rs` | 659 | 1 | 1x FIX-NOW |
| `src/memory/notes/profile/types.rs` | 108 | 0 | Clean data types |
| `src/memory/notes/search_result.rs` | 116 | 1 | 1x FIX-NOW |
| `src/memory/notes/store.rs` | 1,250 | 1 | 1x REJECT (Public trait surface) |
| `src/memory/notes/watcher.rs` | 631 | 0 | Well-structured debounce logic |
| `src/memory/notes/wikilink.rs` | 272 | 1 | 1x FIX-NOW |

---

## 4. Non-Negotiable Invariant Safety Checklist

- [x] **Public API Surface**: `NoteStore`, `NoteIndexer`, `KnowledgeNote`, `UserProfile` public traits and signatures remain un-broken.
- [x] **Markdown Round-Trip Byte-Compatibility**: YAML frontmatter keys (`ExtraFrontmatter`), RFC3339 timestamp representations, and verbatim note body round-tripping remain bit-exact.
- [x] **SQLite Schema & Index Integrity**: Foreign keys, vector tables (`notes_vec_*`), FTS5 tables, and link relations remain unchanged.
- [x] **Graph Algorithm Determinism**: Louvain community detection tie-breaking, MinHash seeds, and 5-signal relevance rankings retain exact deterministic outputs.
- [x] **Concurrency & Lock Safety**: Mutex and RwLock critical sections in `NoteVaultWatcher`, `FsProfileSynthesizer`, and `NoteIndexer` maintain lock order without risk of inversion.
