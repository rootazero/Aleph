import { type ReactNode } from 'react';
import { cn } from '@/lib/utils';

interface CardBaseProps {
  children: ReactNode;
  accentColor?: string;
  className?: string;
}

/**
 * Base card wrapper for system cards in the conversation flow.
 * Provides consistent styling with left accent border and glass background.
 */
export function CardBase({
  children,
  accentColor = 'hsl(var(--accent-purple))',
  className,
}: CardBaseProps) {
  return (
    <div
      className={cn(
        'relative rounded-md bg-card/80 backdrop-blur-sm overflow-hidden',
        className
      )}
    >
      {/* Left accent border */}
      <div
        className="absolute left-0 top-0 bottom-0 w-0.5"
        style={{ backgroundColor: accentColor }}
      />
      {/* Content with left padding to account for border */}
      <div className="pl-3 pr-3 py-2">{children}</div>
    </div>
  );
}
