# Manual Verification Checklist

Items that require human visual/interactive verification. These cannot be
reliably automated.

## Instructions

Run through each item below with the Tauri bridge (`aleph-bridge`) running.
Mark each as PASS/FAIL and note any issues.

---

## M1: OCR Accuracy

- [ ] Take a screenshot of text content (e.g., a web page with paragraphs)
- [ ] Send it through `desktop.ocr` with the base64 image
- [ ] Verify the returned text is readable and reasonably accurate
- [ ] Test with both English and CJK (Chinese/Japanese) text

**Notes:**
_________________________________

---

## M2: Canvas Overlay Quality

- [ ] Send `desktop.canvas_show` with an HTML snippet containing styled text
- [ ] Verify the overlay window appears at the specified position
- [ ] Verify HTML renders correctly (fonts, colors, layout)
- [ ] Send `desktop.canvas_update` with a patch and verify the update applies
- [ ] Send `desktop.canvas_hide` and verify the overlay disappears cleanly
- [ ] Verify no visual artifacts remain after hide

**Notes:**
_________________________________

---

## M3: System Tray Interaction

- [ ] Verify the Aleph icon appears in the system tray / menu bar
- [ ] Click the tray icon and verify the menu appears
- [ ] Verify menu items: About, Show Halo, Provider submenu, Settings, Quit
- [ ] Select a provider from the submenu and verify the checkmark updates
- [ ] Verify "Show Halo" opens the Halo window

**Notes:**
_________________________________

---

## M4: Halo Window Experience

- [ ] Trigger Halo via tray menu "Show Halo"
- [ ] Trigger Halo via global hotkey (Ctrl+Alt+/)
- [ ] Verify Halo appears centered horizontally, ~70% from top of screen
- [ ] Type in the Halo input field and verify responsiveness
- [ ] Verify Halo can be dismissed (Escape key or clicking outside)
- [ ] Repeat show/hide 10 times rapidly — no crashes or visual glitches

**Notes:**
_________________________________

---

## M5: Settings Persistence

- [ ] Open Settings window (tray menu or Cmd+,)
- [ ] Change a setting (e.g., default provider, launch at login)
- [ ] Close the Settings window
- [ ] Quit and restart the bridge application
- [ ] Re-open Settings and verify the changes persisted
- [ ] Verify window position is restored after restart

**Notes:**
_________________________________

---

## Sign-off

| Item | Status | Tester | Date |
|------|--------|--------|------|
| M1: OCR Accuracy | | | |
| M2: Canvas Quality | | | |
| M3: Tray Interaction | | | |
| M4: Halo Experience | | | |
| M5: Settings Persistence | | | |

**Overall verdict:** ____________

**Tester:** ____________

**Date:** ____________
