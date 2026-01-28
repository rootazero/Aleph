# Halo Window Enhancement Design

**Date**: 2026-01-28
**Status**: Approved

## Goal

Enhance the Tauri Halo window to achieve visual parity with macOS native implementation and integrate all 14 dormant components as embedded cards in the conversation flow.

## Current State

- 4 active components: InputArea, ConversationArea, CommandList, TopicList
- 14 dormant components: processing, success, error, toast, clarification, tool confirmation, plan confirmation/progress, task graph, agent plan/progress/conflict, listening, retrieving memory, typewriting
- Basic conversation flow works with streaming support
- Display state only supports: empty / conversation / commandList / topicList

## Design

### 1. Message Model Extension

Extend `HaloMessage` to support system cards embedded in the conversation flow:

```typescript
type HaloMessage =
  | { id: string; role: 'user'; content: string; timestamp: number }
  | { id: string; role: 'assistant'; content: string; timestamp: number; isStreaming?: boolean }
  | { id: string; role: 'system'; timestamp: number; card: SystemCard }

type SystemCard =
  | { type: 'processing'; provider?: string; content?: string }
  | { type: 'success'; message?: string }
  | { type: 'error'; message: string; canRetry: boolean }
  | { type: 'toast'; level: 'info' | 'warning' | 'error'; title: string; message: string }
  | { type: 'clarification'; question: string; options?: string[] }
  | { type: 'toolConfirmation'; tool: string; description?: string; args: Record<string, unknown> }
  | { type: 'planConfirmation'; plan: PlanInfo }
  | { type: 'planProgress'; progress: PlanProgressInfo }
  | { type: 'taskGraphConfirmation'; graph: TaskGraph }
  | { type: 'taskGraphProgress'; graph: TaskGraph; state: TaskGraphState }
  | { type: 'agentPlan'; planId: string; title: string; operations: AgentOperation[] }
  | { type: 'agentProgress'; planId: string; progress: number; currentOperation?: string }
  | { type: 'agentConflict'; conflict: ConflictInfo }
  | { type: 'listening' }
  | { type: 'retrievingMemory' }
  | { type: 'typewriting'; content: string; progress: number }
```

### 2. Embedded Card Visual Design

All cards share a base style and differ by accent color and content.

**Base card style**:
- Background: `bg-card/80 backdrop-blur-sm`
- Left border: 2px colored by type
- Corner radius: `rounded-md` (6px)
- Padding: `p-3`
- Full width within conversation area

**Card type colors**:
| Type | Left Border | Icon Color |
|------|------------|------------|
| processing / tool / plan | Purple | Purple |
| agent | Blue | Blue |
| success | Green | Green |
| error | Red | Red |
| warning / toast(warning) | Orange | Orange |
| info / toast(info) | Blue | Blue |
| clarification | Blue | Blue |

**Card layout**:
```
┌─ colored left border ─────────────────────────┐
│  [Icon]  Title                  [Action Btns]  │
│          Description / Details                  │
│          [Progress Bar / Options / JSON]         │
└────────────────────────────────────────────────┘
```

### 3. ArcSpinner Component

CSS implementation of macOS purple gradient arc spinner:
- SVG circle arc (252° / 70% of circle)
- Conic gradient from transparent to purple
- Linear rotation animation, 0.8s
- Round stroke-linecap
- Default size: 16x16px
- Color: purple (configurable)

### 4. Visual Polish for Existing Components

#### ConversationArea
- Replace generic loading spinner with ArcSpinner
- AI message left accent line (purple, 2px) instead of avatar circle
- Subtle glass effect on message bubbles
- Streaming uses ArcSpinner indicator

#### InputArea
- Use `card-glass` background with softer border
- Send button: purple filled style
- Remove Close(X) button (ESC handles close)

#### CommandList / TopicList
- Selected item: `bg-accent` (soft) instead of `bg-primary`
- Icon colors: `text-muted-foreground`, purple when selected

### 5. UnifiedHaloStore Extension

Add store actions for system card lifecycle:

```typescript
// Add system card to conversation
addSystemCard(card: SystemCard): string  // returns card id

// Update existing system card (e.g., processing → success)
updateSystemCard(id: string, card: Partial<SystemCard>): void

// Remove system card
removeSystemCard(id: string): void

// Handle callbacks from interactive cards
handleToolConfirmation(id: string, approved: boolean): void
handlePlanConfirmation(id: string, approved: boolean): void
handleClarificationResponse(id: string, response: string): void
handleAgentAction(id: string, action: string): void
```

### 6. ConversationArea Rendering

```tsx
{messages.map(msg => {
  if (msg.role === 'user') return <UserBubble key={msg.id} ... />;
  if (msg.role === 'assistant') return <AssistantBubble key={msg.id} ... />;
  if (msg.role === 'system') return <SystemCardRenderer key={msg.id} card={msg.card} />;
})}
```

`SystemCardRenderer` delegates to the appropriate dormant component based on `card.type`.

## Implementation Priority

### Phase A: Visual Polish (existing components)
1. Create ArcSpinner component
2. Polish ConversationArea (accent line, glass bubbles, ArcSpinner)
3. Polish InputArea (glass bg, purple send, remove X)
4. Polish CommandList/TopicList (soft selection)

### Phase B: Card Infrastructure
5. Extend HaloMessage types with SystemCard
6. Extend unifiedHaloStore with card actions
7. Create SystemCardRenderer wrapper
8. Create shared CardBase component (left border, glass bg)

### Phase C: Integrate Dormant Components
9. Wire processing/success/error cards
10. Wire toolConfirmation card
11. Wire planConfirmation/planProgress cards
12. Wire clarification card
13. Wire agentPlan/agentProgress/agentConflict cards
14. Wire toast card
15. Wire taskGraph cards

### Phase D: Polish
16. Animation transitions for card appear/disappear
17. Auto-dismiss for success/toast cards
18. Keyboard shortcuts for confirmation cards (Enter/Esc)
