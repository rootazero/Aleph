import { useRef, useEffect } from 'react';
import { motion } from 'framer-motion';
import { User, Bot, Loader2 } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloMessage } from '@/stores/unifiedHaloStore';

interface MessageBubbleProps {
  message: HaloMessage;
}

function MessageBubble({ message }: MessageBubbleProps) {
  const isUser = message.role === 'user';

  return (
    <div
      className={`flex gap-2 ${isUser ? 'flex-row-reverse' : 'flex-row'}`}
    >
      <div
        className={`flex-shrink-0 w-6 h-6 rounded-full flex items-center justify-center ${
          isUser ? 'bg-primary' : 'bg-secondary'
        }`}
      >
        {isUser ? (
          <User className="w-3.5 h-3.5 text-primary-foreground" />
        ) : (
          <Bot className="w-3.5 h-3.5 text-secondary-foreground" />
        )}
      </div>
      <div
        className={`max-w-[80%] px-3 py-2 rounded-lg text-sm ${
          isUser
            ? 'bg-primary text-primary-foreground'
            : 'bg-secondary text-secondary-foreground'
        }`}
      >
        {message.content}
        {message.isStreaming && (
          <span className="inline-block w-1.5 h-4 ml-1 bg-current animate-pulse" />
        )}
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
        className="overflow-y-auto px-3 py-2 space-y-3"
        style={{ maxHeight }}
      >
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        {isProcessing && !messages.some((m) => m.isStreaming) && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>Thinking...</span>
          </div>
        )}
      </div>
    </motion.div>
  );
}
