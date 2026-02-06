import { useRef, useEffect } from 'react';
import { Send } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';

export function InputArea() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const {
    inputText,
    isProcessing,
    displayState,
    handleInputChange,
    sendMessage,
    handleEscape,
    moveCommandSelection,
    moveTopicSelection,
    selectCommand,
    selectTopic,
    filteredCommands,
    filteredTopics,
    selectedCommandIndex,
    selectedTopicIndex,
  } = useUnifiedHaloStore();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Arrow keys for selection in command/topic mode
    if (displayState.type === 'commandList' || displayState.type === 'topicList') {
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (displayState.type === 'commandList') {
          moveCommandSelection('up');
        } else {
          moveTopicSelection('up');
        }
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (displayState.type === 'commandList') {
          moveCommandSelection('down');
        } else {
          moveTopicSelection('down');
        }
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (displayState.type === 'commandList' && filteredCommands[selectedCommandIndex]) {
          selectCommand(filteredCommands[selectedCommandIndex]);
        } else if (displayState.type === 'topicList' && filteredTopics[selectedTopicIndex]) {
          selectTopic(filteredTopics[selectedTopicIndex]);
        }
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        if (displayState.type === 'commandList' && filteredCommands[selectedCommandIndex]) {
          selectCommand(filteredCommands[selectedCommandIndex]);
        } else if (displayState.type === 'topicList' && filteredTopics[selectedTopicIndex]) {
          selectTopic(filteredTopics[selectedTopicIndex]);
        }
        return;
      }
    }

    // Normal mode
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    } else if (e.key === 'Escape') {
      handleEscape();
    }
  };

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    handleInputChange(e.target.value);
  };

  const canSend =
    inputText.trim() &&
    !inputText.startsWith('/') &&
    !isProcessing &&
    displayState.type !== 'commandList' &&
    displayState.type !== 'topicList';

  return (
    <div className="p-3">
      <div className="flex items-end gap-2">
        <textarea
          ref={inputRef}
          value={inputText}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder="Type a message... (/ for commands, // for topics)"
          rows={2}
          disabled={isProcessing}
          className="flex-1 px-3 py-2 rounded-md border border-border/50 card-glass text-sm resize-none focus:outline-none focus:ring-1 focus:ring-[hsl(var(--accent-purple))]/50 disabled:opacity-50 placeholder:text-muted-foreground/60"
        />
        {canSend && (
          <button
            onClick={sendMessage}
            className="flex-shrink-0 w-8 h-8 rounded-md bg-[hsl(var(--accent-purple))] hover:bg-[hsl(var(--accent-purple))]/90 text-white flex items-center justify-center transition-colors"
          >
            <Send className="w-3.5 h-3.5" />
          </button>
        )}
      </div>
      <div className="mt-1.5 px-1">
        <span className="text-[11px] text-muted-foreground/60">
          {displayState.type === 'commandList' && '↑↓ select  ↵ execute  esc close'}
          {displayState.type === 'topicList' && '↑↓ select  ↵ switch  esc close'}
          {displayState.type === 'empty' && '↵ send  esc close'}
          {displayState.type === 'conversation' && '↵ send  ⇧↵ new line  esc close'}
        </span>
      </div>
    </div>
  );
}
