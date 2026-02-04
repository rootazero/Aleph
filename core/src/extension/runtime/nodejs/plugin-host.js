/**
 * Aleph Plugin Host for Node.js
 *
 * This script runs as a subprocess and communicates with the Rust runtime
 * via JSON-RPC 2.0 over stdin/stdout.
 *
 * Supported methods:
 * - load: Load a plugin module and call its register() function
 * - plugin.call: Call a handler function on a loaded plugin
 * - executeHook: Execute a hook handler on a loaded plugin
 * - unload: Unload a plugin
 * - shutdown: Exit the process
 */

'use strict';

const readline = require('readline');
const path = require('path');
const fs = require('fs');
const { pathToFileURL } = require('url');

// Map of loaded plugins: pluginId -> { module, handlers }
const loadedPlugins = new Map();

// Get allowed plugins directory from environment or default
const PLUGINS_DIR = process.env.ALEPH_PLUGINS_DIR
    ? path.resolve(process.env.ALEPH_PLUGINS_DIR)
    : path.resolve(process.env.HOME || process.env.USERPROFILE || '', '.aleph', 'plugins');

// JSON-RPC 2.0 error codes
const ERROR_CODES = {
    PARSE_ERROR: -32700,
    INVALID_REQUEST: -32600,
    METHOD_NOT_FOUND: -32601,
    INVALID_PARAMS: -32602,
    INTERNAL_ERROR: -32603,
    PLUGIN_NOT_FOUND: -32000,
    HANDLER_NOT_FOUND: -32001,
    PLUGIN_LOAD_ERROR: -32002,
    PATH_TRAVERSAL: -32003,
};

/**
 * Create a JSON-RPC 2.0 success response
 */
function successResponse(id, result) {
    return {
        jsonrpc: '2.0',
        id: id,
        result: result !== undefined ? result : null,
    };
}

/**
 * Create a JSON-RPC 2.0 error response
 */
function errorResponse(id, code, message, data) {
    const response = {
        jsonrpc: '2.0',
        id: id,
        error: {
            code: code,
            message: message,
        },
    };
    if (data !== undefined) {
        response.error.data = data;
    }
    return response;
}

/**
 * Validate that a path is within the allowed plugins directory
 * @param {string} resolvedPath - Fully resolved path to validate
 * @returns {boolean} - True if path is within allowed directory
 */
function isPathAllowed(resolvedPath) {
    // Normalize both paths and ensure the plugin path is within PLUGINS_DIR
    const normalizedPluginsDir = path.normalize(PLUGINS_DIR) + path.sep;
    const normalizedPath = path.normalize(resolvedPath);

    // Check if the path starts with the plugins directory
    return normalizedPath.startsWith(normalizedPluginsDir) || normalizedPath === path.normalize(PLUGINS_DIR);
}

/**
 * Validate that a path points to a file (not a directory)
 * @param {string} resolvedPath - Path to check
 * @returns {boolean} - True if path is a file
 */
function isFile(resolvedPath) {
    try {
        const stats = fs.statSync(resolvedPath);
        return stats.isFile();
    } catch {
        return false;
    }
}

/**
 * Load a plugin module and call its register() function
 *
 * @param {object} params - { pluginId, pluginPath }
 * @returns {object} Registration data with tools, hooks, etc.
 */
