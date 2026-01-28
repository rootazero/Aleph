import type { Session, ConnectionStatus } from '../hooks/useGateway'

interface SidebarProps {
  sessions: Session[]
  currentSession: string | null
  status: ConnectionStatus
  onSelectSession: (sessionKey: string) => void
  onNewChat: () => void
  isOpen: boolean
  onClose: () => void
}

export function Sidebar({
  sessions,
  currentSession,
  status,
  onSelectSession,
  onNewChat,
  isOpen,
  onClose,
}: SidebarProps) {
  return (
    <>
      {/* Mobile overlay */}
      {isOpen && (
        <div
          className="fixed inset-0 bg-black/50 z-40 md:hidden"
          onClick={onClose}
        />
      )}

      {/* Sidebar */}
      <aside
        className={`fixed md:static inset-y-0 left-0 z-50 w-72 bg-gray-100 dark:bg-gray-900
                    border-r border-gray-200 dark:border-gray-700
                    transform transition-transform duration-200 ease-in-out
                    ${isOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'}`}
      >
        <div className="flex flex-col h-full">
          {/* Header */}
          <div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
            <div className="flex items-center gap-3">
              <div className="w-8 h-8 rounded-lg bg-aether-600 flex items-center justify-center">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white" className="w-5 h-5">
                  <path fillRule="evenodd" d="M9 4.5a.75.75 0 01.721.544l.813 2.846a3.75 3.75 0 002.576 2.576l2.846.813a.75.75 0 010 1.442l-2.846.813a3.75 3.75 0 00-2.576 2.576l-.813 2.846a.75.75 0 01-1.442 0l-.813-2.846a3.75 3.75 0 00-2.576-2.576l-2.846-.813a.75.75 0 010-1.442l2.846-.813A3.75 3.75 0 007.466 7.89l.813-2.846A.75.75 0 019 4.5z" clipRule="evenodd" />
                </svg>
              </div>
              <div>
                <h1 className="font-semibold text-gray-900 dark:text-white">Aether</h1>
                <div className="flex items-center gap-1.5 text-xs">
                  <span
                    className={`w-2 h-2 rounded-full ${
                      status === 'connected'
                        ? 'bg-green-500'
                        : status === 'connecting'
                        ? 'bg-yellow-500 animate-pulse'
                        : 'bg-red-500'
                    }`}
                  />
                  <span className="text-gray-500 dark:text-gray-400 capitalize">{status}</span>
                </div>
              </div>
            </div>
            <button
              onClick={onClose}
              className="md:hidden p-2 rounded-lg hover:bg-gray-200 dark:hover:bg-gray-800"
            >
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5">
                <path d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z" />
              </svg>
            </button>
          </div>

          {/* New Chat Button */}
          <div className="p-4">
            <button
              onClick={onNewChat}
              className="w-full flex items-center justify-center gap-2 px-4 py-2.5 rounded-xl
                         bg-aether-600 hover:bg-aether-700 text-white font-medium
                         transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5">
                <path d="M10.75 4.75a.75.75 0 00-1.5 0v4.5h-4.5a.75.75 0 000 1.5h4.5v4.5a.75.75 0 001.5 0v-4.5h4.5a.75.75 0 000-1.5h-4.5v-4.5z" />
              </svg>
              New Chat
            </button>
          </div>

          {/* Sessions List */}
          <div className="flex-1 overflow-y-auto p-2">
            <h2 className="px-3 py-2 text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider">
              Recent Sessions
            </h2>
            <nav className="space-y-1">
              {sessions.length === 0 ? (
                <p className="px-3 py-2 text-sm text-gray-500 dark:text-gray-400">
                  No sessions yet
                </p>
              ) : (
                sessions.map((session) => (
                  <button
                    key={session.key}
                    onClick={() => onSelectSession(session.key)}
                    className={`w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left
                               transition-colors ${
                                 currentSession === session.key
                                   ? 'bg-aether-100 dark:bg-aether-900/30 text-aether-700 dark:text-aether-300'
                                   : 'hover:bg-gray-200 dark:hover:bg-gray-800 text-gray-700 dark:text-gray-300'
                               }`}
                  >
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 flex-shrink-0">
                      <path fillRule="evenodd" d="M10 2c-2.236 0-4.43.18-6.57.524C1.993 2.755 1 4.014 1 5.426v5.148c0 1.413.993 2.67 2.43 2.902.848.137 1.705.248 2.57.331v3.443a.75.75 0 001.28.53l3.58-3.579a.78.78 0 01.527-.224 41.202 41.202 0 005.183-.5c1.437-.232 2.43-1.49 2.43-2.903V5.426c0-1.413-.993-2.67-2.43-2.902A41.289 41.289 0 0010 2zm0 7a1 1 0 100-2 1 1 0 000 2zM8 8a1 1 0 11-2 0 1 1 0 012 0zm5 1a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
                    </svg>
                    <div className="flex-1 min-w-0">
                      <p className="text-sm font-medium truncate">
                        {session.agentId || 'Chat'}
                      </p>
                      <p className="text-xs text-gray-500 dark:text-gray-400">
                        {session.messageCount} messages
                      </p>
                    </div>
                  </button>
                ))
              )}
            </nav>
          </div>

          {/* Footer */}
          <div className="p-4 border-t border-gray-200 dark:border-gray-700">
            <div className="text-xs text-gray-500 dark:text-gray-400 text-center">
              Aether Gateway v0.1.0
            </div>
          </div>
        </div>
      </aside>
    </>
  )
}
