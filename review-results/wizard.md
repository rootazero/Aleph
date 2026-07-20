# Module: src/wizard

- Path: `src/wizard/`
- Files scanned: 6
- Total LOC: 1607
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 1 |
| medium   | 3 |
| low      | 11 |
| **Total**| **15** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/wizard/prompter.rs:103-128|medium|On `step_tx.send` failure (channel closed), the PendingAnswer is inserted into `answers` but never removed; the oneshot Sender leaks in the HashMap for the rest of the session
ISSUE|src/wizard/prompter.rs:146-222|medium|RpcProgressHandle.update/finish/finish_error are no-ops that only emit `debug!` logs; no wizard step is ever sent to the client, so the entire progress UI feature is non-functional despite the trait and types being defined
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/wizard/prompter.rs:132|high|`prompter.finish()` is part of the documented public contract but no flow in flows/onboarding.rs ever calls it; `WizardNextResult.data` is therefore permanently `None` for every current flow — the wizard collects OnboardingData then discards it
ISSUE|src/wizard/prompter.rs:138-143|medium|`prompt_no_wait` sends a step but does NOT register a PendingAnswer — a late answer() with that step_id fails with `StepNotFound` instead of a step-type-aware error
ISSUE|src/wizard/session.rs:236-266|low|Calling `answer()` twice for the same step returns `StepNotFound` even though the step was answered — misleading error
ISSUE|src/wizard/session.rs:189-233|low|No timeout on the `rx.recv().await` wait inside `prompt()` — a client that never calls `answer()` hangs the flow task indefinitely
ISSUE|src/wizard/session.rs:254-265|low|`answer()` validates `current_step.id` then drops the read lock before grabbing the answers write lock — races allow `StepNotFound` for already-completed steps
```

### Perspective 3 — Architecture Compliance
No R1/R3/R4/R8/R9/R10 violations.

### Perspective 4 — Code Quality
```
ISSUE|src/wizard/session.rs:197-199,219-223|low|Duplicated error-unwrap pattern appears twice
ISSUE|src/wizard/session.rs:74|low|`cancel_tx` is `Arc<RwLock<Option<Sender>>>` but only `write().take()` is ever called — `Mutex` would suffice
ISSUE|src/wizard/onboarding.rs:133-140|low|`imessage_transport_options()` is `pub` but never referenced — dead seam for future TOML persistence
ISSUE|src/wizard/onboarding.rs:341,343,414,484|low|`tokio::time::sleep(500ms/300ms)` calls impersonate real validation/save work
ISSUE|src/wizard/onboarding.rs:235-251|low|When secondary provider == primary, the prompt "Use the same API key as the primary provider?" is nonsensical for keyless providers
ISSUE|src/wizard/types.rs:40|low|`StepType::Action` variant defined but no code path emits/matches it
ISSUE|src/wizard/types.rs:48|low|`StepExecutor::Gateway` is set only by the unused `WizardStep::progress()` builder
ISSUE|src/wizard/types.rs:153-158|low|`WizardStep::progress()` constructor exists but no caller invokes it
ISSUE|src/wizard/types.rs:92-98|low|`WizardStep.validation` regex pattern is only enforced client-side; the wizard server accepts any string value from `answer()` without server-side validation
```
