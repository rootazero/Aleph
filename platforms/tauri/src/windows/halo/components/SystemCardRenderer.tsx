import { useEffect, useRef, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import type { SystemCard } from '../types';
import { getCardAccentColor, shouldAutoDismiss, isInteractiveCard } from '../types';
import { CardBase } from './CardBase';
import { HaloProcessing } from './HaloProcessing';
import { HaloSuccess } from './HaloSuccess';
import { HaloError } from './HaloError';
import { HaloToast } from './HaloToast';
import { HaloListening } from './HaloListening';
import { HaloRetrievingMemory } from './HaloRetrievingMemory';
import { HaloTypewriting } from './HaloTypewriting';
import { HaloClarification } from './HaloClarification';
import { HaloToolConfirmation } from './HaloToolConfirmation';
import { HaloPlanConfirmation } from './HaloPlanConfirmation';
import { HaloPlanProgress } from './HaloPlanProgress';
import { HaloTaskGraphConfirmation, HaloTaskGraphProgress } from './HaloTaskGraph';
import { HaloAgentPlan, HaloAgentProgress, HaloAgentConflict } from './HaloAgent';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';

interface SystemCardRendererProps {
  id: string;
  card: SystemCard;
}

const AUTO_DISMISS_DURATION = 3000;
const FADE_OUT_DURATION = 500;

/**
 * Renders the appropriate component for a system card type.
 * Includes animations, keyboard shortcuts, and auto-dismiss.
 */
export function SystemCardRenderer({ id, card }: SystemCardRendererProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [isVisible, setIsVisible] = useState(true);
  const [dismissProgress, setDismissProgress] = useState(100);

  const {
    handleToolConfirmation,
    handlePlanConfirmation,
    handleClarificationResponse,
    handleErrorRetry,
    handleCardDismiss,
  } = useUnifiedHaloStore();

  const accentColor = getCardAccentColor(card);
  const interactive = isInteractiveCard(card);
  const autoDismiss = shouldAutoDismiss(card);

  // Auto-dismiss with fade-out animation
  useEffect(() => {
    if (!autoDismiss) return;

    // Progress countdown animation
    const startTime = Date.now();
    const progressInterval = setInterval(() => {
      const elapsed = Date.now() - startTime;
      const remaining = Math.max(0, 100 - (elapsed / AUTO_DISMISS_DURATION) * 100);
      setDismissProgress(remaining);
    }, 50);

    // Start fade-out before removal
    const fadeTimeout = setTimeout(() => {
      setIsVisible(false);
    }, AUTO_DISMISS_DURATION - FADE_OUT_DURATION);

    // Actually remove after fade completes
    const removeTimeout = setTimeout(() => {
      handleCardDismiss(id);
    }, AUTO_DISMISS_DURATION);

    return () => {
      clearInterval(progressInterval);
      clearTimeout(fadeTimeout);
      clearTimeout(removeTimeout);
    };
  }, [autoDismiss, id, handleCardDismiss]);

  // Keyboard shortcuts for confirmation cards
  useEffect(() => {
    if (!interactive) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      // Only handle if this card's container or its children are focused
      if (!containerRef.current?.contains(document.activeElement) &&
          document.activeElement !== containerRef.current) {
        return;
      }

      if (e.key === 'Enter') {
        e.preventDefault();
        handleConfirm();
      } else if (e.key === 'Escape') {
        e.preventDefault();
        handleCancel();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [card, id, interactive]);

  // Focus the card when it appears if it's interactive
  useEffect(() => {
    if (interactive && containerRef.current) {
      containerRef.current.focus();
    }
  }, [interactive]);

  const handleConfirm = () => {
    switch (card.type) {
      case 'toolConfirmation':
        handleToolConfirmation(id, true);
        break;
      case 'planConfirmation':
        handlePlanConfirmation(id, true);
        break;
      case 'taskGraphConfirmation':
      case 'agentPlan':
        handleCardDismiss(id);
        break;
    }
  };

  const handleCancel = () => {
    switch (card.type) {
      case 'toolConfirmation':
        handleToolConfirmation(id, false);
        break;
      case 'planConfirmation':
        handlePlanConfirmation(id, false);
        break;
      case 'error':
        handleCardDismiss(id);
        break;
      default:
        handleCardDismiss(id);
    }
  };

  const renderContent = () => {
    switch (card.type) {
      case 'processing':
        return <HaloProcessing provider={card.provider} content={card.content} />;

      case 'success':
        return (
          <div className="relative">
            <HaloSuccess message={card.message} />
            {/* Dismiss progress indicator */}
            <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-secondary/30 overflow-hidden rounded-full">
              <motion.div
                className="h-full bg-[hsl(var(--success))]/50"
                initial={{ width: '100%' }}
                animate={{ width: `${dismissProgress}%` }}
                transition={{ duration: 0.05, ease: 'linear' }}
              />
            </div>
          </div>
        );

      case 'error':
        return (
          <HaloError
            message={card.message}
            canRetry={card.canRetry}
            onRetry={() => handleErrorRetry(id)}
            onClose={() => handleCardDismiss(id)}
          />
        );

      case 'toast':
        return (
          <div className="relative">
            <HaloToast message={card.message} level={card.level} />
            {/* Dismiss progress indicator */}
            <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-secondary/30 overflow-hidden rounded-full">
              <motion.div
                className="h-full"
                style={{ backgroundColor: `${accentColor}50` }}
                initial={{ width: '100%' }}
                animate={{ width: `${dismissProgress}%` }}
                transition={{ duration: 0.05, ease: 'linear' }}
              />
            </div>
          </div>
        );

      case 'listening':
        return <HaloListening />;

      case 'retrievingMemory':
        return <HaloRetrievingMemory />;

      case 'typewriting':
        return <HaloTypewriting content={card.content} progress={card.progress} />;

      case 'clarification':
        return (
          <HaloClarification
            question={card.question}
            options={card.options}
            onSubmit={(response) => handleClarificationResponse(id, response)}
            onCancel={() => handleCardDismiss(id)}
          />
        );

      case 'toolConfirmation':
        return (
          <HaloToolConfirmation
            tool={card.tool}
            args={card.args}
            onConfirm={() => handleToolConfirmation(id, true)}
            onCancel={() => handleToolConfirmation(id, false)}
          />
        );

      case 'planConfirmation':
        return (
          <HaloPlanConfirmation
            steps={card.steps}
            onConfirm={() => handlePlanConfirmation(id, true)}
            onCancel={() => handlePlanConfirmation(id, false)}
          />
        );

      case 'planProgress':
        return <HaloPlanProgress steps={card.steps} currentIndex={card.currentIndex} />;

      case 'taskGraphConfirmation':
        return (
          <HaloTaskGraphConfirmation
            graph={card.graph}
            onConfirm={() => handleCardDismiss(id)}
            onCancel={() => handleCardDismiss(id)}
          />
        );

      case 'taskGraphProgress':
        return <HaloTaskGraphProgress graph={card.graph} />;

      case 'agentPlan':
        return (
          <HaloAgentPlan
            plan={card.plan}
            onConfirm={() => handleCardDismiss(id)}
            onCancel={() => handleCardDismiss(id)}
          />
        );

      case 'agentProgress':
        return <HaloAgentProgress progress={card.progress} />;

      case 'agentConflict':
        return (
          <HaloAgentConflict
            conflict={card.conflict}
            onSelect={(optionId) => {
              console.log('Selected option:', optionId);
              handleCardDismiss(id);
            }}
            onCancel={() => handleCardDismiss(id)}
          />
        );

      default:
        return null;
    }
  };

  // Cards that have their own padding, skip CardBase wrapper
  const skipWrapper =
    card.type === 'listening' ||
    card.type === 'clarification' ||
    card.type === 'toolConfirmation' ||
    card.type === 'planConfirmation' ||
    card.type === 'planProgress' ||
    card.type === 'taskGraphConfirmation' ||
    card.type === 'taskGraphProgress' ||
    card.type === 'agentPlan' ||
    card.type === 'agentProgress' ||
    card.type === 'agentConflict' ||
    card.type === 'error';

  const cardContent = skipWrapper ? (
    <div
      className="rounded-md bg-card/80 backdrop-blur-sm overflow-hidden"
      style={{ borderLeft: `2px solid ${accentColor}` }}
    >
      {renderContent()}
    </div>
  ) : (
    <CardBase accentColor={accentColor}>{renderContent()}</CardBase>
  );

  return (
    <AnimatePresence>
      {isVisible && (
        <motion.div
          ref={containerRef}
          tabIndex={interactive ? 0 : -1}
          initial={{ opacity: 0, scale: 0.95, y: 10 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: -10 }}
          transition={{
            duration: 0.2,
            ease: [0.4, 0, 0.2, 1],
          }}
          className="focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 rounded-md"
        >
          {cardContent}
          {/* Keyboard hints for interactive cards */}
          {interactive && (
            <div className="flex justify-end gap-2 mt-1 px-1">
              <span className="text-[10px] text-muted-foreground/50">
                ↵ confirm · esc cancel
              </span>
            </div>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
