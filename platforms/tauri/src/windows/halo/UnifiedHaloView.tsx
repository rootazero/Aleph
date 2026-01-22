import { useEffect, useRef } from 'react';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { motion, AnimatePresence } from 'framer-motion';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import { InputArea } from './components/InputArea';
import { CommandList } from './components/CommandList';
import { TopicList } from './components/TopicList';
import { ConversationArea } from './components/ConversationArea';

// Layout constants
const LAYOUT = {
  WIDTH: 600,
  INPUT_HEIGHT: 80,
  MAX_CONTENT_HEIGHT: 300,
  PADDING: 16,
};

export function UnifiedHaloView() {
  const containerRef = useRef<HTMLDivElement>(null);
  const {
    displayState,
    visible,
    show,
    handleEscape,
    initialize,
    cleanup,
  } = useUnifiedHaloStore();

  // Initialize store
  useEffect(() => {
    initialize().catch(console.error);
    return () => cleanup();
  }, [initialize, cleanup]);

  // Listen for activation events
  useEffect(() => {
    const unlisten = listen('halo:activate', () => {
      show();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [show]);

  // Handle escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        handleEscape();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleEscape]);

  // Auto-resize window based on content
  useEffect(() => {
    if (!containerRef.current) return;

    const resizeObserver = new ResizeObserver(async (entries) => {
      for (const entry of entries) {
        const { height } = entry.contentRect;
        if (height > 0) {
          try {
            const appWindow = getCurrentWindow();
            await appWindow.setSize(
              new LogicalSize(LAYOUT.WIDTH, Math.ceil(height) + LAYOUT.PADDING)
            );
          } catch (error) {
            console.error('Failed to resize window:', error);
          }
        }
      }
    });

    resizeObserver.observe(containerRef.current);
    return () => resizeObserver.disconnect();
  }, []);

  // Render the appropriate content panel
  const renderContentPanel = () => {
    switch (displayState.type) {
      case 'commandList':
        return <CommandList maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'topicList':
        return <TopicList maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'conversation':
        return <ConversationArea maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'empty':
      default:
        return null;
    }
  };

  return (
    <div className="h-screen w-screen flex items-start justify-center p-2">
      <motion.div
        ref={containerRef}
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        className="w-full max-w-[600px] bg-background/95 backdrop-blur-xl border border-border/50 rounded-xl shadow-2xl overflow-hidden"
      >
        {/* Content panel (conversation/commands/topics) */}
        <AnimatePresence mode="wait">
          {displayState.type !== 'empty' && (
            <motion.div
              key={displayState.type}
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.15 }}
            >
              {renderContentPanel()}
            </motion.div>
          )}
        </AnimatePresence>

        {/* Divider */}
        {displayState.type !== 'empty' && (
          <div className="h-px bg-border/50" />
        )}

        {/* Input area (always visible) */}
        <InputArea />
      </motion.div>
    </div>
  );
}
