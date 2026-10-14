---
name: computer-use-book-flight
description: "Book a flight by driving a browser (or a desktop airline app) end-to-end. Use when the user says \"book flight\", \"buy ticket\", \"reserve seat\", or mentions a specific airline/city pair with intent to book. Drives the see → locate → act → verify loop on top of Aleph's GUI tools."
---

# Computer-use recipe — Book a flight

## Trigger phrases

- "book a flight from X to Y on DATE"
- "buy me a ticket to …"
- "find flights and reserve …"
- "订机票" / "买机票"

## Preconditions

Before the loop:
1. Confirm with the user: origin, destination, date(s), passenger count,
   class. Don't guess — wrong booking = real money.
2. Confirm payment instrument & contact info. Don't surface card numbers
   in the chat back to the user; let the page handle them.
3. Call `desktop_check_permissions` if running locally — the loop needs
   Accessibility + Screen-Recording.

## The loop

```
1. desktop_browser_operator { mode: "hybrid" }
   → read the returned `flow` and `tools` list.

2. browser_open { url: "https://www.<carrier>.com" }
   OR launch_app { bundle_id: <carrier app> }

3. desktop wait_visual { timeout_ms: 8000 }
   → wait for the homepage to settle.

4. Loop per field (Origin, Destination, Depart, Return, Pax):
   a. browser_snapshot
      → look up the field by aria-label / placeholder.
   b. If DOM found: browser_click + browser_type.
      Else: desktop_gui_locate { target_text: <field label> }
              → desktop click {center.x, center.y} + type_text.
   c. desktop wait_visual { timeout_ms: 1500 }

5. Submit the search.
   desktop_gui_locate { target_text: "Search" | "Find flights", prefer_role: "AXButton" }
   → click.

6. Wait for results.
   browser_wait_for { network_idle: true } OR desktop wait_visual { timeout_ms: 12000 }.

7. Pick a flight matching user constraints.
   browser_snapshot OR desktop screenshot → enumerate options.
   Show user the top 3, get explicit choice (do NOT auto-pick).

8. Click the chosen option; fill passenger info on the next page using
   the same locate-then-act pattern.

9. STOP before payment. Hand control to the user with a screenshot, the
   summary, and the price. Never auto-submit a payment.
```

## Verification checkpoints

- After step 3: a screenshot must show the carrier's homepage (carrier
  logo visible). If not, abort.
- After step 6: results count > 0 and matches the requested date.
- Before step 9: re-screenshot, OCR the displayed price, ask user to
  confirm — read the exact string back to them.

## Failure recovery

| Symptom | Action |
|---------|--------|
| `desktop_gui_locate { found: false }` | Re-screenshot; try a synonym ("Origin" → "From"). Then escalate to `force_ocr: true`. |
| Captcha visible | Stop. Surface a screenshot. Ask the user to solve in their own browser tab and tell us when done. |
| Modal dialog blocks input | `desktop_ax_snapshot` to read it; act on its buttons explicitly. |
| Page never settles | `desktop wait_visual` returning `stable: false` twice → reload the page or fall back to `mode: "vision"`. |

## Boundaries

- Never auto-submit payment.
- Never store the user's card details.
- If the user gave fuzzy intent ("cheap flight"), surface 3 candidates;
  do not pick.
- If the carrier's site requires login and the user isn't logged in,
  pause and ask.

## Related

- [BROWSER_OPERATOR.md](../../docs/reference/BROWSER_OPERATOR.md)
- [computer-use-book-hotel](../computer-use-book-hotel/SKILL.md) —
  shares the same loop shape.
