import { useRef, useEffect } from 'react';
import { motion } from 'framer-motion';
import { User } from 'lucide-react';
import { ArcSpinner } from '@/components/ui/arc-spinner';
import { SystemCardRenderer } from './SystemCardRenderer';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloMessage } from '@/stores/unifiedHaloStore';

interface MessageBubbleProps {
  message: HaloMessage;
}

function MessageBubble({ message }: MessageBubbleProps) {
  // Handle system cards
  if (message.role === 'system') {
    return <SystemCardRenderer id={message.id} card={message.card} />;
  }

  const isUser = message.role === 'user';

  if (isUser) {
    return (
      <div className="flex justify-end">
        <div className="flex items-start gap-2 max-w-[80%] flex-row-reverse">
          <div className="flex-shrink-0 w-6 h-6 rounded-full bg-primary/10 flex items-center justify-center">
            <User className="w-3.5 h-3.5 text-primary" />
          </div>
          <div className="px-3 py-2 rounded-lg text-sm bg-primary text-primary-foreground">
            {message.content}
          </div>
        </div>
      </div>
    );
  }

  // AI message with purple left accent line
  return (
    <div className="flex">
      <div className="flex max-w-[85%]">
        <div className="w-0.5 rounded-full bg-[hsl(var(--accent-purple))] flex-shrink-0 mr-3" />
        <div className="px-3 py-2 rounded-lg text-sm bg-secondary/60 backdrop-blur-sm text-foreground">
          {message.content}
          {'isStreaming' in message && message.isStreaming && (
            <span className="inline-flex items-center ml-1.5 align-middle">
              <ArcSpinner size={12} />
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

interface ConversationAreaProps {
  maxHeight?: number;
}

export function ConversationArea({ maxHeight = 300 }: ConversationAreaProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const { messages, isProcessing } = useUnifiedHaloStore();

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  if (messages.length === 0) {
    return null;
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        ref={scrollRef}
        className="overflow-y-auto px-3 py-3 space-y-3"
        style={{ maxHeight }}
      >
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        {isProcessing && !messages.some((m) => m.role === 'assistant' && 'isStreaming' in m && m.isStreaming) && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground pl-1">
            <ArcSpinner size={16} />
            <span>Thinking...</span>
          </div>
        )}
      </div>
    </motion.div>
  );
}
