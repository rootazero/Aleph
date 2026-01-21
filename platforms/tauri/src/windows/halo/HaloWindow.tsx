import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { motion, AnimatePresence } from 'framer-motion';
import { useHaloStore } from '@/stores/haloStore';
import { HaloListening } from './components/HaloListening';
import { HaloSuccess } from './components/HaloSuccess';
import { HaloError } from './components/HaloError';
import { HaloProcessing } from './components/HaloProcessing';

export function HaloWindow() {
  const { state, visible, show, hide } = useHaloStore();

  useEffect(() => {
    // Listen for activation events from Rust
    const unlisten = listen('halo:activate', () => {
      show();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [show]);

  // Handle escape key to hide
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        hide();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [hide]);

  const renderContent = () => {
    switch (state.type) {
      case 'listening':
        return <HaloListening />;
      case 'processingWithAI':
        return <HaloProcessing provider={state.provider} />;
      case 'processing':
        return <HaloProcessing content={state.content} />;
      case 'success':
        return <HaloSuccess message={state.message} />;
      case 'error':
        return (
          <HaloError
            message={state.message}
            canRetry={state.canRetry}
            onRetry={() => show()}
            onClose={hide}
          />
        );
      case 'idle':
      default:
        return null;
    }
  };

  return (
    <div className="halo-window h-screen w-screen flex items-center justify-center p-4">
      <AnimatePresence mode="wait">
        {visible && state.type !== 'idle' && (
          <motion.div
            key={state.type}
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{ duration: 0.15 }}
            className="halo-container"
          >
            {renderContent()}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
