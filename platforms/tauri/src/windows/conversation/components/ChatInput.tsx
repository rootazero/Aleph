import { useState, useRef, useEffect } from 'react';
import { Send, Square, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';

interface ChatInputProps {
  onSend: (message: string) => void;
  onCancel: () => void;
  isProcessing: boolean;
  disabled?: boolean;
  placeholder?: string;
}

export function ChatInput({
  onSend,
  onCancel,
  isProcessing,
  disabled = false,
  placeholder = 'Type a message...',
}: ChatInputProps) {
  const [input, setInput] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-resize textarea
  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = 'auto';
      textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`;
    }
  }, [input]);

  // Focus on mount
  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSubmit = () => {
    const trimmed = input.trim();
    if (!trimmed || isProcessing || disabled) return;

    onSend(trimmed);
    setInput('');
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <div className="border-t border-border bg-background p-4">
      <div className="flex items-end gap-2">
        <div className="flex-1 relative">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            disabled={disabled || isProcessing}
            rows={1}
            className={cn(
              'w-full resize-none rounded-lg border border-input bg-background px-3 py-2',
              'text-sm placeholder:text-muted-foreground',
              'focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
              'disabled:opacity-50 disabled:cursor-not-allowed',
              'min-h-[40px] max-h-[200px]'
            )}
          />
        </div>

        {isProcessing ? (
          <Button
            variant="destructive"
            size="icon"
            onClick={onCancel}
            className="flex-shrink-0"
          >
            <Square className="w-4 h-4" />
          </Button>
        ) : (
          <Button
            variant="default"
            size="icon"
            onClick={handleSubmit}
            disabled={!input.trim() || disabled}
            className="flex-shrink-0"
          >
            <Send className="w-4 h-4" />
          </Button>
        )}
      </div>

      {/* Processing indicator */}
      {isProcessing && (
        <div className="flex items-center gap-2 mt-2 text-xs text-muted-foreground">
          <Loader2 className="w-3 h-3 animate-spin" />
          <ProcessingText />
        </div>
      )}
    </div>
  );
}

function ProcessingText() {
  const { t } = useTranslation();
  return <span>{t('conversation.processing')}</span>;
}
