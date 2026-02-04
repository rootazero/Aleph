# Tauri UI macOS Alignment Design

> High-fidelity replication of macOS Settings UI for Tauri cross-platform version

**Date**: 2026-01-28
**Status**: Approved
**Goal**: Achieve 1:1 visual parity with macOS SwiftUI settings interface

---

## 1. Design System Enhancement

### Current Gaps

| Token | macOS (DesignTokens.swift) | Tauri Current |
|-------|---------------------------|---------------|
| Card Background | `controlBackgroundColor.opacity(0.8)` | Solid `--card` |
| Corner Radius | Concentric system (4/6/8/10/12pt) | Single `rounded-card` |
| Spacing | xs(4)/sm(8)/md(16)/lg(24)/xl(32) | Not explicitly defined |
| Typography | title(22)/heading(17)/body(14)/caption(12) | text-title/body/caption misaligned |

### Solution

**1. Update `globals.css`**:
```css
:root {
  /* Transparent card backgrounds */
  --card-glass: 0 0% 100% / 0.8;

  /* Concentric corner radius system */
  --radius-xs: 4px;
  --radius-sm: 6px;
  --radius-md: 10px;
  --radius-lg: 12px;
  --radius-xl: 16px;

  /* Spacing scale */
  --spacing-xs: 4px;
  --spacing-sm: 8px;
  --spacing-md: 16px;
  --spacing-lg: 24px;
  --spacing-xl: 32px;
}

.dark {
  --card-glass: 0 0% 12% / 0.8;
}
```

**2. Update `tailwind.config.ts`**:
```ts
theme: {
  extend: {
    spacing: {
      'xs': '4px',
      'sm': '8px',
      'md': '16px',
      'lg': '24px',
      'xl': '32px',
    },
    borderRadius: {
      'xs': '4px',
      'sm': '6px',
      'md': '10px',
      'lg': '12px',
      'xl': '16px',
    },
    fontSize: {
      'title': ['22px', { lineHeight: '1.3', fontWeight: '600' }],
      'heading': ['17px', { lineHeight: '1.4', fontWeight: '500' }],
      'body': ['14px', { lineHeight: '1.5', fontWeight: '400' }],
      'caption': ['12px', { lineHeight: '1.4', fontWeight: '400' }],
    },
  }
}
```

**3. New glass effect class**:
```css
.card-glass {
  @apply bg-[hsl(var(--card-glass))] backdrop-blur-xl;
}
```

---

## 2. Core Component Enhancement

### Enhanced SettingsCard

```tsx
interface SettingsCardProps {
  title: string;
  description?: string;
  icon?: LucideIcon;           // NEW: Left icon
  variant?: 'inline' | 'section' | 'expandable';  // NEW: Layout variants
  children: React.ReactNode;
  className?: string;
}
```

**Variants**:
- `inline` (default): Title left, control right (current behavior)
- `section`: Full-width card with stacked children
- `expandable`: Collapsible section

### New SettingsSection

```tsx
interface SettingsSectionProps {
  header: string;
  children: React.ReactNode;
}

export function SettingsSection({ header, children }: SettingsSectionProps) {
  return (
    <section className="space-y-3">
      <h2 className="text-heading font-medium text-foreground px-1">
        {header}
      </h2>
      <div className="space-y-2">
        {children}
      </div>
    </section>
  );
}
```

### New SegmentedControl

```tsx
interface SegmentedControlProps<T extends string> {
  options: Array<{
    value: T;
    label: string;
    icon?: LucideIcon;
  }>;
  value: T;
  onChange: (value: T) => void;
}
```

Visual style: Pill-shaped background, selected item highlighted with accent color.

### New InfoBox

```tsx
interface InfoBoxProps {
  variant: 'info' | 'warning' | 'success' | 'error';
  children: React.ReactNode;
}
```

Styling: Icon + text, subtle background tint matching variant color.

### Enhanced SaveBar

Add status indicator:
- Unsaved: `●` dot + "Unsaved changes" text
- Saved: `✓` checkmark + "All changes saved" text
- Error: `!` icon + error message

---

## 3. Settings Pages Alignment

### GeneralSettings

