import { useState, useRef, useEffect } from 'react';
import { Send, X } from 'lucide-react';
import { Button } from '@/components/ui/button';

interface HaloConversationInputProps {
  placeholder?: string;
  onSubmit: (input: string) => void;
  onCancel: () => void;
}

export function HaloConversationInput({
  placeholder = 'Type a message...',
  onSubmit,
  onCancel,
}: HaloConversationInputProps) {
  const [value, setValue] = useState('');
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleSubmit = () => {
    if (value.trim()) {
      onSubmit(value.trim());
      setValue('');
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === 'Escape') {
      onCancel();
    }
  };

  return (
    <div className="flex flex-col gap-2 p-3 min-w-[300px] max-w-[450px]">
      <div className="relative">
        <textarea
          ref={inputRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          rows={3}
          className="w-full px-3 py-2 pr-10 rounded-medium border border-input bg-background text-body resize-none focus:outline-none focus:ring-2 focus:ring-ring"
        />
        <button
          onClick={onCancel}
          className="absolute top-2 right-2 p-1 rounded-small hover:bg-secondary transition-colors"
        >
          <X className="w-4 h-4 text-muted-foreground" />
        </button>
      </div>

      <div className="flex items-center justify-between">
        <span className="text-caption text-muted-foreground">
          Press Enter to send, Shift+Enter for new line
        </span>
        <Button size="sm" onClick={handleSubmit} disabled={!value.trim()}>
          <Send className="w-3.5 h-3.5 mr-1.5" />
          Send
        </Button>
      </div>
    </div>
  );
}
