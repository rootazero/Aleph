import { motion } from 'framer-motion';
import { Bot, Loader2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface StreamingMessageProps {
  content: string;
}

export function StreamingMessage({ content }: StreamingMessageProps) {
  const { t } = useTranslation();

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      className="flex gap-3 p-4"
    >
      {/* Avatar */}
      <div className="flex-shrink-0 w-8 h-8 rounded-full bg-muted flex items-center justify-center">
        <Bot className="w-4 h-4" />
      </div>

      {/* Content */}
      <div className="flex-1 max-w-[80%] rounded-lg p-3 bg-muted">
        {content ? (
          <div className="text-sm whitespace-pre-wrap break-words">
            {content}
            <span className="inline-block w-1 h-4 ml-1 bg-primary animate-pulse" />
          </div>
        ) : (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>{t('halo.thinking')}</span>
          </div>
        )}
      </div>
    </motion.div>
  );
}
