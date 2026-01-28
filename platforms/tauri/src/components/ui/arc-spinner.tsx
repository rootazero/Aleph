import { cn } from '@/lib/utils';

interface ArcSpinnerProps {
  size?: number;
  className?: string;
  color?: string;
}

/**
 * macOS-style gradient arc spinner.
 * Renders a rotating arc with a gradient from transparent to the accent color.
 */
export function ArcSpinner({
  size = 16,
  className,
  color = 'hsl(var(--accent-purple))',
}: ArcSpinnerProps) {
  const strokeWidth = 2;
  const radius = (size - strokeWidth) / 2;
  const center = size / 2;
  // 252° arc (70% of circle)
  const circumference = 2 * Math.PI * radius;
  const arcLength = circumference * 0.7;
  const gapLength = circumference - arcLength;

  const gradientId = `arc-spinner-gradient-${size}`;

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      className={cn('arc-spinner', className)}
      style={{ ['--arc-color' as string]: color }}
    >
      <defs>
        <linearGradient id={gradientId} x1="0%" y1="0%" x2="100%" y2="0%">
          <stop offset="0%" stopColor="currentColor" stopOpacity="0" />
          <stop offset="10%" stopColor="currentColor" stopOpacity="0.1" />
          <stop offset="40%" stopColor="currentColor" stopOpacity="0.4" />
          <stop offset="70%" stopColor="currentColor" stopOpacity="0.7" />
          <stop offset="100%" stopColor="currentColor" stopOpacity="1" />
        </linearGradient>
      </defs>
      <circle
        cx={center}
        cy={center}
        r={radius}
        fill="none"
        stroke={`url(#${gradientId})`}
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeDasharray={`${arcLength} ${gapLength}`}
        style={{ color }}
      />
    </svg>
  );
}
