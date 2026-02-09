import { ExternalLink, ArrowRight } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useTranslation } from 'react-i18next';

interface MigratedToDashboardProps {
  featureName: string;
  dashboardPath?: string;
}

export function MigratedToDashboard({ featureName, dashboardPath }: MigratedToDashboardProps) {
  const { t } = useTranslation();

  const openDashboard = () => {
    const url = dashboardPath
      ? `http://127.0.0.1:18790/cp${dashboardPath}`
      : 'http://127.0.0.1:18790/cp';
    window.open(url, '_blank');
  };

  return (
    <div className="flex flex-col items-center justify-center h-full min-h-[400px] p-8 text-center">
      <div className="max-w-md space-y-6">
        {/* Icon */}
        <div className="flex justify-center">
          <div className="rounded-full bg-primary/10 p-6">
            <ExternalLink className="h-12 w-12 text-primary" />
          </div>
        </div>

        {/* Title */}
        <div className="space-y-2">
          <h2 className="text-2xl font-semibold tracking-tight">
            {featureName} Configuration
          </h2>
          <p className="text-muted-foreground">
            This feature has been migrated to the Control Panel Dashboard for a better configuration experience.
          </p>
        </div>

        {/* Description */}
        <div className="space-y-3 text-sm text-muted-foreground">
          <p>
            The Control Panel Dashboard provides:
          </p>
          <ul className="space-y-2 text-left">
            <li className="flex items-start gap-2">
              <ArrowRight className="h-4 w-4 mt-0.5 flex-shrink-0 text-primary" />
              <span>Real-time configuration updates across all clients</span>
            </li>
            <li className="flex items-start gap-2">
              <ArrowRight className="h-4 w-4 mt-0.5 flex-shrink-0 text-primary" />
              <span>Advanced configuration options and validation</span>
            </li>
            <li className="flex items-start gap-2">
              <ArrowRight className="h-4 w-4 mt-0.5 flex-shrink-0 text-primary" />
              <span>Unified management for all Aleph instances</span>
            </li>
          </ul>
        </div>

        {/* Action Button */}
        <Button onClick={openDashboard} size="lg" className="gap-2">
          <ExternalLink className="h-4 w-4" />
          Open Control Panel Dashboard
        </Button>

        {/* Help Text */}
        <p className="text-xs text-muted-foreground">
          The Dashboard will open in your default browser at{' '}
          <code className="px-1 py-0.5 rounded bg-muted text-foreground">
            http://127.0.0.1:18790/cp
          </code>
        </p>
      </div>
    </div>
  );
}
