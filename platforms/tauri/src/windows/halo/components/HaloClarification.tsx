import { useState } from 'react';
import { motion } from 'framer-motion';
import { HelpCircle, Send } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

interface HaloClarificationProps {
  question: string;
  options?: string[];
  onSubmit: (response: string) => void;
  onCancel: () => void;
}

export function HaloClarification({
  question,
  options,
  onSubmit,
  onCancel,
}: HaloClarificationProps) {
  const [inputValue, setInputValue] = useState('');
  const [selectedOption, setSelectedOption] = useState<string | null>(null);

  const handleSubmit = () => {
    const response = selectedOption || inputValue;
    if (response.trim()) {
      onSubmit(response);
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
    <div className="flex flex-col gap-3 p-3 min-w-[260px] max-w-[380px]">
      {/* Question */}
      <div className="flex items-start gap-2">
        <HelpCircle className="w-4 h-4 text-[hsl(var(--info))] flex-shrink-0 mt-0.5" />
        <p className="text-sm text-foreground">{question}</p>
      </div>

      {/* Options or Input */}
      {options && options.length > 0 ? (
        <div className="flex flex-col gap-1">
          {options.map((option, index) => (
            <motion.button
              key={index}
              initial={{ opacity: 0, x: -10 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ delay: index * 0.05 }}
              onClick={() => setSelectedOption(option)}
              className={cn(
                'text-left px-2.5 py-1.5 rounded-md text-sm transition-colors',
                selectedOption === option
                  ? 'bg-[hsl(var(--accent-blue))] text-white'
                  : 'bg-secondary hover:bg-secondary/80'
              )}
            >
              {option}
            </motion.button>
          ))}
        </div>
      ) : (
        <input
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Type your response..."
          autoFocus
          className="w-full px-2.5 py-1.5 rounded-md border border-border bg-background text-sm focus:outline-none focus:ring-1 focus:ring-[hsl(var(--accent-blue))]/50"
        />
      )}

      {/* Actions */}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          size="sm"
          onClick={handleSubmit}
          disabled={!selectedOption && !inputValue.trim()}
        >
          <Send className="w-3.5 h-3.5 mr-1.5" />
          Submit
        </Button>
      </div>
    </div>
  );
}
