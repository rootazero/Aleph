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
  WIDTH: 800,
  INPUT_HEIGHT: 80,
  MAX_CONTENT_HEIGHT: 500,
  MAX_WINDOW_HEIGHT: 600,
  PADDING: 16,
};

export function UnifiedHaloView() {
  const containerRef = useRef<HTMLDivElement>(null);
  const {
    displayState,
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

    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;

    const resizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { height } = entry.contentRect;
        console.log('[Halo] ResizeObserver triggered, height:', height);

        if (height > 0) {
          // Debounce resize to avoid rapid updates during animation
          if (resizeTimeout) clearTimeout(resizeTimeout);
          resizeTimeout = setTimeout(async () => {
            try {
              const appWindow = getCurrentWindow();
              const newHeight = Math.min(
                Math.ceil(height) + LAYOUT.PADDING,
                LAYOUT.MAX_WINDOW_HEIGHT
              );
              console.log('[Halo] Setting window size:', LAYOUT.WIDTH, 'x', newHeight);
              await appWindow.setSize(new LogicalSize(LAYOUT.WIDTH, newHeight));
            } catch (error) {
              console.error('[Halo] Failed to resize window:', error);
            }
          }, 50);
        }
      }
    });

    resizeObserver.observe(containerRef.current);
    return () => {
      if (resizeTimeout) clearTimeout(resizeTimeout);
      resizeObserver.disconnect();
    };
  }, []);

  // Render the appropriate content panel
  const renderContentPanel = () => {
    switch (displayState.type) {
      case 'commandList':
        return <CommandList key="commandList" maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'topicList':
        return <TopicList key="topicList" maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'conversation':
        return <ConversationArea key="conversation" maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'empty':
      default:
        return null;
    }
  };

  return (
    <div className="w-full flex items-start justify-center p-2">
      <motion.div
        ref={containerRef}
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        className="w-full max-w-[800px] bg-background/95 backdrop-blur-xl border border-border/50 rounded-xl shadow-2xl"
      >
        {/* Content panel (conversation/commands/topics) */}
        <AnimatePresence mode="wait">
          {displayState.type !== 'empty' && renderContentPanel()}
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