**Current Structure** (Tauri):
- Sound switch
- Launch at login switch
- Language select
- Version display

**Target Structure** (macOS alignment):
```
Section: Sound
  └─ Sound Effects toggle

Section: Startup
  └─ Launch at Login toggle
  └─ Description text

Section: Language
  └─ Language Preference picker

Section: Updates
  └─ Check for Updates button

Section: Logs
  └─ View Logs button → opens LogViewer dialog

Section: About
  └─ Version: x.x.x (Build xxx)
```

### BehaviorSettings

**Changes**:

1. **Output Mode**: Replace `Select` with `SegmentedControl`
   - Options: Typewriter (keyboard icon) | Instant (bolt icon)
   - Add `InfoBox` below explaining selected mode

2. **Typing Speed**: Add "Preview" button
   - Opens `TypingPreviewDialog` showing animation at selected speed

3. **PII Scrubbing**: Restructure to match macOS
   - Main toggle for enable/disable
   - When enabled, show checkboxes for each PII type:
     - Email addresses
     - Phone numbers
     - SSN
     - Credit card numbers
   - Remove keyword-based approach (macOS doesn't have this)

### ProvidersSettings

**Complete restructure to dual-pane layout**:

```
┌─────────────────────────────────────────────────────┐
│ [SearchBar w-60]                 [+ Add Custom]     │
├────────────────┬────────────────────────────────────┤
│ Provider List  │  Edit Panel                        │
│ (w-60 fixed)   │  (flex-1)                          │
│                │                                    │
│ ┌────────────┐ │  Provider: OpenAI                  │
│ │ OpenAI   ● │ │  ─────────────────                 │
│ │ Claude   ○ │ │  API Key: [••••••••] 👁            │
│ │ Gemini   ○ │ │  Model: [gpt-4-turbo ▼]           │
│ │ ...        │ │  Base URL: [https://...]          │
│ └────────────┘ │                                    │
│                │  [Test Connection]                 │
│                │  ✓ Connection successful           │
└────────────────┴────────────────────────────────────┘
```

**New Components**:
- `ProviderCard`: Compact list item with status indicator
- `ProviderEditPanel`: Configuration form

---

## 4. New Components File List

```
src/components/ui/
├── segmented-control.tsx    # Segmented picker (macOS style)
├── info-box.tsx             # Information callout box
├── settings-section.tsx     # Settings group with header
├── provider-card.tsx        # Provider list item
├── provider-edit-panel.tsx  # Provider configuration form
├── log-viewer.tsx           # Log viewing dialog
├── typing-preview.tsx       # Typing speed preview dialog
└── search-bar.tsx           # Search input (if not exists)
```

---

## 5. Implementation Priority

| Priority | Task | Files |
|----------|------|-------|
| **P0** | Design tokens alignment | `globals.css`, `tailwind.config.ts` |
| **P1** | Core component enhancement | `settings-card.tsx`, `save-bar.tsx` |
| **P1** | New base components | `segmented-control.tsx`, `info-box.tsx`, `settings-section.tsx` |
| **P2** | General page refactor | `GeneralSettings.tsx` |
| **P2** | Behavior page refactor | `BehaviorSettings.tsx` |
| **P3** | Providers dual-pane refactor | `ProvidersSettings.tsx`, `provider-card.tsx`, `provider-edit-panel.tsx` |
| **P4** | Remaining tabs alignment | 10 other tab files |

---

## 6. Files to Modify

```
src/styles/globals.css
tailwind.config.ts
src/components/ui/settings-card.tsx
src/components/ui/save-bar.tsx
src/windows/settings/tabs/GeneralSettings.tsx
src/windows/settings/tabs/BehaviorSettings.tsx
src/windows/settings/tabs/ProvidersSettings.tsx
```

---

## 7. Reference Files (macOS)

- `platforms/macos/Aleph/Sources/DesignSystem/DesignTokens.swift`
- `platforms/macos/Aleph/Sources/SettingsView.swift`
- `platforms/macos/Aleph/Sources/BehaviorSettingsView.swift`
- `platforms/macos/Aleph/Sources/ProvidersView.swift`
- `platforms/macos/Aleph/Sources/Components/Molecules/UnifiedSaveBar.swift`