async function handleLoad(params) {
    const { pluginId, pluginPath } = params;

    if (!pluginId || !pluginPath) {
        throw { code: ERROR_CODES.INVALID_PARAMS, message: 'Missing pluginId or pluginPath' };
    }

    // Resolve the plugin path
    const resolvedPath = path.resolve(pluginPath);

    // Validate path is within allowed plugins directory FIRST
    if (!isPathAllowed(resolvedPath)) {
        throw {
            code: ERROR_CODES.PATH_TRAVERSAL,
            message: `Path not allowed: must be within plugins directory`,
            data: { pluginsDir: PLUGINS_DIR },
        };
    }

    // Validate that path points to a file
    if (!isFile(resolvedPath)) {
        throw {
            code: ERROR_CODES.PLUGIN_LOAD_ERROR,
            message: `Plugin entry must be a file: ${resolvedPath}`,
        };
    }

    try {
        // Clear require cache if reloading (only after path validation)
        try {
            delete require.cache[require.resolve(resolvedPath)];
        } catch {
            // Ignore if not in cache
        }

        // Load the plugin module using dynamic import for ESM support
        const fileUrl = pathToFileURL(resolvedPath).href;
        const pluginModule = await import(fileUrl);

        // Initialize registration result
        const registration = {
            plugin_id: pluginId,
            tools: [],
            hooks: [],
            channels: [],
            providers: [],
            gateway_methods: [],
        };

        // Collect ONLY explicitly registered handlers
        const handlers = {};

        // Call register() if it exists
        const registerFn = pluginModule.register || (pluginModule.default && pluginModule.default.register);
        if (typeof registerFn === 'function') {
            const registerResult = await registerFn({
                // Tool registration helper
                registerTool: (name, description, parameters, handler) => {
                    const handlerName = `tool_${name}`;
                    handlers[handlerName] = handler;
                    registration.tools.push({
                        name: name,
                        description: description,
                        parameters: parameters,
                        handler: handlerName,
                    });
                },
                // Hook registration helper
                registerHook: (event, handler, priority = 0) => {
                    const handlerName = `hook_${event}_${registration.hooks.length}`;
                    handlers[handlerName] = handler;
                    registration.hooks.push({
                        event: event,
                        priority: priority,
                        handler: handlerName,
                    });
                },
                // Channel registration helper
                registerChannel: (id, label) => {
                    registration.channels.push({ id, label });
                },
                // Provider registration helper
                registerProvider: (id, name, models) => {
                    registration.providers.push({ id, name, models });
                },
                // Gateway method registration helper
                registerGatewayMethod: (method, handler) => {
                    const handlerName = `gateway_${method}`;
                    handlers[handlerName] = handler;
                    registration.gateway_methods.push({
                        method: method,
                        handler: handlerName,
                    });
                },
            });

            // Merge any returned registration data (but NOT arbitrary handlers)
            if (registerResult && typeof registerResult === 'object') {
                if (registerResult.tools) registration.tools.push(...registerResult.tools);
                if (registerResult.hooks) registration.hooks.push(...registerResult.hooks);
                if (registerResult.channels) registration.channels.push(...registerResult.channels);
                if (registerResult.providers) registration.providers.push(...registerResult.providers);
                if (registerResult.gateway_methods) registration.gateway_methods.push(...registerResult.gateway_methods);
            }
        }

        // NOTE: We intentionally do NOT expose all exported functions from the module.
        // Only handlers registered via registerTool/registerHook/registerGatewayMethod are callable.
        // This prevents arbitrary code execution through unregistered module exports.

        // Store the loaded plugin with ONLY registered handlers
        loadedPlugins.set(pluginId, {
            module: pluginModule,
            handlers: handlers,
            path: resolvedPath,
        });

        return registration;
    } catch (err) {
        throw {
            code: ERROR_CODES.PLUGIN_LOAD_ERROR,
            message: `Failed to load plugin: ${err.message}`,
            data: { stack: err.stack },
        };
    }
}

/**
 * Call a handler function on a loaded plugin
 *
 * @param {object} params - { pluginId, handler, args }
 * @returns {*} Handler result
 */
async function handlePluginCall(params) {
    const { pluginId, handler, args } = params;

    if (!pluginId || !handler) {
        throw { code: ERROR_CODES.INVALID_PARAMS, message: 'Missing pluginId or handler' };
    }

    const plugin = loadedPlugins.get(pluginId);
    if (!plugin) {
        throw { code: ERROR_CODES.PLUGIN_NOT_FOUND, message: `Plugin not found: ${pluginId}` };
    }

    // SECURITY: Only allow calling explicitly registered handlers
    const handlerFn = plugin.handlers[handler];
    if (typeof handlerFn !== 'function') {
        throw { code: ERROR_CODES.HANDLER_NOT_FOUND, message: `Handler not found: ${handler}` };
    }

    try {
        const result = await handlerFn(args);
        return result;
    } catch (err) {
        throw {
            code: ERROR_CODES.INTERNAL_ERROR,
            message: `Handler error: ${err.message}`,
            data: { stack: err.stack },
        };
    }
}

/**
 * Execute a hook handler on a loaded plugin
 *
 * @param {object} params - { pluginId, handler, event }
 * @returns {*} Hook result
 */
async function handleExecuteHook(params) {
    const { pluginId, handler, event } = params;

    if (!pluginId || !handler) {
        throw { code: ERROR_CODES.INVALID_PARAMS, message: 'Missing pluginId or handler' };
    }

    const plugin = loadedPlugins.get(pluginId);
    if (!plugin) {
        throw { code: ERROR_CODES.PLUGIN_NOT_FOUND, message: `Plugin not found: ${pluginId}` };
    }

    // SECURITY: Only allow calling explicitly registered handlers
    const handlerFn = plugin.handlers[handler];
    if (typeof handlerFn !== 'function') {
        throw { code: ERROR_CODES.HANDLER_NOT_FOUND, message: `Hook handler not found: ${handler}` };
    }

    try {
        const result = await handlerFn(event);
        return result;
    } catch (err) {
        throw {
            code: ERROR_CODES.INTERNAL_ERROR,
            message: `Hook error: ${err.message}`,
            data: { stack: err.stack },
        };
    }
}

