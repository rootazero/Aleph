import { ArcSpinner } from '@/components/ui/arc-spinner';

interface HaloProcessingProps {
  provider?: string;
  content?: string;
}

export function HaloProcessing({ provider, content }: HaloProcessingProps) {
  return (
    <div className="flex items-center gap-3 min-w-[200px]">
      <ArcSpinner size={16} />
      <span className="text-sm text-foreground">
        {content || provider || 'Processing...'}
      </span>
    </div>
  );
}
