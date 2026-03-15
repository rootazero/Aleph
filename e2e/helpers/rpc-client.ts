import WebSocket from 'ws';

const DEFAULT_PORT = parseInt(process.env.ALEPH_TEST_PORT || '18791', 10);
const RPC_TIMEOUT_MS = 10_000;

/**
 * WebSocket JSON-RPC client for Aleph server.
 *
 * Each call opens a fresh connection (stateless probe pattern),
 * matching the Rust integration test harness behavior.
 */
export class RpcClient {
  private readonly url: string;

  constructor(port: number = DEFAULT_PORT) {
    this.url = `ws://127.0.0.1:${port}/ws`;
  }

  /**
   * Send a JSON-RPC request and return the full response object.
   */
  async call(method: string, params: object = {}): Promise<any> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      const timer = setTimeout(() => {
        ws.close();
        reject(new Error(`RPC call '${method}' timed out after ${RPC_TIMEOUT_MS}ms`));
      }, RPC_TIMEOUT_MS);

      ws.on('open', () => {
        const request = JSON.stringify({
          jsonrpc: '2.0',
          method,
          params,
          id: 1,
        });
        ws.send(request);
      });

      ws.on('message', (data: WebSocket.Data) => {
        try {
          const msg = JSON.parse(data.toString());
          // JSON-RPC response has "id" field; skip event broadcasts
          if (msg.id !== undefined) {
            clearTimeout(timer);
            ws.close();
            resolve(msg);
          }
        } catch {
          // Non-JSON message — ignore
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timer);
        reject(err);
      });

      ws.on('close', () => {
        clearTimeout(timer);
      });
    });
  }

  /**
   * Send a JSON-RPC request and return the "result" field.
   * Throws if the response contains an error.
   */
  async callOk(method: string, params: object = {}): Promise<any> {
    const response = await this.call(method, params);
    if (response.error) {
      throw new Error(
        `RPC '${method}' returned error: ${JSON.stringify(response.error)}`
      );
    }
    return response.result;
  }
}
