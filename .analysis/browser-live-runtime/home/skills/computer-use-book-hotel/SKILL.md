---
name: computer-use-book-hotel
description: "Reserve a hotel room by driving a booking site end-to-end. Use when the user says \"book hotel\", \"reserve room\", \"find accommodation\", or names a city + dates with intent to stay. Same see → locate → act → verify loop as the flight recipe but with city/dates/rooms fields."
---

# Computer-use recipe — Book a hotel

## Trigger phrases

- "book a hotel in CITY for DATES"
- "reserve a room at HOTEL_NAME"
- "find a place to stay in CITY"
- "订酒店" / "订房"

## Preconditions

1. Confirm with the user: city, check-in date, check-out date, room
   count, guest count, room type, budget.
2. Confirm contact info & payment instrument exists (don't read card
   numbers).
3. Call `desktop_check_permissions` if running locally.

## The loop

```
1. desktop_browser_operator { mode: "hybrid" }

2. browser_open { url: "https://www.<site>.com" }

3. desktop wait_visual { timeout_ms: 8000 }

4. Loop per field (Destination, Check-in, Check-out, Rooms, Guests):
   a. browser_snapshot — find the field.
   b. If DOM: browser_click + browser_type.
      Else:   desktop_gui_locate → click + type_text.
   c. For date pickers that hide the input, locate the input first by
      its label, click it, then `desktop_gui_locate` on the calendar
      day. Date pickers often need keyboard arrow-key navigation as a
      last resort.
   d. desktop wait_visual { timeout_ms: 1500 }

5. Submit search.
   desktop_gui_locate { target_text: "Search" | "Find hotels" } → click.

6. Wait for results.
   browser_wait_for OR desktop wait_visual { timeout_ms: 15000 }.

7. Filter by budget (if set).
   desktop_gui_locate { target_text: "Price" | "Sort by" } → use UI.

8. Show user top 3 candidates with prices and ratings; get explicit
   choice. Do NOT auto-pick — hotel choice is highly subjective.

9. Open the chosen property. Verify room type & dates on its page.
   Re-screenshot. Ask user to confirm "Reserve at $X for these dates?"

10. STOP before payment. Hand control back with screenshot + summary.
```

## Verification checkpoints

- After step 3: homepage logo visible.
- After step 6: result count > 0 and date range echoes the request.
- After step 9: full price including taxes/fees is OCR'd and read back
  to the user verbatim.

## Failure recovery

| Symptom | Action |
|---------|--------|
| Map view obscures list | Look for a "List" toggle via `desktop_gui_locate`; otherwise `desktop_browser_operator { mode: "vision" }`. |
| Region-locked site | Surface to user; ask them to switch country in the chat. |
| Currency mismatch | OCR the currency; if it doesn't match the user's locale, surface a warning before committing. |

## Boundaries

- Never reserve without explicit confirmation of price + dates.
- Never use cached payment without re-confirming the card last-4 to
  user.
- Never book the cheapest option unless the user said "cheapest" —
  ratings matter.

## Related

- [BROWSER_OPERATOR.md](../../docs/reference/BROWSER_OPERATOR.md)
- [computer-use-book-flight](../computer-use-book-flight/SKILL.md)
