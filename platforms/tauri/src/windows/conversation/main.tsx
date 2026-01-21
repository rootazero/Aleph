import React, { useEffect, useRef } from 'react';
import ReactDOM from 'react-dom/client';
import '@/styles/globals.css';
import '@/lib/i18n';
import { useConversationStore } from '@/stores/conversationStore';
import { ChatMessage } from './components/ChatMessage';
import { ChatInput } from './components/ChatInput';
import { StreamingMessage } from './components/StreamingMessage';
import { MessageSquare, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useTranslation } from 'react-i18next';

function ConversationWindow() {
  const { t } = useTranslation();
  const {
    messages,
    processingState,
    streamingContent,
    initialize,
    cleanup,
    sendMessage,
    cancelProcessing,
    clearMessages,
  } = useConversationStore();

  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Initialize event listeners
  useEffect(() => {
    initialize().catch((error) => {
      console.error('Failed to initialize conversation:', error);
    });

    return () => {
      cleanup();
    };
  }, [initialize, cleanup]);

  // Auto-scroll to bottom when messages change
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamingContent]);

  const isProcessing =
    processingState.status === 'thinking' ||
    processingState.status === 'streaming' ||
    processingState.status === 'tool';

  return (
    <div className="flex flex-col h-screen bg-background text-foreground">
      {/* Header */}
      <header className="flex items-center justify-between px-4 py-3 border-b border-border">
        <div className="flex items-center gap-2">
          <MessageSquare className="w-5 h-5 text-primary" />
          <h1 className="text-lg font-semibold">{t('conversation.title')}</h1>
        </div>
        <div className="flex items-center gap-2">
          {messages.length > 0 && (
            <Button
              variant="ghost"
              size="icon"
              onClick={clearMessages}
              title={t('conversation.clearMessages')}
            >
              <Trash2 className="w-4 h-4" />
            </Button>
          )}
        </div>
      </header>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto">
        {messages.length === 0 && !isProcessing ? (
          <div className="flex flex-col items-center justify-center h-full text-center p-8">
            <MessageSquare className="w-12 h-12 text-muted-foreground/50 mb-4" />
            <h2 className="text-lg font-medium mb-2">{t('conversation.startPrompt')}</h2>
            <p className="text-sm text-muted-foreground max-w-sm">
              {t('conversation.startDescription')}
            </p>
          </div>
        ) : (
          <div className="pb-4">
            {messages.map((message) => (
              <ChatMessage key={message.id} message={message} />
            ))}

            {/* Streaming indicator */}
            {(processingState.status === 'thinking' ||
              processingState.status === 'streaming') && (
              <StreamingMessage content={streamingContent} />
            )}

            {/* Error display */}
            {processingState.status === 'error' && (
              <div className="mx-4 p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-destructive text-sm">
                {processingState.message}
              </div>
            )}

            <div ref={messagesEndRef} />
          </div>
        )}
      </div>

      {/* Input area */}
      <ChatInput
        onSend={sendMessage}
        onCancel={cancelProcessing}
        isProcessing={isProcessing}
        placeholder={t('conversation.inputPlaceholder')}
      />
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ConversationWindow />
  </React.StrictMode>
);
