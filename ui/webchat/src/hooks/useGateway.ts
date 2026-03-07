import { useState, useEffect, useCallback, useRef } from 'react'

export interface Message {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: Date
  isStreaming?: boolean
}

export interface Session {
  key: string
  agentId: string
  messageCount: number
  lastActive: string
}

interface GatewayEvent {
  topic: string
  data: Record<string, unknown>
  timestamp: number
}

interface JsonRpcRequest {
  jsonrpc: '2.0'
  method: string
  params?: Record<string, unknown>
  id: number | string
}

interface JsonRpcResponse {
  jsonrpc: '2.0'
  result?: unknown
  error?: {
    code: number
    message: string
    data?: unknown
  }
  id: number | string | null
}

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

export interface UseGatewayOptions {
  url?: string
  autoConnect?: boolean
  reconnectDelay?: number
  maxReconnectAttempts?: number
}

export function useGateway(options: UseGatewayOptions = {}) {
  const {
    url = `ws://${window.location.hostname}:18789`,
    autoConnect = true,
    reconnectDelay = 1000,
    maxReconnectAttempts = 5,
  } = options

  const [status, setStatus] = useState<ConnectionStatus>('disconnected')
  const [messages, setMessages] = useState<Message[]>([])
  const [sessions, setSessions] = useState<Session[]>([])
  const [currentSession, setCurrentSession] = useState<string | null>(null)
  const [isTyping, setIsTyping] = useState(false)

  const wsRef = useRef<WebSocket | null>(null)
  const requestIdRef = useRef(0)
  const pendingRequestsRef = useRef<Map<number, {
    resolve: (value: unknown) => void
    reject: (error: Error) => void
  }>>(new Map())
  const reconnectAttemptsRef = useRef(0)
  const streamingMessageRef = useRef<string | null>(null)

  // Connect to Gateway
  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      return
    }

    setStatus('connecting')

    const ws = new WebSocket(url)
    wsRef.current = ws

    ws.onopen = () => {
      setStatus('connected')
      reconnectAttemptsRef.current = 0
      console.log('Connected to Aleph Gateway')
    }

    ws.onclose = () => {
      setStatus('disconnected')
      wsRef.current = null

      // Auto-reconnect
      if (reconnectAttemptsRef.current < maxReconnectAttempts) {
        const delay = reconnectDelay * Math.pow(2, reconnectAttemptsRef.current)
        reconnectAttemptsRef.current++
        console.log(`Reconnecting in ${delay}ms (attempt ${reconnectAttemptsRef.current})`)
        setTimeout(connect, delay)
      } else {
        setStatus('error')
      }
    }

    ws.onerror = (error) => {
      console.error('WebSocket error:', error)
      setStatus('error')
    }

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data) as JsonRpcResponse | { params: GatewayEvent }

        // Handle JSON-RPC response
        if ('result' in data || 'error' in data) {
          const response = data as JsonRpcResponse
          const pending = pendingRequestsRef.current.get(response.id as number)
          if (pending) {
            pendingRequestsRef.current.delete(response.id as number)
            if (response.error) {
              pending.reject(new Error(response.error.message))
            } else {
              pending.resolve(response.result)
            }
          }
          return
        }

        // Handle event notification
        if ('params' in data && data.params?.topic) {
          handleEvent(data.params as GatewayEvent)
        }
      } catch (e) {
        console.error('Failed to parse message:', e)
      }
    }
  }, [url, reconnectDelay, maxReconnectAttempts])

  // Disconnect from Gateway
  const disconnect = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close()
      wsRef.current = null
    }
    setStatus('disconnected')
  }, [])

  // Send JSON-RPC request
  const call = useCallback(<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> => {
    return new Promise((resolve, reject) => {
      if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) {
        reject(new Error('Not connected'))
        return
      }

      const id = ++requestIdRef.current
      const request: JsonRpcRequest = {
        jsonrpc: '2.0',
        method,
        params,
        id,
      }

      pendingRequestsRef.current.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      })

      wsRef.current.send(JSON.stringify(request))

      // Timeout after 30 seconds
      setTimeout(() => {
        if (pendingRequestsRef.current.has(id)) {
          pendingRequestsRef.current.delete(id)
          reject(new Error('Request timeout'))
        }
      }, 30000)
    })
  }, [])

  // Handle incoming events
  const handleEvent = useCallback((event: GatewayEvent) => {
    const { topic, data } = event

    switch (topic) {
      case 'agent.run.started':
        setIsTyping(true)
        streamingMessageRef.current = data.run_id as string
        break

      case 'agent.run.response.chunk':
        if (streamingMessageRef.current) {
          setMessages(prev => {
            const existing = prev.find(m => m.id === streamingMessageRef.current)
            if (existing) {
              return prev.map(m =>
                m.id === streamingMessageRef.current
                  ? { ...m, content: m.content + (data.content as string) }
                  : m
              )
            } else {
              return [...prev, {
                id: streamingMessageRef.current!,
                role: 'assistant',
                content: data.content as string,
                timestamp: new Date(),
                isStreaming: true,
              }]
            }
          })
        }
        break

      case 'agent.run.completed':
        setIsTyping(false)
        if (streamingMessageRef.current) {
          setMessages(prev =>
            prev.map(m =>
              m.id === streamingMessageRef.current
                ? { ...m, isStreaming: false }
                : m
            )
          )
          streamingMessageRef.current = null
        }
        break

      case 'agent.run.error':
        setIsTyping(false)
        streamingMessageRef.current = null
        console.error('Agent error:', data.error)
        break

      case 'session.created':
      case 'session.updated':
        loadSessions()
        break

      default:
        console.log('Unhandled event:', topic, data)
    }
  }, [])

  // Load sessions
  const loadSessions = useCallback(async () => {
    try {
      const result = await call<{ sessions: Session[] }>('sessions.list')
      setSessions(result.sessions || [])
    } catch (e) {
      console.error('Failed to load sessions:', e)
    }
  }, [call])

  // Send message
  const sendMessage = useCallback(async (content: string) => {
    if (!content.trim()) return

    // Add user message immediately
    const userMessage: Message = {
      id: `user-${Date.now()}`,
      role: 'user',
      content,
      timestamp: new Date(),
    }
    setMessages(prev => [...prev, userMessage])

    try {
      await call('agent.run', {
        input: content,
        session_key: currentSession,
        stream: true,
      })
    } catch (e) {
      console.error('Failed to send message:', e)
      // Add error message
      setMessages(prev => [...prev, {
        id: `error-${Date.now()}`,
        role: 'system',
        content: `Error: ${e instanceof Error ? e.message : 'Unknown error'}`,
        timestamp: new Date(),
      }])
    }
  }, [call, currentSession])

  // Clear messages
  const clearMessages = useCallback(() => {
    setMessages([])
  }, [])

  // Switch session
  const switchSession = useCallback((sessionKey: string) => {
    setCurrentSession(sessionKey)
    setMessages([])
    // Load session history
    call<{ messages: Message[] }>('sessions.history', { key: sessionKey })
      .then(result => {
        if (result.messages) {
          setMessages(result.messages.map(m => ({
            ...m,
            timestamp: new Date(m.timestamp),
          })))
        }
      })
      .catch(console.error)
  }, [call])

  // Auto-connect on mount
  useEffect(() => {
    if (autoConnect) {
      connect()
    }
    return () => {
      disconnect()
    }
  }, [autoConnect, connect, disconnect])

  // Load sessions when connected
  useEffect(() => {
    if (status === 'connected') {
      loadSessions()
    }
  }, [status, loadSessions])

  // Fetch available commands from unified registry
  const fetchCommands = useCallback(async () => {
    try {
      const result = await call<{ commands: Array<{
        key: string
        description: string
        icon: string
        hint?: string
        command_type: string
        source_type: string
      }> }>('commands.list')
      return result.commands || []
    } catch (e) {
      console.error('Failed to fetch commands:', e)
      return []
    }
  }, [call])

  return {
    status,
    messages,
    sessions,
    currentSession,
    isTyping,
    connect,
    disconnect,
    sendMessage,
    clearMessages,
    switchSession,
    call,
    fetchCommands,
  }
}
