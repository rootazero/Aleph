import { motion } from 'framer-motion';
import { User, Bot, Wrench, Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { Message } from '@/stores/conversationStore';

interface ChatMessageProps {
  message: Message;
}

export function ChatMessage({ message }: ChatMessageProps) {
  const isUser = message.role === 'user';
  const isTool = message.role === 'tool';
  const isAssistant = message.role === 'assistant';

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      className={cn(
        'flex gap-3 p-4',
        isUser && 'flex-row-reverse'
      )}
    >
      {/* Avatar */}
      <div
        className={cn(
          'flex-shrink-0 w-8 h-8 rounded-full flex items-center justify-center',
          isUser && 'bg-primary text-primary-foreground',
          isAssistant && 'bg-muted',
          isTool && 'bg-yellow-500/10 text-yellow-500'
        )}
      >
        {isUser && <User className="w-4 h-4" />}
        {isAssistant && <Bot className="w-4 h-4" />}
        {isTool && <Wrench className="w-4 h-4" />}
      </div>

      {/* Content */}
      <div
        className={cn(
          'flex-1 max-w-[80%] rounded-lg p-3',
          isUser && 'bg-primary text-primary-foreground',
          isAssistant && 'bg-muted',
          isTool && 'bg-yellow-500/10 border border-yellow-500/20'
        )}
      >
        {/* Tool header */}
        {isTool && message.toolName && (
          <div className="flex items-center gap-2 text-xs text-yellow-500 mb-2">
            <Wrench className="w-3 h-3" />
            <span>{message.toolName}</span>
            {message.isStreaming && <Loader2 className="w-3 h-3 animate-spin" />}
          </div>
        )}

        {/* Message content */}
        <div className="text-sm whitespace-pre-wrap break-words">
          {message.content}
        </div>

        {/* Timestamp */}
        <div
          className={cn(
            'text-xs mt-2 opacity-50',
            isUser && 'text-right'
          )}
        >
          {new Date(message.timestamp).toLocaleTimeString()}
        </div>
      </div>
    </motion.div>
  );
}
