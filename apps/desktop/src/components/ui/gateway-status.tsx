/**
 * Gateway Status Component
 *
 * Shows the current Gateway WebSocket connection status.
 */

import { useEffect } from 'react';
import { useGatewayStore } from '@/stores/gatewayStore';
import { Wifi, WifiOff, Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';

interface GatewayStatusProps {
  className?: string;
  showLabel?: boolean;
  autoConnect?: boolean;
}

export function GatewayStatus({
  className,
  showLabel = false,
  autoConnect = true,
}: GatewayStatusProps) {
  const { connectionState, error, connect } = useGatewayStore();

  useEffect(() => {
    if (autoConnect && connectionState === 'disconnected') {
      connect().catch(() => {
        // Error is stored in the store
      });
    }
  }, [autoConnect, connectionState, connect]);

  const getStatusColor = () => {
    switch (connectionState) {
      case 'connected':
        return 'text-green-500';
      case 'connecting':
        return 'text-yellow-500';
      case 'error':
        return 'text-red-500';
      default:
        return 'text-muted-foreground';
    }
  };

  const getStatusIcon = () => {
    switch (connectionState) {
      case 'connected':
        return <Wifi className="w-4 h-4" />;
      case 'connecting':
        return <Loader2 className="w-4 h-4 animate-spin" />;
      case 'error':
      case 'disconnected':
        return <WifiOff className="w-4 h-4" />;
    }
  };

  const getStatusLabel = () => {
    switch (connectionState) {
      case 'connected':
        return 'Gateway Connected';
      case 'connecting':
        return 'Connecting...';
      case 'error':
        return error || 'Connection Error';
      default:
        return 'Disconnected';
    }
  };

  return (
    <div
      className={cn(
        'flex items-center gap-1.5',
        getStatusColor(),
        className
      )}
      title={getStatusLabel()}
    >
      {getStatusIcon()}
      {showLabel && (
        <span className="text-xs font-medium">{getStatusLabel()}</span>
      )}
    </div>
  );
}
