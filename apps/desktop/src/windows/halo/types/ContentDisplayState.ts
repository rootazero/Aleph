// Content display states - mutually exclusive
export type ContentDisplayState =
  | { type: 'empty' }                           // Initial: only input box
  | { type: 'conversation' }                    // Show conversation history
  | { type: 'commandList'; prefix: string }     // "/" commands
  | { type: 'topicList'; prefix: string };      // "//" topics

// Helper functions
export function isShowingPanel(state: ContentDisplayState): boolean {
  return state.type !== 'empty';
}

export function isShowingCommandList(state: ContentDisplayState): boolean {
  return state.type === 'commandList';
}

export function isShowingTopicList(state: ContentDisplayState): boolean {
  return state.type === 'topicList';
}
