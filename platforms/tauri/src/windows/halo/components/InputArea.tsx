import { useRef, useEffect } from 'react';
import { Send, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
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
    hide,
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

  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="relative">
        <textarea
          ref={inputRef}
          value={inputText}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder="Type a message... (/ for commands, // for topics)"
          rows={2}
          disabled={isProcessing}
          className="w-full px-3 py-2 pr-10 rounded-lg border border-input bg-background/80 backdrop-blur-sm text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-50"
        />
        <button
          onClick={hide}
          className="absolute top-2 right-2 p-1 rounded hover:bg-secondary/80 transition-colors"
        >
          <X className="w-4 h-4 text-muted-foreground" />
        </button>
      </div>

      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">
          {displayState.type === 'commandList' && 'Select command with ↑↓, Enter to execute'}
          {displayState.type === 'topicList' && 'Select topic with ↑↓, Enter to switch'}
          {displayState.type === 'empty' && 'Enter to send, Esc to close'}
          {displayState.type === 'conversation' && 'Enter to send, Shift+Enter for new line'}
        </span>
        {displayState.type !== 'commandList' && displayState.type !== 'topicList' && (
          <Button
            size="sm"
            onClick={sendMessage}
            disabled={!inputText.trim() || inputText.startsWith('/') || isProcessing}
          >
            <Send className="w-3.5 h-3.5 mr-1.5" />
            Send
          </Button>
        )}
      </div>
    </div>
  );
}
