import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { motion, AnimatePresence } from 'framer-motion';
import { useHaloStore } from '@/stores/haloStore';

// Import all Halo components
import { HaloListening } from './components/HaloListening';
import { HaloRetrievingMemory } from './components/HaloRetrievingMemory';
import { HaloProcessing } from './components/HaloProcessing';
import { HaloTypewriting } from './components/HaloTypewriting';
import { HaloSuccess } from './components/HaloSuccess';
import { HaloError } from './components/HaloError';
import { HaloToast } from './components/HaloToast';
import { HaloClarification } from './components/HaloClarification';
import { HaloConversationInput } from './components/HaloConversationInput';
import { HaloToolConfirmation } from './components/HaloToolConfirmation';
import { HaloPlanConfirmation } from './components/HaloPlanConfirmation';
import { HaloPlanProgress } from './components/HaloPlanProgress';
import { HaloTaskGraphConfirmation, HaloTaskGraphProgress } from './components/HaloTaskGraph';
import { HaloAgentPlan, HaloAgentProgress, HaloAgentConflict } from './components/HaloAgent';

export function HaloWindow() {
  const containerRef = useRef<HTMLDivElement>(null);
  const {
    state,
    visible,
    show,
    hide,
    initialize,
    cleanup,
    confirmTool,
    confirmPlan,
    confirmTaskGraph,
    confirmAgent,
    resolveConflict,
    submitClarification,
    submitConversation,
  } = useHaloStore();

  // Initialize AI event listeners
  useEffect(() => {
    initialize().catch((error) => {
      console.error('Failed to initialize Halo AI listeners:', error);
    });

    return () => {
      cleanup();
    };
  }, [initialize, cleanup]);

  // Listen for activation events from Rust
  useEffect(() => {
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

  // Auto-resize window based on content
  useEffect(() => {
    if (!containerRef.current || !visible) return;

    const resizeObserver = new ResizeObserver(async (entries) => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect;
        if (width > 0 && height > 0) {
          try {
            const appWindow = getCurrentWindow();
            // Add padding for shadow and border
            await appWindow.setSize(
              new LogicalSize(Math.ceil(width) + 32, Math.ceil(height) + 32)
            );
          } catch (error) {
            console.error('Failed to resize window:', error);
          }
        }
      }
    });

    resizeObserver.observe(containerRef.current);
    return () => resizeObserver.disconnect();
  }, [visible]);

  const renderContent = () => {
    switch (state.type) {
      case 'listening':
        return <HaloListening />;

      case 'retrievingMemory':
        return <HaloRetrievingMemory />;

      case 'processingWithAI':
        return <HaloProcessing provider={state.provider} />;

      case 'processing':
        return <HaloProcessing content={state.content} />;

      case 'typewriting':
        return <HaloTypewriting content={state.content} progress={state.progress} />;

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

      case 'toast':
        return <HaloToast message={state.message} level={state.level} />;

      case 'clarification':
        return (
          <HaloClarification
            question={state.question}
            options={state.options}
            onSubmit={submitClarification}
            onCancel={hide}
          />
        );

      case 'conversationInput':
        return (
          <HaloConversationInput
            placeholder={state.placeholder}
            onSubmit={submitConversation}
            onCancel={hide}
          />
        );

      case 'toolConfirmation':
        return (
          <HaloToolConfirmation
            tool={state.tool}
            args={state.args}
            onConfirm={() => confirmTool(true)}
            onCancel={() => confirmTool(false)}
          />
        );

      case 'planConfirmation':
        return (
          <HaloPlanConfirmation
            steps={state.steps}
            onConfirm={() => confirmPlan(true)}
            onCancel={() => confirmPlan(false)}
          />
        );

      case 'planProgress':
        return (
          <HaloPlanProgress steps={state.steps} currentIndex={state.currentIndex} />
        );

      case 'taskGraphConfirmation':
        return (
          <HaloTaskGraphConfirmation
            graph={state.graph}
            onConfirm={() => confirmTaskGraph(true)}
            onCancel={() => confirmTaskGraph(false)}
          />
        );

      case 'taskGraphProgress':
        return <HaloTaskGraphProgress graph={state.graph} />;

      case 'agentPlan':
        return (
          <HaloAgentPlan
            plan={state.plan}
            onConfirm={() => confirmAgent(true)}
            onCancel={() => confirmAgent(false)}
          />
        );

      case 'agentProgress':
        return <HaloAgentProgress progress={state.progress} />;

      case 'agentConflict':
        return (
          <HaloAgentConflict
            conflict={state.conflict}
            onSelect={resolveConflict}
            onCancel={hide}
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
            ref={containerRef}
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
