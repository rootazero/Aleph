import { RpcClient } from './rpc-client';

const rpc = new RpcClient();

/**
 * Remove all providers except the built-in "test" provider.
 * Call this before tests that mutate provider state.
 */
export async function cleanProviders(): Promise<void> {
  const result = await rpc.callOk('providers.list', {});
  const providers = result?.providers || [];

  for (const provider of providers) {
    const name = provider.name || provider.id;
    if (name && name !== 'test') {
      try {
        await rpc.callOk('providers.delete', { name });
      } catch {
        // Ignore errors (e.g., default provider protection)
      }
    }
  }
}

/**
 * Inject a provider via RPC.
 */
export async function injectProvider(
  name: string,
  config: {
    protocol?: string;
    models?: string[];
    base_url?: string;
    api_key?: string;
    enabled?: boolean;
  } = {}
): Promise<any> {
  return rpc.callOk('providers.create', {
    name,
    protocol: config.protocol || 'openai',
    models: config.models || ['test-model'],
    base_url: config.base_url || 'http://localhost:1/v1',
    api_key: config.api_key || '',
    enabled: config.enabled !== undefined ? config.enabled : true,
  });
}

/**
 * List all providers via RPC.
 */
export async function getProviders(): Promise<any[]> {
  const result = await rpc.callOk('providers.list', {});
  return result?.providers || [];
}

/**
 * Get a specific provider by name via RPC.
 */
export async function getProvider(name: string): Promise<any> {
  return rpc.callOk('providers.get', { name });
}

/**
 * Delete a specific provider by name via RPC.
 */
export async function deleteProvider(name: string): Promise<any> {
  return rpc.callOk('providers.delete', { name });
}
