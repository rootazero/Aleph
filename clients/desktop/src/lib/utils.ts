import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

/**
 * Merge Tailwind CSS classes with clsx
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * Format a keyboard shortcut for display
 */
export function formatShortcut(modifiers: string[], key: string): string {
  const symbolMap: Record<string, string> = {
    command: '⌘',
    cmd: '⌘',
    control: '⌃',
    ctrl: '⌃',
    option: '⌥',
    alt: '⌥',
    shift: '⇧',
  };

  const formattedModifiers = modifiers.map(
    (mod) => symbolMap[mod.toLowerCase()] || mod
  );

  return [...formattedModifiers, key.toUpperCase()].join(' ');
}

/**
 * Debounce function
 */
export function debounce<T extends (...args: unknown[]) => unknown>(
  fn: T,
  delay: number
): (...args: Parameters<T>) => void {
  let timeoutId: ReturnType<typeof setTimeout>;
  return (...args: Parameters<T>) => {
    clearTimeout(timeoutId);
    timeoutId = setTimeout(() => fn(...args), delay);
  };
}

/**
 * Sleep for a given number of milliseconds
 */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