/**
 * Unload a plugin
 *
 * @param {object} params - { pluginId }
 */
async function handleUnload(params) {
    const { pluginId } = params;

    if (!pluginId) {
        throw { code: ERROR_CODES.INVALID_PARAMS, message: 'Missing pluginId' };
    }

    const plugin = loadedPlugins.get(pluginId);
    if (!plugin) {
        // Not an error if already unloaded
        return { success: true };
    }

    // Call cleanup if available
    const cleanupFn = plugin.module.cleanup || (plugin.module.default && plugin.module.default.cleanup);
    if (typeof cleanupFn === 'function') {
        try {
            await cleanupFn();
        } catch (err) {
            // Log but don't fail on cleanup errors
            console.error(`Plugin cleanup error: ${err.message}`);
        }
    }

    // Clear from require cache (for CommonJS modules)
    try {
        delete require.cache[require.resolve(plugin.path)];
    } catch {
        // Ignore cache clear errors
    }

    loadedPlugins.delete(pluginId);
    return { success: true };
}

/**
 * Shutdown the plugin host process
 */
async function handleShutdown() {
    // Cleanup all plugins with proper async handling
    for (const [pluginId, plugin] of loadedPlugins) {
        const cleanupFn = plugin.module.cleanup || (plugin.module.default && plugin.module.default.cleanup);
        if (typeof cleanupFn === 'function') {
            try {
                await cleanupFn();
            } catch (err) {
                // Log but ignore cleanup errors during shutdown
                console.error(`Plugin ${pluginId} cleanup error: ${err.message}`);
            }
        }
    }
    loadedPlugins.clear();

    // Exit gracefully
    process.exit(0);
}

/**
 * Handle an incoming JSON-RPC request
 */
async function handleRequest(request) {
    const { id, method, params } = request;

    try {
        let result;

        switch (method) {
            case 'load':
                result = await handleLoad(params || {});
                break;
            case 'plugin.call':
                result = await handlePluginCall(params || {});
                break;
            case 'executeHook':
                result = await handleExecuteHook(params || {});
                break;
            case 'unload':
                result = await handleUnload(params || {});
                break;
            case 'shutdown':
                await handleShutdown();
                // handleShutdown calls process.exit, so this won't be reached
                return;
            default:
                return errorResponse(id, ERROR_CODES.METHOD_NOT_FOUND, `Method not found: ${method}`);
        }

        return successResponse(id, result);
    } catch (err) {
        if (err.code !== undefined && err.message !== undefined) {
            // Structured error from handlers
            return errorResponse(id, err.code, err.message, err.data);
        }
        // Unexpected error
        return errorResponse(id, ERROR_CODES.INTERNAL_ERROR, err.message || 'Internal error');
    }
}

/**
 * Process a single line of input
 */
async function processLine(line) {
    const trimmed = line.trim();
    if (!trimmed) return;

    let request;
    try {
        request = JSON.parse(trimmed);
    } catch (err) {
        const response = errorResponse(null, ERROR_CODES.PARSE_ERROR, 'Parse error');
        console.log(JSON.stringify(response));
        return;
    }

    // Validate JSON-RPC format
    if (!request.jsonrpc || request.jsonrpc !== '2.0' || !request.method) {
        const response = errorResponse(request.id || null, ERROR_CODES.INVALID_REQUEST, 'Invalid request');
        console.log(JSON.stringify(response));
        return;
    }

    const response = await handleRequest(request);
    if (response) {
        console.log(JSON.stringify(response));
    }
}

// Set up readline for line-based stdin reading with explicit encoding
const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false,
});

// Set explicit encoding for stdin
process.stdin.setEncoding('utf8');

rl.on('line', (line) => {
    processLine(line).catch((err) => {
        // Catch any unhandled errors
        const response = errorResponse(null, ERROR_CODES.INTERNAL_ERROR, err.message || 'Internal error');
        console.log(JSON.stringify(response));
    });
});

rl.on('close', () => {
    // stdin closed, exit gracefully
    process.exit(0);
});

// Handle uncaught exceptions
process.on('uncaughtException', (err) => {
    console.error(`Uncaught exception: ${err.message}`);
    process.exit(1);
});

process.on('unhandledRejection', (reason) => {
    console.error(`Unhandled rejection: ${reason}`);
    process.exit(1);
});
